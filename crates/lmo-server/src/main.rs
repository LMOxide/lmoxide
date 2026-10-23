mod conn;
mod http;
mod territory;
mod universe;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Server emulator for LEGO Minifigures Online.
#[derive(Parser, Debug, Clone)]
pub struct Config {
    /// Address all listeners bind to.
    #[arg(long, env = "LMO_BIND", default_value = "127.0.0.1")]
    pub bind: std::net::IpAddr,
    /// Host name or IP the client should use to reach this server.
    #[arg(long, env = "LMO_PUBLIC_HOST", default_value = "127.0.0.1")]
    pub public_host: String,
    /// HTTP port serving the control (config), patch and tracking endpoints.
    #[arg(long, env = "LMO_HTTP_PORT", default_value_t = 8080)]
    pub http_port: u16,
    /// Universe agent port (the client's `UniverseAddr`).
    #[arg(long, env = "LMO_UNIVERSE_PORT", default_value_t = 7100)]
    pub universe_port: u16,
    /// Territory manager / player agent port.
    #[arg(long, env = "LMO_TERRITORY_PORT", default_value_t = 7101)]
    pub territory_port: u16,
    /// Path to an installed client ("LEGO Minifigures Online" folder); its files are served
    /// to the patcher.
    #[arg(long, env = "LMO_CLIENT_DIR")]
    pub client_dir: Option<PathBuf>,
    /// Serve the client's download list as-is instead of dropping resources missing from
    /// the local RDB.
    #[arg(long)]
    pub no_filter_download_list: bool,
}

impl Config {
    fn addr(&self, port: u16) -> SocketAddr {
        SocketAddr::new(self.bind, port)
    }
    pub fn public_addr(&self, port: u16) -> String {
        format!("{}:{}", self.public_host, port)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,lmo_server=debug".into()),
        )
        .init();
    let cfg = Arc::new(Config::parse());

    let http = tokio::spawn(http::serve(cfg.clone(), cfg.addr(cfg.http_port)));
    let universe = tokio::spawn(conn::listen(
        "universe",
        cfg.addr(cfg.universe_port),
        cfg.clone(),
        universe::handle,
    ));
    let territory = tokio::spawn(conn::listen(
        "territory",
        cfg.addr(cfg.territory_port),
        cfg.clone(),
        territory::handle,
    ));

    tokio::select! {
        r = http => r??,
        r = universe => r??,
        r = territory => r??,
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}
