//! `lmoxide-patch-server`: everything a client needs over HTTP before it talks to the game
//! server.
//!
//! - the stock client's control/patch/login/chat-filter endpoints ([`stock`]), and
//! - the LMOxide patcher protocol under `/lmoxide/v1/` ([`api`]): signed manifests of complete
//!   Windows and Linux installs, already patched for this server ([`patching`]), so
//!   `lmoxide-patcher` (or a GUI) only has to sync files.
//!
//! Plain HTTP always (the stock client knows nothing else); optional built-in HTTPS on a
//! second port. LMOxide patchers don't need it: manifests are signed (`docs/PATCHING.md`).

mod api;
mod app;
mod install;
mod patching;
mod stock;

use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use axum::{Router, routing::get};
use clap::Parser;
use lmoxide_patch::{Platform, manifest::public_key_hex};
use tracing::info;
use tracing_subscriber::EnvFilter;

pub use app::App;

/// Default `--exclude` patterns: logs, crash dumps and leftovers of tools in a client dir.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "*.orig",
    "*.bundled",
    "*.log",
    "*.dmp",
    "*.tmp",
    "ClientLog.txt",
    "CrashLog.txt",
    "LauncherLog.txt",
    "RawCallstack.txt",
    "stderr.txt",
    "dxdiag.txt",
    "OldLogs/**",
    "LocalConfig.xml",
    "LocalConfig.xml.new",
    "lmo-kit.conf",
    "dbDebug.patcher",
    "unins000.*",
    ".lmoxide/**",
];

