use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;

use crate::{Db, Error, Result, now};

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    /// Account instance as the client sees it (also the character instance for now).
    pub id: u32,
    pub username: String,
    /// False for accounts created without a password: they can't log in until one is set, or
    /// the server auto-registers (`--auto-register`) and the first login sets it.
    pub has_password: bool,
    pub gm_level: u32,
    /// Unix time.
    pub created_at: i64,
    pub last_login_at: Option<i64>,
    pub last_login_ip: Option<String>,
    pub ban: Option<Ban>,
    /// The linked Discord user id (a snowflake as decimal text).
    pub discord_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Ban {
    /// Unix time the ban ends; `None` is permanent.
    pub until: Option<i64>,
    pub reason: Option<String>,
}

impl Account {
    /// The ban in force now, if any.
    pub fn active_ban(&self) -> Option<&Ban> {
        self.ban.as_ref().filter(|b| b.until.is_none_or(|t| t > now()))
    }
}

pub(crate) const COLUMNS: &str =
    "id, username, password_hash IS NOT NULL, gm_level, created_at, last_login_at, last_login_ip, banned, ban_until, ban_reason, discord_id";

pub(crate) fn account(r: &Row) -> rusqlite::Result<Account> {
    let banned: bool = r.get(7)?;
    Ok(Account {
        id: r.get(0)?,
        username: r.get(1)?,
        has_password: r.get(2)?,
        gm_level: r.get(3)?,
        created_at: r.get(4)?,
        last_login_at: r.get(5)?,
        last_login_ip: r.get(6)?,
        ban: banned.then(|| Ok::<_, rusqlite::Error>(Ban { until: r.get(8)?, reason: r.get(9)? })).transpose()?,
        discord_id: r.get(10)?,
    })
}

/// Login names: 3 to 32 ASCII letters, digits, `_`, `-` or `.`.
pub fn validate_username(name: &str) -> Result<()> {
    let ok = (3..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b));
    if ok { Ok(()) } else { Err(Error::Invalid("usernames are 3-32 letters, digits, '_', '-' or '.'".into())) }
}

/// Passwords: 8 to 128 bytes.
pub fn validate_password(password: &str) -> Result<()> {
    if (8..=128).contains(&password.len()) {
        Ok(())
    } else {
        Err(Error::Invalid("passwords are 8-128 characters".into()))
    }
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::encode_b64(&rand::random::<[u8; 16]>()).map_err(|_| Error::Hash)?;
    Ok(Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|_| Error::Hash)?.to_string())
}

impl Db {
    /// Creates an account with a random id. `password` is checked with [`validate_password`].
    pub fn create_account(&self, username: &str, password: Option<&str>, gm_level: u32) -> Result<Account> {
        validate_username(username)?;
        let hash = password.map(|p| validate_password(p).and_then(|_| hash_password(p))).transpose()?;
        let conn = self.conn();
        loop {
            // above 16,000,000: the account's character has this id, and the client takes
            // lower character instances for NPCs (n3Dynel::IsNPC)
            let id = rand::random_range(16_000_001..0x7fff_ffffu32);
            let res = conn.execute(
                "INSERT INTO accounts (id, username, password_hash, gm_level, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, username, hash, gm_level, now()],
            );
            match res {
                Ok(_) => break,
                Err(rusqlite::Error::SqliteFailure(e, Some(msg)))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation && msg.contains("username") =>
                {
                    return Err(Error::UsernameTaken);
                }
                // id collision: try another
                Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {}
                Err(e) => return Err(e.into()),
            }
        }
        drop(conn);
        self.account_by_name(username)?.ok_or(Error::NotFound("account"))
    }

    pub fn account(&self, id: u32) -> Result<Option<Account>> {
        let sql = format!("SELECT {COLUMNS} FROM accounts WHERE id = ?1");
        Ok(self.conn().query_row(&sql, [id], account).optional()?)
    }

