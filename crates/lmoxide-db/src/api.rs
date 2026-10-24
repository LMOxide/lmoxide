//! REST API credentials: API keys (bots, scripts) and login sessions (web UI). Secrets are
//! random 256-bit tokens handed out once; only their SHA-256 is stored.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Db, Error, Result, now};

/// What a credential may do. Each scope includes the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Server status, players online, account and character lookups.
    Read,
    /// Kick, ban and unban players, broadcast messages.
    Moderate,
    /// Everything: accounts, passwords, GM levels, API keys, the audit log.
    Admin,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Moderate => "moderate",
            Scope::Admin => "admin",
        }
    }
}

impl std::str::FromStr for Scope {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "read" => Ok(Scope::Read),
            "moderate" => Ok(Scope::Moderate),
            "admin" => Ok(Scope::Admin),
            _ => Err(Error::Invalid(format!("unknown scope {s:?} (read, moderate, admin)"))),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    pub scope: Scope,
    /// Unix time.
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked: bool,
}

const KEY_PREFIX: &str = "lmox_";

fn new_secret() -> String {
    let bytes: [u8; 32] = rand::random();
    format!("{KEY_PREFIX}{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn digest(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

impl Db {
    /// Creates an API key; returns it with its secret, which is not stored and can't be shown
    /// again.
    pub fn create_api_key(&self, name: &str, scope: Scope) -> Result<(ApiKey, String)> {
        if name.trim().is_empty() || name.len() > 64 {
            return Err(Error::Invalid("API key names are 1-64 characters".into()));
        }
        let secret = new_secret();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO api_keys (name, key_hash, scopes, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, digest(&secret), scope.as_str(), now()],
        )?;
        let id = conn.last_insert_rowid();
        drop(conn);
        Ok((self.api_key(id)?.ok_or(Error::NotFound("API key"))?, secret))
    }

    pub fn api_key(&self, id: i64) -> Result<Option<ApiKey>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, name, scopes, created_at, last_used_at, revoked FROM api_keys WHERE id = ?1",
                [id],
                api_key_row,
            )
            .optional()?)
    }

    pub fn list_api_keys(&self) -> Result<Vec<ApiKey>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, name, scopes, created_at, last_used_at, revoked FROM api_keys ORDER BY id")?;
        let rows = stmt.query_map([], api_key_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The live (not revoked) key with this secret; marks it used.
    pub fn authenticate_api_key(&self, secret: &str) -> Result<Option<ApiKey>> {
        if !secret.starts_with(KEY_PREFIX) {
            return Ok(None);
        }
        let conn = self.conn();
        let key = conn
            .query_row(
                "SELECT id, name, scopes, created_at, last_used_at, revoked FROM api_keys
                 WHERE key_hash = ?1 AND revoked = 0",
                [digest(secret)],
                api_key_row,
            )
            .optional()?;
        if let Some(k) = &key {
            conn.execute("UPDATE api_keys SET last_used_at = ?2 WHERE id = ?1", params![k.id, now()])?;
        }
        Ok(key)
    }

    pub fn revoke_api_key(&self, id: i64) -> Result<()> {
        let n = self.conn().execute("UPDATE api_keys SET revoked = 1 WHERE id = ?1", [id])?;
        if n == 0 { Err(Error::NotFound("API key")) } else { Ok(()) }
    }

    /// Starts a login session for the account; returns its token.
    pub fn create_session(&self, account_id: u32, ttl_secs: i64) -> Result<String> {
        let token = new_secret();
        let conn = self.conn();
        let t = now();
        conn.execute("DELETE FROM api_sessions WHERE expires_at <= ?1", [t])?;
        conn.execute(
            "INSERT INTO api_sessions (token_hash, account_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![digest(&token), account_id, t, t + ttl_secs],
        )?;
        Ok(token)
    }

    /// The account of a live session token, with the session's expiry.
    pub fn session_account(&self, token: &str) -> Result<Option<(u32, i64)>> {
        if !token.starts_with(KEY_PREFIX) {
            return Ok(None);
        }
        Ok(self
            .conn()
            .query_row(
                "SELECT account_id, expires_at FROM api_sessions WHERE token_hash = ?1 AND expires_at > ?2",
                params![digest(token), now()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn delete_session(&self, token: &str) -> Result<()> {
        self.conn().execute("DELETE FROM api_sessions WHERE token_hash = ?1", [digest(token)])?;
        Ok(())
    }
}

fn api_key_row(r: &rusqlite::Row) -> rusqlite::Result<ApiKey> {
    let scope: String = r.get(2)?;
    Ok(ApiKey {
        id: r.get(0)?,
        name: r.get(1)?,
        scope: scope.parse().unwrap_or(Scope::Read),
        created_at: r.get(3)?,
        last_used_at: r.get(4)?,
        revoked: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_sessions() {
        let db = Db::open_in_memory().unwrap();
        let (key, secret) = db.create_api_key("bot", Scope::Moderate).unwrap();
        assert!(secret.starts_with("lmox_") && secret.len() == 69);
        assert_eq!(db.authenticate_api_key(&secret).unwrap().unwrap().id, key.id);
        assert!(db.authenticate_api_key("lmox_nope").unwrap().is_none());
        db.revoke_api_key(key.id).unwrap();
        assert!(db.authenticate_api_key(&secret).unwrap().is_none());
        assert!(Scope::Admin > Scope::Moderate && Scope::Moderate > Scope::Read);

        let a = db.create_account("alice", Some("password1"), 0).unwrap();
        let token = db.create_session(a.id, 60).unwrap();
        assert_eq!(db.session_account(&token).unwrap().unwrap().0, a.id);
        db.set_password(a.id, Some("password2")).unwrap();
        assert!(db.session_account(&token).unwrap().is_none(), "password change ends sessions");
        let token = db.create_session(a.id, -1).unwrap();
        assert!(db.session_account(&token).unwrap().is_none(), "expired");
    }
}