/// Patch server for LMOxide (LEGO Minifigures Online): stock client endpoints and the
/// LMOxide patcher protocol.
#[derive(Parser, Debug, Clone)]
#[command(version)]
pub struct Config {
    /// Address the listeners bind to.
    #[arg(long, env = "LMOXIDE_BIND", default_value = "127.0.0.1")]
    pub bind: IpAddr,
    /// Plain-HTTP port (control, patch, tracking, patcher protocol).
    #[arg(long, env = "LMOXIDE_HTTP_PORT", default_value_t = 8080)]
    pub http_port: u16,
    /// Host name or IP clients use to reach this server and the game server.
    #[arg(long, env = "LMOXIDE_PUBLIC_HOST", default_value = "127.0.0.1")]
    pub public_host: String,
    /// HTTP port clients connect to, if it differs from --http-port (port mapping).
    #[arg(long, env = "LMOXIDE_PUBLIC_HTTP_PORT")]
    pub public_http_port: Option<u16>,
    /// Host of the game server's universe agent, if not --public-host.
    #[arg(long, env = "LMOXIDE_UNIVERSE_HOST")]
    pub universe_host: Option<String>,
    /// The game server's universe agent port (the client's `UniverseAddr`).
    #[arg(long, env = "LMOXIDE_UNIVERSE_PORT", default_value_t = 7100)]
    pub universe_port: u16,
    /// `host:port` patched into the executables' chat filter URL (at most 21 characters);
    /// default: the public HTTP address.
    #[arg(long, env = "LMOXIDE_CHAT_FILTER_HOST")]
    pub chat_filter_host: Option<String>,
    /// HTTPS port (needs --tls-cert and --tls-key). Off when unset.
    #[arg(long, env = "LMOXIDE_HTTPS_PORT", requires_all = ["tls_cert", "tls_key"])]
    pub https_port: Option<u16>,
    /// HTTPS port clients connect to, if it differs from --https-port.
    #[arg(long, env = "LMOXIDE_PUBLIC_HTTPS_PORT")]
    pub public_https_port: Option<u16>,
    /// PEM certificate chain for HTTPS.
    #[arg(long, env = "LMOXIDE_TLS_CERT")]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key for HTTPS.
    #[arg(long, env = "LMOXIDE_TLS_KEY")]
    pub tls_key: Option<PathBuf>,
    /// Display name in /lmoxide/v1/info and manifests.
    #[arg(long, env = "LMOXIDE_SERVER_NAME", default_value = "LMOxide")]
    pub server_name: String,
    /// Windows client ("LEGO Minifigures Online" folder, with RDB/): served to the stock
    /// patcher and as the `windows` manifest.
    #[arg(long, env = "LMOXIDE_CLIENT_DIR")]
    pub client_dir: Option<PathBuf>,
    /// Native Linux client (extracted LMO_Linux_v1.5.557666): the `linux` manifest.
    #[arg(long, env = "LMOXIDE_LINUX_CLIENT_DIR")]
    pub linux_client_dir: Option<PathBuf>,
    /// Files added to / replacing files of the Windows install.
    #[arg(long, env = "LMOXIDE_WINDOWS_OVERLAY_DIR")]
    pub windows_overlay_dir: Option<PathBuf>,
    /// Files added to / replacing files of the Linux install (the client kit's
    /// libgconf-2.so.4, liblmofixes.so, lmo launcher).
    #[arg(long, env = "LMOXIDE_LINUX_OVERLAY_DIR")]
    pub linux_overlay_dir: Option<PathBuf>,
    /// Paths left out of the Windows install and deleted by patchers (comma-separated).
    #[arg(long, env = "LMOXIDE_WINDOWS_REMOVE", value_delimiter = ',')]
    pub windows_remove: Vec<String>,
    /// Paths left out of the Linux install and deleted by patchers (comma-separated). The
    /// bundled 2016 libstdc++ breaks today's system libraries.
    #[arg(long, env = "LMOXIDE_LINUX_REMOVE", value_delimiter = ',', default_value = "libstdc++.so.6")]
    pub linux_remove: Vec<String>,
    /// Glob patterns of client-dir files left out of manifests (comma-separated; a pattern
    /// without `/` matches the file name anywhere). Replaces the default list.
    #[arg(long, env = "LMOXIDE_EXCLUDE", value_delimiter = ',', default_values_t = DEFAULT_EXCLUDES.iter().map(|s| s.to_string()))]
    pub exclude: Vec<String>,
    /// Don't serve `<file>.orig` in place of `<file>` (the stock copies patch tools keep).
    #[arg(long, env = "LMOXIDE_NO_ORIG")]
    pub no_orig: bool,
    /// The game server's login public key (512 hex characters), patched into the executables.
    /// Polled; binaries are re-patched when it changes. Unpatched while missing.
    #[arg(long, env = "LMOXIDE_LOGIN_PUBLIC_KEY_FILE")]
    pub login_public_key_file: Option<PathBuf>,
    /// Seconds between checks of --login-public-key-file.
    #[arg(long, env = "LMOXIDE_KEY_POLL_SECS", default_value_t = 10)]
    pub key_poll_secs: u64,
    /// State: manifest signing key, hash cache, patched files, upstream cache.
    #[arg(long, env = "LMOXIDE_DATA_DIR", default_value = "lmoxide-patch-data")]
    pub data_dir: PathBuf,
    /// Threads hashing client files.
    #[arg(long, env = "LMOXIDE_HASH_JOBS", default_value_t = 4)]
    pub hash_jobs: usize,
    /// Patch server to fetch files missing from the local client from (Funcom's original,
    /// e.g. http://update.lego.funcom.com/legoupm). Disabled when unset.
    #[arg(long, env = "LMOXIDE_UPSTREAM")]
    pub upstream: Option<String>,
    /// Where files fetched from --upstream are cached (default <data-dir>/rdb-cache).
    #[arg(long, env = "LMOXIDE_CACHE_DIR")]
    pub cache_dir: Option<PathBuf>,
    /// Serve the client's download list as-is instead of dropping resources missing from
    /// the local RDB.
    #[arg(long, env = "LMOXIDE_NO_FILTER_DOWNLOAD_LIST")]
    pub no_filter_download_list: bool,
    /// Client kit tarball (patches/linux-client/make-kit.sh) served at `/client-kit.tar.gz`.
    #[arg(long, env = "LMOXIDE_CLIENT_KIT")]
    pub client_kit: Option<PathBuf>,
}

impl Config {
    pub fn public_http_addr(&self) -> String {
        format!("{}:{}", self.public_host, self.public_http_port.unwrap_or(self.http_port))
    }

    pub fn public_https_addr(&self) -> Option<String> {
        let port = self.public_https_port.or(self.https_port)?;
        Some(format!("{}:{port}", self.public_host))
    }

    pub fn universe_addr(&self) -> String {
        format!("{}:{}", self.universe_host.as_deref().unwrap_or(&self.public_host), self.universe_port)
    }

    pub fn chat_filter_host(&self) -> String {
        self.chat_filter_host.clone().unwrap_or_else(|| self.public_http_addr())
    }

    pub fn client_dir_for(&self, p: Platform) -> Option<&Path> {
        match p {
            Platform::Windows => self.client_dir.as_deref(),
            Platform::Linux => self.linux_client_dir.as_deref(),
        }
    }

    pub fn overlay_dir_for(&self, p: Platform) -> Option<&Path> {
        match p {
            Platform::Windows => self.windows_overlay_dir.as_deref(),
            Platform::Linux => self.linux_overlay_dir.as_deref(),
        }
    }

    pub fn remove_for(&self, p: Platform) -> &[String] {
        match p {
            Platform::Windows => &self.windows_remove,
            Platform::Linux => &self.linux_remove,
        }
    }
}