    /// Case-insensitive lookup.
    pub fn account_by_name(&self, username: &str) -> Result<Option<Account>> {
        let sql = format!("SELECT {COLUMNS} FROM accounts WHERE username = ?1");
        Ok(self.conn().query_row(&sql, [username], account).optional()?)
    }

    /// Accounts ordered by username, optionally only those whose name contains `search`.
    pub fn list_accounts(&self, search: Option<&str>, offset: u32, limit: u32) -> Result<Vec<Account>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM accounts WHERE instr(lower(username), lower(?1)) > 0
             ORDER BY username LIMIT ?2 OFFSET ?3"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![search.unwrap_or(""), limit, offset], account)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn count_accounts(&self) -> Result<u64> {
        self.count_accounts_matching(None)
    }

    /// How many accounts [`Db::list_accounts`] with this `search` finds in all.
    pub fn count_accounts_matching(&self, search: Option<&str>) -> Result<u64> {
        Ok(self.conn().query_row(
            "SELECT count(*) FROM accounts WHERE instr(lower(username), lower(?1)) > 0",
            [search.unwrap_or("")],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    /// The stored password hash (an argon2 PHC string), to copy a login to another server's
    /// database ([`Db::import_login`]).
    pub fn password_hash(&self, id: u32) -> Result<Option<String>> {
        Ok(self.conn().query_row("SELECT password_hash FROM accounts WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten())
    }

    /// Makes `username` log in here as on another server (which has it as account `id`):
    /// creates the account (with that id when it is free) or gives it that password hash. Its
    /// characters here stay its own. Whether anything changed.
    pub fn import_login(&self, id: u32, username: &str, hash: Option<&str>) -> Result<bool> {
        if let Some(a) = self.account_by_name(username)? {
            if self.password_hash(a.id)?.as_deref() == hash {
                return Ok(false);
            }
            self.update_account("password_hash = ?2", params![a.id, hash])?;
            return Ok(true);
        }
        validate_username(username)?;
        let conn = self.conn();
        let free = conn.query_row("SELECT count(*) FROM accounts WHERE id = ?1", [id], |r| r.get::<_, i64>(0))? == 0;
        let id = if free { id } else { loop {
            let id = rand::random_range(16_000_001..0x7fff_ffffu32);
            if conn.query_row("SELECT count(*) FROM accounts WHERE id = ?1", [id], |r| r.get::<_, i64>(0))? == 0 {
                break id;
            }
        } };
        conn.execute(
            "INSERT INTO accounts (id, username, password_hash, gm_level, created_at) VALUES (?1, ?2, ?3, 0, ?4)",
            params![id, username, hash, now()],
        )?;
        Ok(true)
    }

    /// Checks a password against the stored hash. False when the account has none.
    pub fn verify_password(&self, id: u32, password: &str) -> Result<bool> {
        let hash: Option<String> = self
            .conn()
            .query_row("SELECT password_hash FROM accounts WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(hash.is_some_and(|h| {
            PasswordHash::new(&h).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
        }))
    }

    /// Sets (checked with [`validate_password`]) or clears the password.
    pub fn set_password(&self, id: u32, password: Option<&str>) -> Result<()> {
        let hash = password.map(|p| validate_password(p).and_then(|_| hash_password(p))).transpose()?;
        self.update_account("password_hash = ?2", params![id, hash])?;
        // a password change ends the account's API sessions
        self.conn().execute("DELETE FROM api_sessions WHERE account_id = ?1", [id])?;
        Ok(())
    }

    pub fn set_gm_level(&self, id: u32, level: u32) -> Result<()> {
        self.update_account("gm_level = ?2", params![id, level])
    }

    pub fn ban(&self, id: u32, ban: &Ban) -> Result<()> {
        self.update_account("banned = 1, ban_until = ?2, ban_reason = ?3", params![id, ban.until, ban.reason])
    }

    pub fn unban(&self, id: u32) -> Result<()> {
        self.update_account("banned = 0, ban_until = NULL, ban_reason = NULL", params![id])
    }

    pub fn rename_account(&self, id: u32, username: &str) -> Result<()> {
        validate_username(username)?;
        match self.update_account("username = ?2", params![id, username]) {
            Err(Error::Sqlite(rusqlite::Error::SqliteFailure(e, _)))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(Error::UsernameTaken)
            }
            r => r,
        }
    }

    pub fn record_login(&self, id: u32, ip: &str) -> Result<()> {
        self.update_account("last_login_at = ?2, last_login_ip = ?3", params![id, now(), ip])
    }

    /// Deletes the account with its characters, friendships and sessions.
    pub fn delete_account(&self, id: u32) -> Result<()> {
        let n = self.conn().execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        if n == 0 { Err(Error::NotFound("account")) } else { Ok(()) }
    }

    /// `UPDATE accounts SET <set> WHERE id = ?1`, with the id as the first parameter.
    pub(crate) fn update_account(&self, set: &str, params: &[&dyn rusqlite::ToSql]) -> Result<()> {
        let n = self.conn().execute(&format!("UPDATE accounts SET {set} WHERE id = ?1"), params)?;
        if n == 0 { Err(Error::NotFound("account")) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_logins() {
        let (play, survey) = (Db::open_in_memory().unwrap(), Db::open_in_memory().unwrap());
        let a = play.create_account("player2", Some("hunter22"), 2).unwrap();
        let hash = play.password_hash(a.id).unwrap();
        assert!(survey.import_login(a.id, "player2", hash.as_deref()).unwrap());
        let b = survey.account_by_name("PLAYER2").unwrap().unwrap();
        assert_eq!((b.id, b.gm_level), (a.id, 0));
        assert!(survey.verify_password(b.id, "hunter22").unwrap());
        assert!(!survey.import_login(a.id, "player2", hash.as_deref()).unwrap());
        // a new password there follows; an id already taken here gets another
        play.set_password(a.id, Some("new password")).unwrap();
        assert!(survey.import_login(a.id, "player2", play.password_hash(a.id).unwrap().as_deref()).unwrap());
        assert!(survey.verify_password(b.id, "new password").unwrap());
        assert!(survey.import_login(a.id, "someone", None).unwrap());
        assert_ne!(survey.account_by_name("someone").unwrap().unwrap().id, a.id);
    }

    #[test]
    fn accounts() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account("Host_Temple", Some("correct horse"), 0).unwrap();
        assert!(a.has_password && a.id > 0 && a.id < 0x8000_0000);
        assert!(matches!(db.create_account("host_temple", None, 0), Err(Error::UsernameTaken)));
        assert_eq!(db.account_by_name("HOST_temple").unwrap().unwrap().id, a.id);
        assert!(db.verify_password(a.id, "correct horse").unwrap());
        assert!(!db.verify_password(a.id, "wrong").unwrap());
        db.set_password(a.id, None).unwrap();
        assert!(!db.verify_password(a.id, "correct horse").unwrap());
        assert!(db.create_account("x", None, 0).is_err());
        assert!(db.create_account("short", Some("1234"), 0).is_err());

        db.ban(a.id, &Ban { until: None, reason: Some("test".into()) }).unwrap();
        assert!(db.account(a.id).unwrap().unwrap().active_ban().is_some());
        db.ban(a.id, &Ban { until: Some(now() - 1), reason: None }).unwrap();
        assert!(db.account(a.id).unwrap().unwrap().active_ban().is_none());
        db.unban(a.id).unwrap();
        assert!(db.account(a.id).unwrap().unwrap().ban.is_none());

        assert_eq!(db.list_accounts(Some("TEMP"), 0, 10).unwrap().len(), 1);
        db.create_account("other", None, 0).unwrap();
        assert_eq!(db.count_accounts_matching(Some("TEMP")).unwrap(), 1);
        assert_eq!(db.count_accounts().unwrap(), 2);
        db.delete_account(a.id).unwrap();
        assert!(db.account(a.id).unwrap().is_none());
    }
}
