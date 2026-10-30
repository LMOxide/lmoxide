//! Process-wide state set up once by `main`: the database, the login key and the maintenance
//! switch.

use std::sync::{Arc, Mutex, OnceLock};

use lmoxide_db::Db;
use lmoxide_net::login::LoginKey;

static DB: OnceLock<Arc<Db>> = OnceLock::new();
static LOGIN_KEY: OnceLock<LoginKey> = OnceLock::new();
static STARTED_AT: OnceLock<i64> = OnceLock::new();
static DATA_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
/// The server's data directory (backups of deleted characters live there).
pub fn data_dir() -> &'static std::path::Path {
    DATA_DIR.get().map_or(std::path::Path::new("."), |p| p.as_path())
}

pub fn set_data_dir(dir: std::path::PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// `Some(message)` while in maintenance: only accounts with a GM level may log in.
/// The GM levels that unlock things (`--tester-gm-level`, `--devcmd-gm-level`,
/// `--admin-gm-level`, `--client-gm-level`), for code without the config at hand.
#[derive(Debug, Clone, Copy)]
pub struct Levels {
    pub tester: u32,
    pub devcmd: u32,
    pub admin: u32,
    pub client: u32,
}

static LEVELS: OnceLock<Levels> = OnceLock::new();

pub fn levels() -> Levels {
    // tests run without serve(): the defaults
    *LEVELS.get_or_init(|| Levels { tester: 1, devcmd: 2, admin: 4, client: 2 })
}

pub fn set_levels(levels: Levels) {
    let _ = LEVELS.set(levels);
}

static MAINTENANCE: Mutex<Option<Option<String>>> = Mutex::new(None);

pub fn init(db: Arc<Db>, key: LoginKey) {
    let _ = DB.set(db);
    let _ = LOGIN_KEY.set(key);
    let _ = STARTED_AT.set(lmoxide_db::now());
}

pub fn db() -> &'static Db {
    DB.get().expect("state::init not called")
}

pub fn login_key() -> &'static LoginKey {
    LOGIN_KEY.get().expect("state::init not called")
}

pub fn started_at() -> i64 {
    STARTED_AT.get().copied().unwrap_or(0)
}

/// The maintenance message if maintenance is on (`Some(None)`: on, no message).
pub fn maintenance() -> Option<Option<String>> {
    MAINTENANCE.lock().unwrap().clone()
}

pub fn set_maintenance(on: Option<Option<String>>) {
    *MAINTENANCE.lock().unwrap() = on;
}

/// Logs a database error and carries on with `default`: the game keeps running when a save
/// or lookup fails.
pub fn or_log<T>(r: lmoxide_db::Result<T>, what: &str, default: T) -> T {
    r.unwrap_or_else(|e| {
        tracing::error!("database: {what}: {e}");
        default
    })
}

/// Initializes with an in-memory database (once per test binary).
#[cfg(test)]
pub fn init_for_tests() {
    if DB.get().is_none() {
        init(Arc::new(Db::open_in_memory().unwrap()), LoginKey::generate());
    }
}
