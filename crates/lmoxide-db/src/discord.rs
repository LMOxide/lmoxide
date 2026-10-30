//! Discord accounts linked to game accounts (the Discord bot), and the one-time codes the bot
//! sends in DMs: `link` (a signed-in user links their account) and `login` (sign in to set a
//! forgotten password). Codes are `XXXX-XXXX`; only their SHA-256 is stored.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    Account, Db, Error, Result,
    accounts::{COLUMNS, account},
    now,
};

/// A game account linked to a Discord account.
#[derive(Debug, Clone, Serialize)]
pub struct DiscordLink {
    pub discord_id: String,
    pub account_id: u32,
    pub username: String,
    pub gm_level: u32,
    /// Whether a ban is in force.
    pub banned: bool,
}

/// What a one-time code is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodePurpose {
    Link,
    Login,
}

impl CodePurpose {
    fn as_str(self) -> &'static str {
        match self {
            CodePurpose::Link => "link",
            CodePurpose::Login => "login",
        }
    }
}

/// Discord user ids (snowflakes): 1-20 decimal digits.
pub fn validate_discord_id(id: &str) -> Result<()> {
    if (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(Error::Invalid("Discord ids are 1-20 digits".into()))
    }
}

/// Letters and digits that can't be confused with each other (no 0/O, 1/I/L).
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const CODE_LEN: usize = 8;

