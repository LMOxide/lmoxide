//! LMOxide REST API: administration of the game server and public information for a web UI,
//! Discord bots and scripts. Shares the database with lmoxide-server and reaches the running
//! server through its control interface (lmoxide-control). Docs: docs/API.md; the OpenAPI
//! document is served at `/v1/openapi.json`.

mod auth;
mod error;
mod routes;

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    Json, Router,
    http::{HeaderValue, Method, header},
    routing::get,
};
use clap::Parser;
use lmoxide_db::{Db, Scope};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};

use crate::error::ApiError;

/// LMOxide REST API. Without a subcommand it serves the API.
#[derive(Parser, Debug)]
#[command(name = "lmoxide-api", version)]
struct Cli {
    #[command(flatten)]
    storage: Storage,
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    cfg: Config,
}

#[derive(clap::Args, Debug)]
struct Storage {
    /// Directory of the game server's database.
    #[arg(long, env = "LMOXIDE_DATA_DIR", default_value = "data", global = true)]
    data_dir: PathBuf,
    /// SQLite database file (default <data-dir>/lmoxide.db), shared with lmoxide-server.
    #[arg(long, env = "LMOXIDE_DATABASE", global = true)]
    database: Option<PathBuf>,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Manage API keys (for bots and scripts).
    #[command(subcommand)]
    Key(KeyCmd),
    /// Print the OpenAPI document.
    Openapi,
}

#[derive(clap::Subcommand, Debug)]
enum KeyCmd {
    /// Create a key; its secret is printed once.
    Create {
        name: String,
        /// read, moderate or admin.
        #[arg(long, default_value = "read")]
        scope: String,
    },
    List,
    Revoke { id: i64 },
}

#[derive(clap::Args, Debug)]
pub struct Config {
    /// Address to listen on.
    #[arg(long, env = "LMOXIDE_API_BIND", default_value = "127.0.0.1:8090")]
    pub bind: SocketAddr,
    /// TLS certificate chain (PEM). With --tls-key the API speaks HTTPS; otherwise put it behind
    /// a TLS-terminating reverse proxy.
    #[arg(long, env = "LMOXIDE_API_TLS_CERT", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,
    /// TLS private key (PEM).
    #[arg(long, env = "LMOXIDE_API_TLS_KEY", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
    /// The game server's control interface.
    #[arg(long, env = "LMOXIDE_CONTROL_URL", default_value = "http://127.0.0.1:7300")]
    pub control_url: String,
    /// The game server's control token (its --control-token). Without it, live features
    /// (players online, kicks, broadcasts) are unavailable.
    #[arg(long, env = "LMOXIDE_CONTROL_TOKEN", hide_env_values = true)]
    pub control_token: Option<String>,
    /// Read the control token from a file (e.g. a Docker secret).
    #[arg(long, env = "LMOXIDE_CONTROL_TOKEN_FILE", conflicts_with = "control_token")]
    pub control_token_file: Option<PathBuf>,
    /// Require a read-scope credential for /v1/status and /v1/online too.
    #[arg(long, env = "LMOXIDE_API_PRIVATE_STATUS")]
    pub private_status: bool,
    /// Allow anyone to create game accounts through POST /v1/auth/register (throttled per
    /// address).
    #[arg(long, env = "LMOXIDE_API_OPEN_REGISTRATION")]
    pub open_registration: bool,
    /// Lifetime of login sessions.
    #[arg(long, env = "LMOXIDE_API_SESSION_TTL_HOURS", default_value_t = 24)]
    pub session_ttl_hours: u32,
    /// GM level from which an account's login session has the moderate scope.
    #[arg(long, env = "LMOXIDE_API_MODERATOR_GM_LEVEL", default_value_t = 1)]
    pub moderator_gm_level: u32,
    /// GM level from which an account's login session has the admin scope.
    #[arg(long, env = "LMOXIDE_API_ADMIN_GM_LEVEL", default_value_t = 2)]
    pub admin_gm_level: u32,
    /// Origins allowed to call the API from a browser (CORS), e.g. https://admin.example.org.
    /// Comma-separated; none by default.
    #[arg(long, env = "LMOXIDE_API_CORS_ORIGINS", value_delimiter = ',')]
    pub cors_origins: Vec<String>,
    /// Take the client address from the last X-Forwarded-For entry (only behind a reverse
    /// proxy that sets it), for rate limits.
    #[arg(long, env = "LMOXIDE_API_TRUST_FORWARDED_FOR")]
    pub trust_forwarded_for: bool,
}

pub struct AppState {
    pub db: Arc<Db>,
    pub control: Option<lmoxide_control::Client>,
    pub cfg: Config,
    pub limiter: auth::RateLimiter,
    pub lockout: auth::Lockout,
}

/// Runs a database call on a blocking thread.
pub async fn blocking<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&Db) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || f(&db)).await.map_err(ApiError::internal)?
}

