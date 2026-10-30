//! Account friends: one account asks another, and once the other accepts, every character of
//! each is friends with every character of the other (`friendships` rows, so the game's friend
//! list and its removal work as for any friend). Characters made later become friends with the
//! other account's characters when first saved ([`Db::save_character`]). Removing the account
//! friendship removes the friendships between their characters that it made (`account_friend`;
//! not those they had before or made in game). The Discord bot sends requests
//! to linked targets by DM ([`Db::unnotified_friend_requests`]).

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::{Db, Error, Result, now};

/// How an account stands with another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FriendState {
    /// Accepted: their characters are friends.
    Friends,
    /// The other account asked; waiting for this one's answer.
    Incoming,
    /// This account asked; waiting for the other's answer.
    Outgoing,
}

/// Another account as one account sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountFriend {
    pub account_id: u32,
    pub username: String,
    pub state: FriendState,
    /// Unix time it was asked.
    pub requested_at: i64,
    /// Unix time it was accepted (friends).
    pub accepted_at: Option<i64>,
}

/// A request waiting for its target, who is linked to Discord and hasn't been sent it yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FriendRequestNotice {
    pub requester_id: u32,
    pub requester_username: String,
    pub target_id: u32,
    pub target_username: String,
    /// The target's Discord user id.
    pub discord_id: String,
    pub requested_at: i64,
}

/// What a request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requested {
    /// Waiting for the other account's answer.
    Sent,
    /// The other account had asked this one already: now friends, with these character
    /// friendships (low, high) new.
    Accepted(Vec<(u32, u32)>),
}

