mod achievements;
mod actions;
mod auras;
mod admin;
mod ballista;
mod buildie;
mod cannon;
mod chat;
mod collection;
mod combat;
mod community;
mod content;
mod conn;
mod control;
mod devcmd;
mod dash;
mod editor;
mod discovery;
mod effects;
mod encounter;
mod game;
mod gamedata;
mod gates;
mod jumppad;
mod knubot;
mod loot;
mod minifigs;
mod mirrors;
mod npc;
mod npc_spells;
mod pistons;
mod plates;
mod procs;
mod progression;
mod quests;
mod projectile;
mod scenery;
mod scripts;
mod security;
mod sessions;
mod shop;
mod skills;
mod spawns;
mod smash;
mod state;
mod store;
mod survey;
mod switch;
mod teleport;
mod tiki;
mod tutorial;
mod territory;
mod universe;
mod walls;
mod world;
mod world_data;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// LMOxide game server for LEGO Minifigures Online: universe agent, territory manager, game
/// and chat servers, and the internal control interface for lmoxide-api.
///
/// Without a subcommand it runs the server. The client's HTTP services (configuration,
/// patching, login page) are lmoxide-patch-server's job.
#[derive(Parser, Debug)]
#[command(name = "lmoxide-server", version)]
struct Cli {
    #[command(flatten)]
    storage: Storage,
    #[command(subcommand)]
    cmd: Option<admin::Cmd>,
    #[command(flatten)]
    serve: Config,
}

#[derive(clap::Args, Debug, Clone)]
pub struct Storage {
    /// Directory for the database and the login public key file.
    #[arg(long, env = "LMOXIDE_DATA_DIR", default_value = "lmoxide-data", global = true)]
    pub data_dir: PathBuf,
    /// SQLite database file (default <data-dir>/lmoxide.db), shared with lmoxide-api.
    #[arg(long, env = "LMOXIDE_DATABASE", global = true)]
    pub database: Option<PathBuf>,
}

impl Storage {
    pub fn database(&self) -> PathBuf {
        self.database.clone().unwrap_or_else(|| self.data_dir.join("lmoxide.db"))
    }