/// The code as stored: upper case, without the dash or spaces. `None`: can't be a code.
fn normalize_code(code: &str) -> Option<String> {
    let c: String = code.chars().filter(|c| *c != '-' && !c.is_whitespace()).map(|c| c.to_ascii_uppercase()).collect();
    (c.len() == CODE_LEN && c.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(c)
}

fn code_digest(normalized: &str) -> Vec<u8> {
    Sha256::digest(normalized.as_bytes()).to_vec()
}

impl Db {
    /// Links the account to a Discord account. Conflict if either is linked already.
    pub fn link_discord(&self, account_id: u32, discord_id: &str) -> Result<()> {
        validate_discord_id(discord_id)?;
        let a = self.account(account_id)?.ok_or(Error::NotFound("account"))?;
        if a.discord_id.is_some() {
            return Err(Error::Conflict("this account is already linked to a Discord account".into()));
        }
        if self.account_by_discord(discord_id)?.is_some() {
            return Err(Error::Conflict("this Discord account is already linked to a game account".into()));
        }
        self.update_account("discord_id = ?2", params![account_id, discord_id])
    }

    /// Unlinks the Discord account and revokes the keys that act as its game account; returns
    /// that account.
    pub fn unlink_discord(&self, discord_id: &str) -> Result<Account> {
        let a = self.account_by_discord(discord_id)?.ok_or(Error::NotFound("Discord link"))?;
        self.update_account("discord_id = NULL", params![a.id])?;
        self.revoke_account_keys(a.id)?;
        Ok(a)
    }

    pub fn account_by_discord(&self, discord_id: &str) -> Result<Option<Account>> {
        let sql = format!("SELECT {COLUMNS} FROM accounts WHERE discord_id = ?1");
        Ok(self.conn().query_row(&sql, [discord_id], account).optional()?)
    }

    pub fn discord_link(&self, discord_id: &str) -> Result<Option<DiscordLink>> {
        Ok(self.account_by_discord(discord_id)?.map(link))
    }

    /// Every linked account, by Discord id.
    pub fn discord_links(&self) -> Result<Vec<DiscordLink>> {
        let sql = format!("SELECT {COLUMNS} FROM accounts WHERE discord_id IS NOT NULL ORDER BY discord_id");
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], account)?;
        Ok(rows.map(|r| r.map(link)).collect::<rusqlite::Result<_>>()?)
    }

    /// A new one-time code for the Discord account, lasting `ttl_secs`; it replaces the last
    /// one for the same purpose. Returns the code (`XXXX-XXXX`) and when it expires.
    pub fn create_discord_code(&self, discord_id: &str, purpose: CodePurpose, ttl_secs: i64) -> Result<(String, i64)> {
        validate_discord_id(discord_id)?;
        let conn = self.conn();
        let t = now();
        conn.execute("DELETE FROM discord_codes WHERE expires_at <= ?1", [t])?;
        loop {
            let raw: String =
                (0..CODE_LEN).map(|_| CODE_ALPHABET[rand::random_range(0..CODE_ALPHABET.len())] as char).collect();
            let res = conn.execute(
                "INSERT OR REPLACE INTO discord_codes (discord_id, purpose, code_hash, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![discord_id, purpose.as_str(), code_digest(&raw), t, t + ttl_secs],
            );
            match res {
                Ok(_) => return Ok((format!("{}-{}", &raw[..4], &raw[4..]), t + ttl_secs)),
                // the same code live for someone else (code_hash is unique): draw again
                Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Uses up a live code for this purpose (any case, dash optional); returns its Discord id.
    pub fn take_discord_code(&self, code: &str, purpose: CodePurpose) -> Result<Option<String>> {
        let Some(code) = normalize_code(code) else { return Ok(None) };
        let conn = self.conn();
        let id: Option<String> = conn
            .query_row(
                "SELECT discord_id FROM discord_codes WHERE code_hash = ?1 AND purpose = ?2 AND expires_at > ?3",
                params![code_digest(&code), purpose.as_str(), now()],
                |r| r.get(0),
            )
            .optional()?;
        if id.is_some() {
            conn.execute("DELETE FROM discord_codes WHERE code_hash = ?1", [code_digest(&code)])?;
        }
        Ok(id)
    }
}

fn link(a: Account) -> DiscordLink {
    DiscordLink {
        banned: a.active_ban().is_some(),
        discord_id: a.discord_id.unwrap_or_default(),
        account_id: a.id,
        username: a.username,
        gm_level: a.gm_level,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account("alice", Some("password1"), 0).unwrap();
        let b = db.create_account("bob", Some("password1"), 0).unwrap();
        assert!(db.link_discord(a.id, "12x").is_err());
        db.link_discord(a.id, "165540221100752897").unwrap();
        assert!(matches!(db.link_discord(b.id, "165540221100752897"), Err(Error::Conflict(_))));
        assert!(matches!(db.link_discord(a.id, "2"), Err(Error::Conflict(_))));
        let l = db.discord_link("165540221100752897").unwrap().unwrap();
        assert_eq!((l.account_id, l.username.as_str(), l.banned), (a.id, "alice", false));
        assert_eq!(db.discord_links().unwrap().len(), 1);

        let (_, secret) = db.create_account_key(a.id, "discord-bot").unwrap();
        assert_eq!(db.unlink_discord("165540221100752897").unwrap().id, a.id);
        assert!(db.authenticate_api_key(&secret).unwrap().is_none(), "unlinking revokes its keys");
        assert!(db.discord_links().unwrap().is_empty());
        assert!(matches!(db.unlink_discord("165540221100752897"), Err(Error::NotFound(_))));
    }

    #[test]
    fn codes() {
        let db = Db::open_in_memory().unwrap();
        let (code, exp) = db.create_discord_code("42", CodePurpose::Link, 600).unwrap();
        assert!(exp > now() && code.len() == 9 && code.as_bytes()[4] == b'-');
        assert_eq!(db.take_discord_code(&code, CodePurpose::Login).unwrap(), None, "wrong purpose");
        let sloppy = code.replace('-', " ").to_lowercase();
        assert_eq!(db.take_discord_code(&sloppy, CodePurpose::Link).unwrap().as_deref(), Some("42"));
        assert_eq!(db.take_discord_code(&code, CodePurpose::Link).unwrap(), None, "used once");

        // a new code replaces the last one for the same purpose
        let (old, _) = db.create_discord_code("42", CodePurpose::Login, 600).unwrap();
        let (new, _) = db.create_discord_code("42", CodePurpose::Login, 600).unwrap();
        assert_eq!(db.take_discord_code(&old, CodePurpose::Login).unwrap(), None);
        assert_eq!(db.take_discord_code(&new, CodePurpose::Login).unwrap().as_deref(), Some("42"));

        let (expired, _) = db.create_discord_code("42", CodePurpose::Login, -1).unwrap();
        assert_eq!(db.take_discord_code(&expired, CodePurpose::Login).unwrap(), None);
        assert_eq!(db.take_discord_code("nonsense", CodePurpose::Login).unwrap(), None);
    }
}