/// The character pairs (low, high) of two accounts.
fn character_pairs(conn: &Connection, a: u32, b: u32) -> rusqlite::Result<Vec<(u32, u32)>> {
    let mut stmt = conn.prepare(
        "SELECT min(x.id, y.id), max(x.id, y.id) FROM characters x, characters y
         WHERE x.account_id = ?1 AND y.account_id = ?2 ORDER BY 1, 2",
    )?;
    let rows = stmt.query_map(params![a, b], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// A new character `id` of `account` becomes friends with the characters of the account's
/// friends (inside [`Db::save_character`]'s transaction).
pub(crate) fn befriend_new_character(conn: &Connection, id: u32, account: u32, at: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO friendships (low, high, created_at, account_friend)
         SELECT min(?1, c.id), max(?1, c.id), ?3, 1 FROM characters c JOIN account_friends f
           ON (f.requester = ?2 AND c.account_id = f.target) OR (f.target = ?2 AND c.account_id = f.requester)
         WHERE f.accepted = 1 AND c.id <> ?1",
        params![id, account, at],
    )?;
    Ok(())
}

impl Db {
    /// The accounts `account` is friends with or has a request with, by username.
    pub fn account_friends(&self, account: u32) -> Result<Vec<AccountFriend>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT a.id, a.username, f.accepted, f.requester = ?1, f.created_at, f.accepted_at
             FROM account_friends f JOIN accounts a ON a.id = CASE WHEN f.requester = ?1 THEN f.target ELSE f.requester END
             WHERE f.requester = ?1 OR f.target = ?1 ORDER BY a.username",
        )?;
        let rows = stmt.query_map([account], |r| {
            let (accepted, mine): (bool, bool) = (r.get(2)?, r.get(3)?);
            Ok(AccountFriend {
                account_id: r.get(0)?,
                username: r.get(1)?,
                state: match (accepted, mine) {
                    (true, _) => FriendState::Friends,
                    (false, true) => FriendState::Outgoing,
                    (false, false) => FriendState::Incoming,
                },
                requested_at: r.get(4)?,
                accepted_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How `account` stands with `other` (none: neither asked).
    pub fn account_friend(&self, account: u32, other: u32) -> Result<Option<AccountFriend>> {
        Ok(self.account_friends(account)?.into_iter().find(|f| f.account_id == other))
    }

    /// `from` asks `to`. If `to` had asked `from` already, that is accepted instead.
    pub fn request_account_friend(&self, from: u32, to: u32) -> Result<Requested> {
        if from == to {
            return Err(Error::Invalid("an account can't befriend itself".into()));
        }
        if self.account(to)?.is_none() {
            return Err(Error::NotFound("account"));
        }
        match self.account_friend(from, to)?.map(|f| f.state) {
            Some(FriendState::Friends) => Err(Error::Conflict("already friends".into())),
            Some(FriendState::Outgoing) => Err(Error::Conflict("already asked".into())),
            Some(FriendState::Incoming) => Ok(Requested::Accepted(self.accept_account_friend(from, to)?)),
            None => {
                self.conn().execute(
                    "INSERT INTO account_friends (requester, target, created_at) VALUES (?1, ?2, ?3)",
                    params![from, to, now()],
                )?;
                Ok(Requested::Sent)
            }
        }
    }

    /// `target` accepts `requester`'s request: every character of each becomes friends with
    /// every character of the other. Returns the character friendships (low, high) that are new.
    pub fn accept_account_friend(&self, target: u32, requester: u32) -> Result<Vec<(u32, u32)>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let t = now();
        let n = tx.execute(
            "UPDATE account_friends SET accepted = 1, accepted_at = ?3 WHERE requester = ?1 AND target = ?2 AND accepted = 0",
            params![requester, target, t],
        )?;
        if n == 0 {
            return Err(Error::NotFound("friend request"));
        }
        let mut added = Vec::new();
        {
            let mut insert = tx.prepare("INSERT OR IGNORE INTO friendships (low, high, created_at, account_friend) VALUES (?1, ?2, ?3, 1)")?;
            for (low, high) in character_pairs(&tx, requester, target)? {
                if insert.execute(params![low, high, t])? > 0 {
                    added.push((low, high));
                }
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// `target` turns down `requester`'s request (it is deleted; they may ask again).
    pub fn reject_account_friend(&self, target: u32, requester: u32) -> Result<()> {
        let n = self.conn().execute(
            "DELETE FROM account_friends WHERE requester = ?1 AND target = ?2 AND accepted = 0",
            params![requester, target],
        )?;
        if n == 0 { Err(Error::NotFound("friend request")) } else { Ok(()) }
    }

    /// Ends `account`'s friendship with `other` (or a request either way). For a friendship,
    /// every friendship between their characters goes too; returns those (low, high).
    pub fn remove_account_friend(&self, account: u32, other: u32) -> Result<Vec<(u32, u32)>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let accepted: Option<bool> = tx
            .query_row(
                "DELETE FROM account_friends WHERE (requester = ?1 AND target = ?2) OR (requester = ?2 AND target = ?1)
                 RETURNING accepted",
                params![account, other],
                |r| r.get(0),
            )
            .optional()?;
        let Some(accepted) = accepted else {
            return Err(Error::NotFound("account friend"));
        };
        let mut removed = Vec::new();
        if accepted {
            let mut delete = tx.prepare("DELETE FROM friendships WHERE low = ?1 AND high = ?2 AND account_friend = 1")?;
            for (low, high) in character_pairs(&tx, account, other)? {
                if delete.execute(params![low, high])? > 0 {
                    removed.push((low, high));
                }
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Requests waiting for a target linked to Discord that the bot hasn't sent yet, oldest first.
    pub fn unnotified_friend_requests(&self) -> Result<Vec<FriendRequestNotice>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT r.id, r.username, t.id, t.username, t.discord_id, f.created_at
             FROM account_friends f JOIN accounts r ON r.id = f.requester JOIN accounts t ON t.id = f.target
             WHERE f.accepted = 0 AND f.notified_at IS NULL AND t.discord_id IS NOT NULL
             ORDER BY f.created_at, r.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(FriendRequestNotice {
                requester_id: r.get(0)?,
                requester_username: r.get(1)?,
                target_id: r.get(2)?,
                target_username: r.get(3)?,
                discord_id: r.get(4)?,
                requested_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The bot sent `target` the request from `requester`; false if there is no such request.
    pub fn mark_friend_request_notified(&self, requester: u32, target: u32) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE account_friends SET notified_at = ?3 WHERE requester = ?1 AND target = ?2 AND accepted = 0",
            params![requester, target, now()],
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Character;

    fn character(name: &str) -> Character {
        Character {
            name: name.into(),
            mode: "progression".into(),
            playfield: 1200,
            pos: [0.0; 3],
            team: Vec::new(),
            stars: 0,
            upgrades: Default::default(),
            tokens: Default::default(),
            xp: 0,
            level: 1,
            minifigure_levels: Default::default(),
            purchased_upgrades: Default::default(),
            tags: Default::default(),
            counters: Default::default(),
            defeats: Default::default(),
            visits: Default::default(),
            finds: Default::default(),
            smashed: Default::default(),
            collection: None,
            quests: Default::default(),
            story: Default::default(),
            speed: 1.0,
        }
    }

    fn sorted(mut v: Vec<u32>) -> Vec<u32> {
        v.sort();
        v
    }

    #[test]
    fn requests_accept_and_remove() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account("alice", None, 0).unwrap();
        let b = db.create_account("bob", None, 0).unwrap();
        let c = db.create_account("carol", None, 0).unwrap();
        db.save_character(a.id, a.id, &character("alice")).unwrap();
        let a2 = db.new_character_id().unwrap();
        db.save_character(a2, a.id, &character("alice2")).unwrap();
        db.save_character(b.id, b.id, &character("bob")).unwrap();

        assert!(matches!(db.request_account_friend(a.id, a.id), Err(Error::Invalid(_))));
        assert!(matches!(db.request_account_friend(a.id, 1), Err(Error::NotFound(_))));
        assert_eq!(db.request_account_friend(a.id, b.id).unwrap(), Requested::Sent);
        assert!(matches!(db.request_account_friend(a.id, b.id), Err(Error::Conflict(_))));
        assert_eq!(db.account_friend(a.id, b.id).unwrap().unwrap().state, FriendState::Outgoing);
        assert_eq!(db.account_friend(b.id, a.id).unwrap().unwrap().state, FriendState::Incoming);
        assert!(db.friends_of(b.id).unwrap().is_empty(), "nothing until accepted");

        // the bot's DMs: only targets linked to Discord, once
        assert!(db.unnotified_friend_requests().unwrap().is_empty());
        db.link_discord(b.id, "100000000000000002").unwrap();
        let n = db.unnotified_friend_requests().unwrap();
        assert_eq!((n.len(), n[0].requester_username.as_str(), n[0].discord_id.as_str()), (1, "alice", "100000000000000002"));
        assert!(db.mark_friend_request_notified(a.id, b.id).unwrap());
        assert!(db.unnotified_friend_requests().unwrap().is_empty());

        // accepting befriends every pair of characters (alice and bob were friends already)
        assert!(db.add_friendship(a.id, b.id).unwrap());
        assert!(matches!(db.accept_account_friend(a.id, b.id), Err(Error::NotFound(_))), "only the target accepts");
        let added = db.accept_account_friend(b.id, a.id).unwrap();
        assert_eq!(added, vec![(a2.min(b.id), a2.max(b.id))]);
        assert_eq!(sorted(db.friends_of(b.id).unwrap()), sorted(vec![a.id, a2]));
        assert_eq!(db.account_friend(a.id, b.id).unwrap().unwrap().state, FriendState::Friends);
        assert!(matches!(db.request_account_friend(b.id, a.id), Err(Error::Conflict(_))));

        // a character made later too (not when saved again)
        let b2 = db.new_character_id().unwrap();
        db.save_character(b2, b.id, &character("bob2")).unwrap();
        db.save_character(b2, b.id, &character("bob2")).unwrap();
        assert_eq!(sorted(db.friends_of(b2).unwrap()), sorted(vec![a.id, a2]));
        assert!(db.friends_of(c.id).unwrap().is_empty());
        // befriended in game as well: theirs from now on
        assert!(!db.add_friendship(b2, a2).unwrap());

        // asking someone who asked you accepts
        db.save_character(c.id, c.id, &character("carol")).unwrap();
        assert_eq!(db.request_account_friend(c.id, a.id).unwrap(), Requested::Sent);
        let Requested::Accepted(pairs) = db.request_account_friend(a.id, c.id).unwrap() else { panic!("not accepted") };
        assert_eq!(pairs.len(), 2);
        assert_eq!(sorted(db.friends_of(c.id).unwrap()), sorted(vec![a.id, a2]));

        // removing takes the characters' friendships it made along, not one they had before or
        // made in game; a rejected request is gone
        assert_eq!(db.remove_account_friend(a.id, b.id).unwrap().len(), 2);
        assert_eq!(sorted(db.friends_of(b.id).unwrap()), vec![a.id]);
        assert_eq!(db.friends_of(b2).unwrap(), vec![a2]);
        assert!(db.account_friend(a.id, b.id).unwrap().is_none());
        assert!(matches!(db.remove_account_friend(a.id, b.id), Err(Error::NotFound(_))));
        db.request_account_friend(b.id, c.id).unwrap();
        db.reject_account_friend(c.id, b.id).unwrap();
        assert!(db.account_friend(c.id, b.id).unwrap().is_none());
        assert_eq!(db.account_friends(a.id).unwrap().len(), 1);
    }
}