    pub fn open(&self) -> anyhow::Result<lmoxide_db::Db> {
        let path = self.database();
        lmoxide_db::Db::open(&path).map_err(|e| anyhow::anyhow!("opening {}: {e}", path.display()))
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct Config {
    /// Address the game listeners bind to.
    #[arg(long, env = "LMOXIDE_BIND", default_value = "127.0.0.1")]
    pub bind: std::net::IpAddr,
    /// Host name or IP the client should use to reach this server.
    #[arg(long, env = "LMOXIDE_PUBLIC_HOST", default_value = "127.0.0.1")]
    pub public_host: String,
    /// Universe agent port (the client's `UniverseAddr`).
    #[arg(long, env = "LMOXIDE_UNIVERSE_PORT", default_value_t = 7100)]
    pub universe_port: u16,
    /// Territory manager / player agent port.
    #[arg(long, env = "LMOXIDE_TERRITORY_PORT", default_value_t = 7101)]
    pub territory_port: u16,
    /// Game server port (playfields).
    #[arg(long, env = "LMOXIDE_GAME_PORT", default_value_t = 7102)]
    pub game_port: u16,
    /// Chat server port (plain-text IRC).
    #[arg(long, env = "LMOXIDE_CHAT_PORT", default_value_t = 7200)]
    pub chat_port: u16,
    /// Playfield new characters start in.
    #[arg(long, env = "LMOXIDE_START_PLAYFIELD", default_value_t = 1200)]
    pub start_playfield: u32,
    /// Create an account on the first login with an unknown username (the password given
    /// becomes the account's), and let accounts without a password set it by
    /// logging in. Convenient on a LAN; leave off on public servers.
    #[arg(long, env = "LMOXIDE_AUTO_REGISTER")]
    pub auto_register: bool,
    /// GM level every account has at least (stat GmLevel). Free-text chat and the client's GM
    /// chat commands need a non-zero level; ordinary LMO players only had macro chat. Per-account
    /// levels: `lmoxide-server account gm`.
    #[arg(long, env = "LMOXIDE_DEFAULT_GM_LEVEL", default_value_t = 0)]
    pub default_gm_level: u32,
    /// GM level needed for the test commands typed into chat (`!tp`, `!goto`, ...).
    #[arg(long, env = "LMOXIDE_DEVCMD_GM_LEVEL", default_value_t = 1)]
    pub devcmd_gm_level: u32,
    /// What kind of server this is: `play` (every character at the level cap with every
    /// minifigure fully ranked), `progression` (characters level up from level 1, see
    /// --progression) or `survey` (nothing moves or attacks; surveyors place and mark content
    /// from the web UI, docs/SURVEY.md). Run one server per mode, on its own ports and data dir.
    #[arg(long, env = "LMOXIDE_MODE", value_enum, default_value_t = Mode::Play)]
    pub mode: Mode,
    /// Character progression (the same as `--mode progression`): characters start at level 1
    /// and level up from defeated enemies (capped per world), minifigures start at rank 0 and are
    /// ranked up with stars. Off: every character is at the level cap with every minifigure
    /// fully ranked.
    #[arg(long, env = "LMOXIDE_PROGRESSION")]
    pub progression: bool,
    /// Survey mode: the survey set's folder (default <data-dir>/survey) of the world editor before
    /// content records: taken into the content working copy once (its migrated.json).
    #[arg(long, env = "LMOXIDE_SURVEY_SET")]
    pub survey_set: Option<PathBuf>,
    /// Play servers: the world editor's publish queue (the survey server's var/content-queue),
    /// which admins see and decide on here (docs/WORLD-CONTENT.md).
    #[arg(long, env = "LMOXIDE_CONTENT_QUEUE")]
    pub content_queue: Option<PathBuf>,
    /// The game's content tree (docs/CONTENT.md; default: the source checkout's content/).
    #[arg(long, env = "LMOXIDE_CONTENT_DIR")]
    pub content_dir: Option<PathBuf>,
    /// How players' straight shots are aimed at the enemy they target: where it is when the
    /// shot leaves (`current`), or where it will be when the shot gets there (`lead`).
    #[arg(long, env = "LMOXIDE_SHOT_AIM", value_enum, default_value_t = projectile::Aim::Current)]
    pub shot_aim: projectile::Aim,
    /// How players' lobbed shots (thrown bombs, eggs, ...), which land where they were thrown,
    /// are aimed at the enemy they target: `lead` lands them where an enemy running along its
    /// path will be by then, `current` where it was at the throw.
    #[arg(long, env = "LMOXIDE_LOB_AIM", value_enum, default_value_t = projectile::Aim::Lead)]
    pub lob_aim: projectile::Aim,
    /// Most simultaneous connections from one IP address (per listener).
    #[arg(long, env = "LMOXIDE_MAX_CONNECTIONS_PER_IP", default_value_t = 8)]
    pub max_connections_per_ip: u32,
    /// Where the internal control interface for lmoxide-api listens. Keep it private.
    #[arg(long, env = "LMOXIDE_CONTROL_BIND", default_value = "127.0.0.1:7300")]
    pub control_bind: SocketAddr,
    /// Shared secret lmoxide-api authenticates with (at least 32 characters). The control
    /// interface is off without it.
    #[arg(long, env = "LMOXIDE_CONTROL_TOKEN", hide_env_values = true)]
    pub control_token: Option<String>,
    /// Read the control token from a file (e.g. a Docker secret).
    #[arg(long, env = "LMOXIDE_CONTROL_TOKEN_FILE", conflicts_with = "control_token")]
    pub control_token_file: Option<PathBuf>,
    /// Where to write the login public key for lmoxide-patch-server, which patches it into
    /// clients (default <data-dir>/login-public-key.txt).
    #[arg(long, env = "LMOXIDE_LOGIN_PUBLIC_KEY_FILE")]
    pub login_public_key_file: Option<PathBuf>,
    /// Use the login key and the logins (usernames, passwords) of another server's database,
    /// e.g. the play server's for a survey server next to it: clients patched for that server log
    /// in here too, with the same accounts. Characters, GM levels and bans stay this server's
    /// own. Logins are copied at startup, every minute and when someone logs in.
    #[arg(long, env = "LMOXIDE_SHARE_ACCOUNTS")]
    pub share_accounts: Option<PathBuf>,
    /// A server next to this one players can switch to in game (`!server <name>`, the dev UI),
    /// e.g. the world editor's survey server for the play server and the other way round: its
    /// name, its data directory (its database checks login cookies), and the addresses clients
    /// reach its game and chat servers at ("host:port"). The peer needs this server as its peer.
    #[arg(long, env = "LMOXIDE_PEER_NAME", requires_all = ["peer_data_dir", "peer_game_addr", "peer_chat_addr"])]
    pub peer_name: Option<String>,
    #[arg(long, env = "LMOXIDE_PEER_DATA_DIR")]
    pub peer_data_dir: Option<PathBuf>,
    #[arg(long, env = "LMOXIDE_PEER_GAME_ADDR")]
    pub peer_game_addr: Option<String>,
    #[arg(long, env = "LMOXIDE_PEER_CHAT_ADDR")]
    pub peer_chat_addr: Option<String>,
    /// Add a server-side character that walks around the start position (for testing what
    /// other players look like).
    #[arg(long, env = "LMOXIDE_BOT")]
    pub bot: bool,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Play,
    Progression,
    Survey,
}

impl Config {
    fn addr(&self, port: u16) -> SocketAddr {
        SocketAddr::new(self.bind, port)
    }
    pub fn public_addr(&self, port: u16) -> String {
        format!("{}:{}", self.public_host, port)
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,lmoxide_server=debug".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Some(cmd) => admin::run(cmd, &cli.storage),
        None => tokio::runtime::Runtime::new()?.block_on(serve(cli.serve, cli.storage)),
    }
}

async fn serve(cfg: Config, storage: Storage) -> anyhow::Result<()> {
    gamedata::init(cfg.content_dir.clone())?;
    let db = Arc::new(storage.open()?);
    if let Some(path) = &cfg.share_accounts {
        let other = lmoxide_db::Db::open(path).map_err(|e| anyhow::anyhow!("opening {}: {e}", path.display()))?;
        admin::share_login_key(&other, &db)?;
        let n = admin::share_accounts(&other, &db, None)?;
        info!(from = %path.display(), changed = n, "shared accounts");
        state::set_shared_accounts(other);
    }
    let key = admin::login_key(&db, false)?;
    let key_file = cfg.login_public_key_file.clone().unwrap_or_else(|| storage.data_dir.join("login-public-key.txt"));
    if let Some(dir) = key_file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&key_file, format!("{}\n", key.public_hex()))?;
    info!(file = %key_file.display(), "login public key written (patched into clients by lmoxide-patch-server)");
    if cfg.auto_register {
        warn!("--auto-register: anyone who can reach the server can create an account by logging in");
    }
    state::init(db, key);
    state::set_data_dir(storage.data_dir.clone());
    let mut cfg = cfg;
    if cfg.mode == Mode::Progression {
        cfg.progression = true;
    }
    if cfg.mode == Mode::Survey {
        anyhow::ensure!(!cfg.progression, "survey servers have no progression: drop --progression");
        // surveyors teleport and use the test commands
        cfg.default_gm_level = cfg.default_gm_level.max(cfg.devcmd_gm_level).max(1);
        let dir = cfg.survey_set.clone().unwrap_or_else(|| storage.data_dir.join("survey"));
        survey::init(dir);
        editor::init(&gamedata::dir(cfg.content_dir.clone()));
    } else {
        content::init(cfg.content_queue.clone());
    }
    progression::set_enabled(cfg.progression);
    let peer = cfg.peer_name.clone().map(|name| {
        let dir = cfg.peer_data_dir.clone().unwrap_or_default();
        (name, dir.join("lmoxide.db"), cfg.peer_game_addr.clone().unwrap_or_default(), cfg.peer_chat_addr.clone().unwrap_or_default())
    });
    switch::init(if cfg.mode == Mode::Survey { "editor" } else { "play" }, peer)?;
    for (shots, aim) in [(projectile::Shots::Straight, cfg.shot_aim), (projectile::Shots::Lobbed, cfg.lob_aim)] {
        anyhow::ensure!(projectile::set_player_aim(shots, aim), "aim {} isn't implemented yet", aim.name());
    }
    let cfg = Arc::new(cfg);

    let control_token = match (&cfg.control_token, &cfg.control_token_file) {
        (Some(t), _) => Some(t.trim().to_string()),
        (None, Some(f)) => Some(std::fs::read_to_string(f)?.trim().to_string()),
        (None, None) => None,
    };
    match control_token {
        Some(t) if t.len() >= control::MIN_TOKEN_LEN => {
            let bind = cfg.control_bind;
            tokio::spawn(async move {
                if let Err(e) = control::serve(bind, t).await {
                    warn!("control interface stopped: {e:#}");
                }
            });
        }
        Some(_) => anyhow::bail!("the control token must be at least {} characters", control::MIN_TOKEN_LEN),
        None => info!("no --control-token: control interface (lmoxide-api) disabled"),
    }

    let universe = tokio::spawn(conn::listen("universe", cfg.addr(cfg.universe_port), cfg.clone(), universe::handle));
    let territory = tokio::spawn(conn::listen("territory", cfg.addr(cfg.territory_port), cfg.clone(), territory::handle));
    let game = tokio::spawn(conn::listen("game", cfg.addr(cfg.game_port), cfg.clone(), game::handle));
    let chat = tokio::spawn(chat::listen(cfg.addr(cfg.chat_port), cfg.max_connections_per_ip));
    npc::start();
    buildie::start();
    jumppad::start();
    scenery::start();
    teleport::start();
    mirrors::start();
    gates::start();
    plates::start();
    pistons::start();
    tiki::start();
    ballista::start();
    if cfg.mode != Mode::Survey {
        // arenas close doors and spawn adds
        encounter::start();
    }
    if cfg.bot {
        chat::BOT_ECHO.store(true, std::sync::atomic::Ordering::Relaxed);
        tokio::spawn(game::run_bot(cfg.start_playfield));
    }

    tokio::select! {
        r = universe => r??,
        r = territory => r??,
        r = game => r??,
        r = chat => r??,
        _ = shutdown_signal() => {
            info!("shutting down: disconnecting players so their characters are saved");
            sessions::kick_all("The server is shutting down.");
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            while sessions::players_online() > 0 && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
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
