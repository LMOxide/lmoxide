//! `lmoxide`: LMOxide in one process (docs/SERVER-ARCHITECTURE.md "Hosting"). The game server
//! (every game mode), the REST API and the patch server run together from one config file, the
//! API and the patch server (and optionally a web UI's files) behind one HTTP listener. The API
//! reaches the game server over a loopback control port (a free one) with a token made at start.
//!
//!   lmoxide [--config lmoxide.toml]       run everything (docker/lmoxide.example.toml)
//!   lmoxide game <lmoxide-server args>    the game server's commands (`game account gm <user> 1`)
//!   lmoxide api <lmoxide-api args>        the API's commands (`api key create bot --scope read`)
//!
//! The separate programs (lmoxide-server, lmoxide-api, lmoxide-patch-server) still work for
//! anyone who wants to run the parts apart.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context;
use clap::Parser;
use serde::Deserialize;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// The config file. Each part's table takes that program's options (without the leading `--`):
/// `[game]` lmoxide-server's, `[api]` lmoxide-api's, `[patch]` lmoxide-patch-server's.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct File {
    /// The database (game server and API) and the login key's directory.
    data_dir: PathBuf,
    /// The host name or IP players reach the server at (game ports and HTTP).
    public_host: String,
    #[serde(default)]
    http: Http,
    #[serde(default)]
    game: toml::Table,
    #[serde(default)]
    api: toml::Table,
    #[serde(default)]
    patch: toml::Table,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Http {
    /// Where the API, the patch server and the web files listen (put a TLS proxy in front).
    #[serde(default = "default_http_bind")]
    bind: SocketAddr,
    /// A folder served for every path nothing else answers (a built web UI).
    #[serde(default)]
    web_dir: Option<PathBuf>,
}

impl Default for Http {
    fn default() -> Self {
        Http { bind: default_http_bind(), web_dir: None }
    }
}

fn default_http_bind() -> SocketAddr {
    "0.0.0.0:8080".parse().expect("address")
}

#[derive(Parser)]
#[command(name = "lmoxide", version, about = "LMOxide in one process: game server, REST API and patch server")]
struct Cli {
    /// The config file.
    #[arg(long, env = "LMOXIDE_CONFIG", default_value = "lmoxide.toml")]
    config: PathBuf,
}

#[derive(Parser)]
struct GameArgs {
    #[command(flatten)]
    storage: lmoxide_server::Storage,
    #[command(flatten)]
    cfg: lmoxide_server::Config,
}

#[derive(Parser)]
struct ApiArgs {
    #[command(flatten)]
    cfg: lmoxide_api::Config,
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    match std::env::args().nth(1).as_deref() {
        Some("game") => return lmoxide_server::run_cli_from(std::iter::once(program).chain(args.skip(1))),
        Some("api") => return lmoxide_api::run_cli_from(std::iter::once(program).chain(args.skip(1))),
        _ => {}
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.config).with_context(|| format!("reading {}", cli.config.display()))?;
    let file: File = toml::from_str(&text).with_context(|| format!("{}", cli.config.display()))?;
    tokio::runtime::Runtime::new()?.block_on(run(file))
}

/// A config table as command line options: `key = "v"` → `--key=v`, `key = true` → `--key`
/// (false: left out), numbers as they are, arrays joined with commas. `defaults` are added for
/// keys the table doesn't set.
fn options(program: &str, table: &toml::Table, defaults: &[(&str, String)]) -> anyhow::Result<Vec<String>> {
    let mut out = vec![program.to_string()];
    for (key, value) in table {
        let v = match value {
            toml::Value::Boolean(true) => {
                out.push(format!("--{key}"));
                continue;
            }
            toml::Value::Boolean(false) => continue,
            toml::Value::String(s) => s.clone(),
            toml::Value::Integer(n) => n.to_string(),
            toml::Value::Float(f) => f.to_string(),
            toml::Value::Array(a) => a.iter().map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).collect::<Vec<_>>().join(","),
            other => anyhow::bail!("{program}: {key} = {other}: not an option value"),
        };
        out.push(format!("--{key}={v}"));
    }
    for (key, value) in defaults {
        if !table.contains_key(*key) {
            out.push(format!("--{key}={value}"));
        }
    }
    Ok(out)
}

