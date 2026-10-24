//! Game login cookies. The client protocol carries a 32-bit cookie from the universe agent to
//! the territory, game and chat servers; it is random and tied to one account, and a new login
//! replaces the account's older cookies. Kept in the database so a client can reconnect
//! (`LoginFromCrash`) after a server restart.

use rusqlite::{OptionalExtension, params};

use crate::{Db, Result, now};

impl Db {
    /// A new cookie for the account, replacing its older ones.
    pub fn create_game_session(&self, account_id: u32, ttl_secs: i64) -> Result<u32> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let t = now();
        tx.execute("DELETE FROM game_sessions WHERE account_id = ?1 OR expires_at <= ?2", params![account_id, t])?;
        let cookie = loop {
            let cookie = rand::random_range(1..=u32::MAX);
            let n = tx.execute(
                "INSERT OR IGNORE INTO game_sessions (cookie, account_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
                params![cookie, account_id, t, t + ttl_secs],
            )?;
            if n == 1 {
                break cookie;
            }
        };
        tx.commit()?;
        Ok(cookie)
    }

    /// The account a live cookie belongs to.
    pub fn game_session(&self, cookie: u32) -> Result<Option<u32>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT account_id FROM game_sessions WHERE cookie = ?1 AND expires_at > ?2",
                params![cookie, now()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Ends the account's game sessions (kick, ban, password change).
    pub fn end_game_sessions(&self, account_id: u32) -> Result<()> {
        self.conn().execute("DELETE FROM game_sessions WHERE account_id = ?1", [account_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account("alice", None, 0).unwrap();
        let c1 = db.create_game_session(a.id, 60).unwrap();
        assert_eq!(db.game_session(c1).unwrap(), Some(a.id));
        let c2 = db.create_game_session(a.id, 60).unwrap();
        assert_eq!(db.game_session(c1).unwrap(), None, "replaced by the new login");
        assert_eq!(db.game_session(c2).unwrap(), Some(a.id));
        db.end_game_sessions(a.id).unwrap();
        assert_eq!(db.game_session(c2).unwrap(), None);
        let c3 = db.create_game_session(a.id, -1).unwrap();
        assert_eq!(db.game_session(c3).unwrap(), None, "expired");
    }
}