/// Requests are all GETs; anything with a body larger than this is refused.
const MAX_REQUEST_BODY: usize = 16 * 1024;

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/control/{folder}/LocalConfig.xml", get(stock::local_config))
        .route("/control/{folder}/PatchInfoClient.txt", get(stock::patch_info_client))
        .route("/patch/rdb/res/{a}/{b}/{rest}", get(stock::rdb_resource))
        .route("/patch/rdb/full/{hash}", get(stock::full_file))
        .route("/patch/{*path}", get(stock::patch_file))
        .route("/track", get(stock::track))
        .route("/rest/chat_filter", get(stock::chat_filter))
        .route("/auth/", get(stock::login_page))
        .route("/client-kit.tar.gz", get(stock::client_kit))
        .route("/lmoxide/v1/info", get(api::info))
        .route("/lmoxide/v1/manifest/{platform}", get(api::manifest))
        .route("/lmoxide/v1/files/{sha256}", get(api::file))
        .route("/lmoxide/v1/login-key", get(api::login_key))
        .fallback(stock::fallback)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(MAX_REQUEST_BODY))
        .with_state(app)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,lmoxide_patch_server=debug".into()),
        )
        .init();
    let cfg = Arc::new(Config::parse());
    for (what, dir) in [("--client-dir", &cfg.client_dir), ("--linux-client-dir", &cfg.linux_client_dir)] {
        if let Some(d) = dir {
            anyhow::ensure!(d.is_dir(), "{what} {} is not a directory", d.display());
        }
    }
    let signing = app::load_signing_key(&cfg.data_dir)?;
    info!(key = %public_key_hex(&signing.verifying_key()), "manifest signing key (patchers pin this)");
    let upstream = cfg.upstream.as_ref().map(|base| stock::Upstream {
        base: base.trim_end_matches('/').to_string(),
        cache: cfg.cache_dir.clone().unwrap_or_else(|| cfg.data_dir.join("rdb-cache")),
        client: reqwest::Client::new(),
    });
    if let Some(u) = &upstream {
        info!(base = %u.base, cache = %u.cache.display(), "upstream fallback enabled");
    }
    let app = Arc::new(App::new(cfg.clone(), signing, upstream));

    // Stock endpoints are ready after this; manifests follow once everything is hashed.
    let prepared = app::prepare(&app).await?;
    let bg = app.clone();
    tokio::spawn(async move {
        if let Err(e) = app::publish(&bg, prepared).await {
            tracing::error!("building manifests failed: {e:#}");
        }
        app::watch(bg).await;
    });

    let router = router(app.clone());
    let http_addr = SocketAddr::new(cfg.bind, cfg.http_port);
    let listener = tokio::net::TcpListener::bind(http_addr).await.with_context(|| format!("binding {http_addr}"))?;
    info!("http listening on {http_addr} (public {})", cfg.public_http_addr());
    // No Nagle: thousands of small files per install would each wait on delayed ACKs.
    let listener = axum::serve::ListenerExt::tap_io(listener, |tcp| {
        let _ = tcp.set_nodelay(true);
    });
    let http = axum::serve(listener, router.clone());

    let https = async {
        let (Some(port), Some(cert), Some(key)) = (cfg.https_port, &cfg.tls_cert, &cfg.tls_key) else {
            return std::future::pending::<anyhow::Result<()>>().await;
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
            .await
            .context("loading --tls-cert/--tls-key")?;
        let addr = SocketAddr::new(cfg.bind, port);
        info!("https listening on {addr}");
        axum_server::bind_rustls(addr, tls).serve(router.into_make_service()).await?;
        Ok(())
    };

    tokio::select! {
        r = http => r?,
        r = https => r?,
        _ = shutdown() => info!("shutting down"),
    }
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_defaults_and_env_names() {
        let cfg = Config::try_parse_from(["lmoxide-patch-server"]).unwrap();
        assert_eq!(cfg.public_http_addr(), "127.0.0.1:8080");
        assert_eq!(cfg.universe_addr(), "127.0.0.1:7100");
        assert_eq!(cfg.linux_remove, ["libstdc++.so.6"]);
        assert!(cfg.exclude.contains(&"*.orig".to_string()));
        let cfg = Config::try_parse_from([
            "x",
            "--public-host",
            "lmo.example",
            "--public-http-port",
            "80",
            "--universe-host",
            "game",
            "--exclude",
            "a,b/**",
        ])
        .unwrap();
        assert_eq!(cfg.public_http_addr(), "lmo.example:80");
        assert_eq!(cfg.universe_addr(), "game:7100");
        assert_eq!(cfg.exclude, ["a", "b/**"]);
        assert!(Config::try_parse_from(["x", "--https-port", "8443"]).is_err());
    }
}