async fn run(file: File) -> anyhow::Result<()> {
    // the API's credential for the game server's control interface, new at every start, and a
    // free loopback port for it
    let token: String = (0..32).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
    let control = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.to_string();
    let data_dir = file.data_dir.display().to_string();
    let (http_ip, http_port) = (file.http.bind.ip().to_string(), file.http.bind.port().to_string());

    let game = GameArgs::try_parse_from(options(
        "game",
        &file.game,
        &[
            ("data-dir", data_dir.clone()),
            ("public-host", file.public_host.clone()),
            ("control-bind", control.clone()),
            ("control-token", token.clone()),
        ],
    )?)
    .context("[game]")?;
    let modes = file.game.get("modes").and_then(|m| m.as_str()).map(str::to_string);
    let mut patch_defaults = vec![
        ("bind", http_ip),
        ("http-port", http_port.clone()),
        ("public-host", file.public_host.clone()),
        ("login-public-key-file", file.data_dir.join("login-public-key.txt").display().to_string()),
        ("data-dir", file.data_dir.join("patch-data").display().to_string()),
        // the API answers on the same host and port
        ("api-url", format!("http://{}:{http_port}", file.public_host)),
    ];
    if let Some(m) = modes {
        patch_defaults.push(("modes", m));
    }
    let patch = Arc::new(lmoxide_patch_server::Config::try_parse_from(options("patch", &file.patch, &patch_defaults)?).context("[patch]")?);
    // the API lets the same accounts into the world editor's routes as the game into its mode
    // (and its admins the admins' commands)
    let editor_level = game.cfg.world_editor_gm_level.to_string();
    let admin_level = game.cfg.admin_gm_level.to_string();
    let api = ApiArgs::try_parse_from(options(
        "api",
        &file.api,
        &[("control-url", format!("http://{control}")), ("control-token", token), ("editor-gm-level", editor_level), ("admin-gm-level", admin_level)],
    )?)
        .context("[api]")?
        .cfg;

    let database = game.storage.database();
    let mut game_task = tokio::spawn(lmoxide_server::serve(game.cfg, game.storage));
    // the API opens the database once the game server has migrated it (its control interface
    // listens after that)
    let started = tokio::time::Instant::now();
    while tokio::net::TcpStream::connect(&control).await.is_err() {
        anyhow::ensure!(!game_task.is_finished(), "the game server stopped while starting");
        anyhow::ensure!(started.elapsed() < Duration::from_secs(120), "the game server's control interface didn't open");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let db = Arc::new(lmoxide_db::Db::open(&database).map_err(|e| anyhow::anyhow!("opening {}: {e}", database.display()))?);
    let (patch_routes, _lock) = lmoxide_patch_server::start(patch).await.context("patch server")?;
    let mut router = lmoxide_api::app(api, db)?.merge(patch_routes);
    if let Some(dir) = &file.http.web_dir {
        router = router.fallback_service(tower_http::services::ServeDir::new(dir));
    }
    let listener = tokio::net::TcpListener::bind(file.http.bind).await.with_context(|| format!("binding {}", file.http.bind))?;
    info!(bind = %file.http.bind, "http: API, patch server{}", if file.http.web_dir.is_some() { ", web files" } else { "" });
    let http = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>());
    tokio::select! {
        r = &mut game_task => r?,
        r = http => r.context("http"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_become_options() {
        let table: toml::Table = toml::from_str("modes = \"progression:7100\"\nauto-register = true\nbot = false\nopen-world-cap = 30").unwrap();
        let args = options("game", &table, &[("data-dir", "d".into()), ("modes", "x".into())]).unwrap();
        assert!(args.contains(&"--modes=progression:7100".to_string()));
        assert!(args.contains(&"--auto-register".to_string()));
        assert!(args.contains(&"--open-world-cap=30".to_string()));
        assert!(args.contains(&"--data-dir=d".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--bot") || a == "--modes=x"));
    }

    #[test]
    fn the_example_config_parses() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docker/lmoxide.example.toml")).unwrap();
        let file: File = toml::from_str(&text).unwrap();
        GameArgs::try_parse_from(options("game", &file.game, &[("data-dir", "d".into())]).unwrap()).unwrap();
        lmoxide_patch_server::Config::try_parse_from(options("patch", &file.patch, &[]).unwrap()).unwrap();
        ApiArgs::try_parse_from(options("api", &file.api, &[]).unwrap()).unwrap();
    }
}
