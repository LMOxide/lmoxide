//! LMOxide storage in one SQLite database (WAL mode), shared by the game server and the REST
//! API: accounts, characters, friendships, API keys and sessions, the audit log and settings.
//!
//! Both processes may open the same file. Calls are synchronous and short; async callers that
//! must not block (the API) run them on a blocking thread.

mod accounts;
mod api;
mod characters;
mod game_sessions;

use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub use accounts::{Account, Ban};
pub use api::{ApiKey, Scope};
pub use characters::{Character, CharacterSummary, Defeat, QuestState, Visit};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Invalid(String),
    #[error("username already taken")]
    UsernameTaken,
    #[error("password hashing failed")]
    Hash,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_progression.sql"),
    include_str!("../migrations/0003_audit_target.sql"),
    include_str!("../migrations/0004_player_ids.sql"),
    include_str!("../migrations/0005_journal.sql"),
    include_str!("../migrations/0006_purchased_upgrades.sql"),
    include_str!("../migrations/0007_collection.sql"),
    include_str!("../migrations/0008_quests.sql"),
    include_str!("../migrations/0009_character_mode.sql"),
    include_str!("../migrations/0010_session_mode.sql"),
    include_str!("../migrations/0011_story.sql"),

];

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Opens (creating if needed) the database file and applies pending migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(dir) = path.as_ref().parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let new = !path.as_ref().exists();
        let conn = Connection::open(&path)?;
        // password hashes and the login key: owner only (SQLite gives its -wal/-shm files the
        // same mode)
        #[cfg(unix)]
        if new {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    /// A private in-memory database (tests).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version = conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
        if version > MIGRATIONS.len() {
            return Err(Error::Invalid(format!(
                "database schema version {version} is newer than this build ({})",
                MIGRATIONS.len()
            )));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
            tracing::info!("database migrated to schema version {}", i + 1);
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Records an administrative action. `actor` is e.g. `api-key:discord-bot`,
    /// `account:hosttemple` or `cli`.
    pub fn audit(&self, actor: &str, action: &str, target: Option<&str>, details: Option<&str>) -> Result<()> {
        self.conn().execute(
            "INSERT INTO audit_log (at, actor, action, target, details) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![now(), actor, action, target, details],
        )?;
        Ok(())
    }

    /// [`Db::audit`] for an action on an account: the target is its username, and its id is
    /// kept too (`target_account`).
    pub fn audit_account(&self, actor: &str, action: &str, account: &Account, details: Option<&str>) -> Result<()> {
        self.conn().execute(
            "INSERT INTO audit_log (at, actor, action, target, target_account, details) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now(), actor, action, account.username, account.id, details],
        )?;
        Ok(())
    }

    /// The newest audit entries, before entry id `before` if given.
    pub fn audit_log(&self, limit: u32, before: Option<i64>) -> Result<Vec<AuditEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, at, actor, action, target, target_account, details FROM audit_log
             WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![before.unwrap_or(i64::MAX), limit], |r| {
            Ok(AuditEntry {
                id: r.get(0)?,
                at: r.get(1)?,
                actor: r.get(2)?,
                action: r.get(3)?,
                target: r.get(4)?,
                target_account: r.get(5)?,
                details: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    /// Unix time.
    pub at: i64,
    pub actor: String,
    pub action: String,
    /// For actions on an account: its username (at the time).
    pub target: Option<String>,
    /// For actions on an account: its id.
    pub target_account: Option<u32>,
    pub details: Option<String>,
}

/// Current unix time in seconds.
pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_ids_leave_the_npc_range() {
        // a schema-3 database with an account in the client's NPC range and a friendship whose
        // order flips when it moves
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for sql in &MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        let tx = conn.transaction().unwrap();
        for (id, name) in [(5_000_000, "low"), (20_000_000, "high")] {
            tx.execute("INSERT INTO accounts (id, username, gm_level, created_at) VALUES (?1, ?2, 0, 0)", params![id, name]).unwrap();
            tx.execute(
                "INSERT INTO characters (id, account_id, name, playfield, pos_x, pos_y, pos_z, created_at, saved_at) VALUES (?1, ?1, ?2, 1200, 0, 0, 0, 0, 0)",
                params![id, name],
            )
            .unwrap();
        }
        tx.execute("INSERT INTO character_team (character_id, slot, position, template) VALUES (5000000, 6, 0, 663929)", []).unwrap();
        tx.execute("INSERT INTO friendships (low, high, created_at) VALUES (5000000, 20000000, 0)", []).unwrap();
        tx.commit().unwrap();
        let db = Db::init(conn).unwrap();
        let low = db.account_by_name("low").unwrap().unwrap();
        assert_eq!(low.id, 1_005_000_000);
        let conn = db.conn();
        let team: u32 = conn.query_row("SELECT count(*) FROM character_team WHERE character_id = 1005000000", [], |r| r.get(0)).unwrap();
        let friends: (u32, u32) = conn.query_row("SELECT low, high FROM friendships", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((team, friends), (1, (20_000_000, 1_005_000_000)));
    }

    #[test]
    fn audit_targets() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account("Target", None, 0).unwrap();
        db.audit_account("cli", "account-ban", &a, Some("spam")).unwrap();
        let e = &db.audit_log(10, None).unwrap()[0];
        assert_eq!((e.target.as_deref(), e.target_account), (Some("Target"), Some(a.id)));
    }

    /// Migration 0003 gives old entries the account id and the username as target.
    #[test]
    fn audit_target_migration() {
        let mut conn = Connection::open_in_memory().unwrap();
        for (i, sql) in MIGRATIONS[..2].iter().enumerate() {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", (i + 1) as i64).unwrap();
        }
        let tx = conn.transaction().unwrap();
        tx.execute("INSERT INTO accounts (id, username, created_at) VALUES (7, 'Seven', 0)", []).unwrap();
        for (action, target) in [("account-ban", "7"), ("account-create", "seven"), ("account-delete", "gone"), ("kick", "99")] {
            tx.execute("INSERT INTO audit_log (at, actor, action, target) VALUES (0, 'api', ?1, ?2)", [action, target]).unwrap();
        }
        tx.commit().unwrap();
        let db = Db::init(conn).unwrap();
        let got: Vec<_> =
            db.audit_log(10, None).unwrap().into_iter().rev().map(|e| (e.target.unwrap(), e.target_account)).collect();
        assert_eq!(
            got,
            // (0004 then moves account 7 out of the client's NPC range)
            [("Seven".into(), Some(1_000_000_007)), ("seven".into(), Some(1_000_000_007)), ("gone".into(), None), ("99".into(), Some(99))]
        );
    }
}