#[derive(OpenApi)]
#[openapi(
    info(title = "LMOxide API", description = "Administration and status of an LMOxide (LEGO Minifigures Online) server."),
    modifiers(&BearerAuth),
    security(("bearer" = [])),
    tags(
        (name = "public", description = "Server status; public unless --private-status"),
        (name = "auth", description = "Login sessions for game accounts"),
        (name = "players", description = "The running game server"),
        (name = "accounts", description = "Accounts and characters"),
        (name = "admin", description = "API keys and the audit log"),
    )
)]
struct ApiDoc;

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi.components.get_or_insert_default().add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("An API key or a session token from /v1/auth/login (both start with lmox_)"))
                    .build(),
            ),
        );
    }
}

fn openapi() -> (axum::Router<Arc<AppState>>, utoipa::openapi::OpenApi) {
    utoipa_axum::router::OpenApiRouter::with_openapi(ApiDoc::openapi()).merge(routes::router()).split_for_parts()
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let open_db = || {
        let path = cli.storage.database.clone().unwrap_or_else(|| cli.storage.data_dir.join("lmoxide.db"));
        Db::open(&path).with_context(|| format!("opening {}", path.display()))
    };
    match cli.cmd {
        Some(Cmd::Key(cmd)) => keys(&open_db()?, cmd),
        Some(Cmd::Openapi) => {
            println!("{}", openapi().1.to_pretty_json()?);
            Ok(())
        }
        None => tokio::runtime::Runtime::new()?.block_on(serve(cli.cfg, Arc::new(open_db()?))),
    }
}

fn keys(db: &Db, cmd: KeyCmd) -> anyhow::Result<()> {
    match cmd {
        KeyCmd::Create { name, scope } => {
            let scope: Scope = scope.parse()?;
            let (key, secret) = db.create_api_key(&name, scope)?;
            db.audit("cli", "api-key-create", Some(&key.name), Some(scope.as_str()))?;
            println!("created API key {} ({}, id {})", key.name, scope.as_str(), key.id);
            println!("secret (shown once): {secret}");
        }
        KeyCmd::List => {
            println!("{:<5} {:<24} {:<9} {:<8} last used", "id", "name", "scope", "revoked");
            for k in db.list_api_keys()? {
                let used = k.last_used_at.map_or("never".into(), |t| t.to_string());
                println!("{:<5} {:<24} {:<9} {:<8} {used}", k.id, k.name, k.scope.as_str(), k.revoked);
            }
        }
        KeyCmd::Revoke { id } => {
            db.revoke_api_key(id)?;
            db.audit("cli", "api-key-revoke", Some(&id.to_string()), None)?;
            println!("revoked API key {id}");
        }
    }
    Ok(())
}

async fn serve(cfg: Config, db: Arc<Db>) -> anyhow::Result<()> {
    let token = match (&cfg.control_token, &cfg.control_token_file) {
        (Some(t), _) => Some(t.trim().to_string()),
        (None, Some(f)) => Some(std::fs::read_to_string(f)?.trim().to_string()),
        (None, None) => None,
    };
    let control = token.map(|t| lmoxide_control::Client::new(&cfg.control_url, t));
    if control.is_none() {
        warn!("no control token: players online, kicks and broadcasts are unavailable");
    }
    if cfg.tls_cert.is_none() && !cfg.bind.ip().is_loopback() {
        warn!("serving plain HTTP on {}: use --tls-cert/--tls-key or a TLS reverse proxy", cfg.bind);
    }
    let cors = (!cfg.cors_origins.is_empty()).then(|| {
        let origins: Vec<HeaderValue> = cfg.cors_origins.iter().filter_map(|o| o.parse().ok()).collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
            .max_age(Duration::from_secs(3600))
    });
    let (bind, tls) = (cfg.bind, cfg.tls_cert.clone().zip(cfg.tls_key.clone()));
    let state = Arc::new(AppState { db, control, cfg, limiter: Default::default(), lockout: Default::default() });

    let (api, doc) = openapi();
    let doc = Arc::new(doc);
    let mut app: Router = api
        .route("/v1/openapi.json", get(move || std::future::ready(Json(doc.as_ref().clone()))))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::rate_limit))
        .with_state(state)
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(TimeoutLayer::with_status_code(axum::http::StatusCode::REQUEST_TIMEOUT, Duration::from_secs(30)))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(TraceLayer::new_for_http());
    if let Some(cors) = cors {
        app = app.layer(cors);
    }
    let service = app.into_make_service_with_connect_info::<SocketAddr>();

    let handle = axum_server::Handle::new();
    let h = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        h.graceful_shutdown(Some(Duration::from_secs(10)));
    });
    match tls {
        Some((cert, key)) => {
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
                .with_context(|| format!("loading {} / {}", cert.display(), key.display()))?;
            info!("API listening on https://{bind}");
            axum_server::bind_rustls(bind, tls).handle(handle).serve(service).await?;
        }
        None => {
            info!("API listening on http://{bind}");
            axum_server::bind(bind).handle(handle).serve(service).await?;
        }
    }
    Ok(())
}

/// Ctrl-C, or SIGTERM (`docker stop`).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
