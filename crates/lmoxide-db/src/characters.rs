use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::{Db, Error, Result, now};

/// A saved character, as the game server keeps it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Character {
    pub name: String,
    pub playfield: u32,
    pub pos: [f32; 3],
    /// Minifigure team: (inventory 101 position, item template). The first entry is active.
    pub team: Vec<(u32, u32)>,
    /// Minifigure XP (the star counter).
    #[serde(default)]
    pub stars: u64,
    /// Active upgrade bitmask per minifigure template (unset: the game's default).
    #[serde(default)]
    pub upgrades: BTreeMap<u32, u32>,
    /// Other token counters (minifigure parts): token -> (gained, spent), as UpdateTokenAmount
    /// sends them.
    #[serde(default)]
    pub tokens: BTreeMap<u32, (u64, u64)>,
    /// Player XP (stat 52).
    #[serde(default)]
    pub xp: u64,
    /// Player level (stat 54); 0 = never set.
    #[serde(default)]
    pub level: u32,
    /// Minifigure levels by item template (unset: the server's default).
    #[serde(default)]
    pub minifigure_levels: BTreeMap<u32, u32>,
    /// Upgrades bought in the Item Shop by item template: a bit per upgrade position (unset:
    /// none, or for older saves the tiers of the minifigure's level).
    #[serde(default)]
    pub purchased_upgrades: BTreeMap<u32, u32>,
    /// Earned achievement tags -> unix time.
    #[serde(default)]
    pub tags: BTreeMap<u32, i64>,
    /// Achievement counters: stat -> (minifigure template, 0 for the player) -> value.
    #[serde(default)]
    pub counters: BTreeMap<u32, BTreeMap<u32, u64>>,
    /// Defeated enemies, bosses, champions and smashables by name.
    #[serde(default)]
    pub defeats: BTreeMap<String, Defeat>,
    /// Playfields entered.
    #[serde(default)]
    pub visits: BTreeMap<u32, Visit>,
    /// Collectibles and places found ("<playfield>:<world object name>") -> unix time.
    #[serde(default)]
    pub finds: BTreeMap<String, i64>,
}

/// How often the character defeated something of one name.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Defeat {
    /// enemy, champion, boss or smashable
    pub kind: String,
    pub count: u64,
    /// Unix times of the first and the last.
    pub first_at: i64,
    pub last_at: i64,
}

/// How often the character entered a playfield.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Visit {
    pub count: u64,
    /// Unix times of the first and the last.
    pub first_at: i64,
    pub last_at: i64,
}

/// A character row without its collections, for listings.
#[derive(Debug, Clone, Serialize)]
pub struct CharacterSummary {
    pub id: u32,
    pub account_id: u32,
    pub name: String,
    pub playfield: u32,
    pub stars: u64,
    /// Unix time of the last save.
    pub saved_at: i64,
}

impl Db {
    pub fn load_character(&self, id: u32) -> Result<Option<Character>> {
        let conn = self.conn();
        let Some(mut c) = conn
            .query_row(
                "SELECT name, playfield, pos_x, pos_y, pos_z, stars, xp, level FROM characters WHERE id = ?1",
                [id],
                |r| {
                    Ok(Character {
                        name: r.get(0)?,
                        playfield: r.get(1)?,
                        pos: [r.get(2)?, r.get(3)?, r.get(4)?],
                        team: Vec::new(),
                        stars: r.get::<_, i64>(5)? as u64,
                        upgrades: BTreeMap::new(),
                        tokens: BTreeMap::new(),
                        xp: r.get::<_, i64>(6)? as u64,
                        level: r.get(7)?,
                        minifigure_levels: BTreeMap::new(),
                        purchased_upgrades: BTreeMap::new(),
                        tags: BTreeMap::new(),
                        counters: BTreeMap::new(),
                        defeats: BTreeMap::new(),
                        visits: BTreeMap::new(),
                        finds: BTreeMap::new(),
                    })
                },
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut stmt = conn.prepare("SELECT position, template FROM character_team WHERE character_id = ?1 ORDER BY slot")?;
        c.team = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT template, mask FROM character_upgrades WHERE character_id = ?1")?;
        c.upgrades = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT token, gained, spent FROM character_tokens WHERE character_id = ?1")?;
        c.tokens = stmt
            .query_map([id], |r| Ok((r.get(0)?, (r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as u64))))?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT template, level, purchased FROM character_minifigures WHERE character_id = ?1")?;
        for row in stmt.query_map([id], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?, r.get::<_, u32>(2)?)))? {
            let (template, level, purchased) = row?;
            c.minifigure_levels.insert(template, level);
            if purchased != 0 {
                c.purchased_upgrades.insert(template, purchased);
            }
        }
        let mut stmt = conn.prepare("SELECT tag, achieved_at FROM character_tags WHERE character_id = ?1")?;
        c.tags = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT stat, template, value FROM character_counters WHERE character_id = ?1")?;
        for row in stmt.query_map([id], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?, r.get::<_, i64>(2)? as u64)))? {
            let (stat, template, value) = row?;
            c.counters.entry(stat).or_default().insert(template, value);
        }
        let mut stmt = conn.prepare("SELECT name, kind, count, first_at, last_at FROM character_defeats WHERE character_id = ?1")?;
        c.defeats = stmt
            .query_map([id], |r| {
                Ok((r.get(0)?, Defeat { kind: r.get(1)?, count: r.get::<_, i64>(2)? as u64, first_at: r.get(3)?, last_at: r.get(4)? }))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT playfield, count, first_at, last_at FROM character_visits WHERE character_id = ?1")?;
        c.visits = stmt
            .query_map([id], |r| Ok((r.get(0)?, Visit { count: r.get::<_, i64>(1)? as u64, first_at: r.get(2)?, last_at: r.get(3)? })))?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT find, found_at FROM character_finds WHERE character_id = ?1")?;
        c.finds = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(Some(c))
    }

    /// Creates or replaces character `id` of `account_id`.
    pub fn save_character(&self, id: u32, account_id: u32, c: &Character) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let t = now();
        tx.execute(
            "INSERT INTO characters (id, account_id, name, playfield, pos_x, pos_y, pos_z, stars, xp, level, created_at, saved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, playfield = excluded.playfield,
               pos_x = excluded.pos_x, pos_y = excluded.pos_y, pos_z = excluded.pos_z,
               stars = excluded.stars, xp = excluded.xp, level = excluded.level, saved_at = excluded.saved_at",
            params![id, account_id, c.name, c.playfield, c.pos[0], c.pos[1], c.pos[2], c.stars as i64, c.xp as i64, c.level, t],
        )?;
        tx.execute("DELETE FROM character_team WHERE character_id = ?1", [id])?;
        tx.execute("DELETE FROM character_upgrades WHERE character_id = ?1", [id])?;
        tx.execute("DELETE FROM character_tokens WHERE character_id = ?1", [id])?;
        tx.execute("DELETE FROM character_minifigures WHERE character_id = ?1", [id])?;
        tx.execute("DELETE FROM character_tags WHERE character_id = ?1", [id])?;
        tx.execute("DELETE FROM character_counters WHERE character_id = ?1", [id])?;
        tx.execute("DELETE FROM character_defeats WHERE character_id = ?1", [id])?;
        tx.execute("DELETE FROM character_visits WHERE character_id = ?1", [id])?;
        tx.execute("DELETE FROM character_finds WHERE character_id = ?1", [id])?;
        {
            let mut team = tx.prepare("INSERT INTO character_team (character_id, slot, position, template) VALUES (?1, ?2, ?3, ?4)")?;
            for (slot, (position, template)) in c.team.iter().enumerate() {
                team.execute(params![id, slot as i64, position, template])?;
            }
            let mut up = tx.prepare("INSERT INTO character_upgrades (character_id, template, mask) VALUES (?1, ?2, ?3)")?;
            for (template, mask) in &c.upgrades {
                up.execute(params![id, template, mask])?;
            }
            let mut tok =
                tx.prepare("INSERT INTO character_tokens (character_id, token, gained, spent) VALUES (?1, ?2, ?3, ?4)")?;
            for (token, (gained, spent)) in &c.tokens {
                tok.execute(params![id, token, *gained as i64, *spent as i64])?;
            }
            let mut fig =
                tx.prepare("INSERT INTO character_minifigures (character_id, template, level, purchased) VALUES (?1, ?2, ?3, ?4)")?;
            let templates: std::collections::BTreeSet<u32> =
                c.minifigure_levels.keys().chain(c.purchased_upgrades.keys()).copied().collect();
            for template in templates {
                let level = c.minifigure_levels.get(&template).copied().unwrap_or(0);
                let purchased = c.purchased_upgrades.get(&template).copied().unwrap_or(0);
                fig.execute(params![id, template, level, purchased])?;
            }
            let mut tag = tx.prepare("INSERT INTO character_tags (character_id, tag, achieved_at) VALUES (?1, ?2, ?3)")?;
            for (t, at) in &c.tags {
                tag.execute(params![id, t, at])?;
            }
            let mut counter =
                tx.prepare("INSERT INTO character_counters (character_id, stat, template, value) VALUES (?1, ?2, ?3, ?4)")?;
            for (stat, values) in &c.counters {
                for (template, value) in values {
                    counter.execute(params![id, stat, template, *value as i64])?;
                }
            }
            let mut defeat = tx.prepare(
                "INSERT INTO character_defeats (character_id, name, kind, count, first_at, last_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (name, d) in &c.defeats {
                defeat.execute(params![id, name, d.kind, d.count as i64, d.first_at, d.last_at])?;
            }
            let mut visit =
                tx.prepare("INSERT INTO character_visits (character_id, playfield, count, first_at, last_at) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for (playfield, v) in &c.visits {
                visit.execute(params![id, playfield, v.count as i64, v.first_at, v.last_at])?;
            }
            let mut find = tx.prepare("INSERT INTO character_finds (character_id, find, found_at) VALUES (?1, ?2, ?3)")?;
            for (f, at) in &c.finds {
                find.execute(params![id, f, at])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn character_exists(&self, id: u32) -> Result<bool> {
        Ok(self.conn().query_row("SELECT count(*) FROM characters WHERE id = ?1", [id], |r| r.get::<_, i64>(0))? > 0)
    }

    /// The account a character belongs to.
    pub fn character_account(&self, id: u32) -> Result<Option<u32>> {
        Ok(self.conn().query_row("SELECT account_id FROM characters WHERE id = ?1", [id], |r| r.get(0)).optional()?)
    }

    /// An unused id for an account's further character (its first has the account's id): above
    /// 16,000,000 like the accounts (the client keeps lower instances for NPCs), clear of every
    /// account and character id.
    pub fn new_character_id(&self) -> Result<u32> {
        let conn = self.conn();
        loop {
            let id = rand::random_range(16_000_001..0x7fff_ffffu32);
            let used: i64 = conn.query_row(
                "SELECT (SELECT count(*) FROM characters WHERE id = ?1) + (SELECT count(*) FROM accounts WHERE id = ?1)",
                [id],
                |r| r.get(0),
            )?;
            if used == 0 {
                return Ok(id);
            }
        }
    }

    pub fn character_name(&self, id: u32) -> Result<Option<String>> {
        Ok(self.conn().query_row("SELECT name FROM characters WHERE id = ?1", [id], |r| r.get(0)).optional()?)
    }

    pub fn characters_of(&self, account_id: u32) -> Result<Vec<CharacterSummary>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, account_id, name, playfield, stars, saved_at FROM characters WHERE account_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([account_id], |r| {
            Ok(CharacterSummary {
                id: r.get(0)?,
                account_id: r.get(1)?,
                name: r.get(2)?,
                playfield: r.get(3)?,
                stars: r.get::<_, i64>(4)? as u64,
                saved_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn friends_of(&self, character: u32) -> Result<Vec<u32>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT high FROM friendships WHERE low = ?1 UNION SELECT low FROM friendships WHERE high = ?1",
        )?;
        let rows = stmt.query_map([character], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds a friendship; false if it already existed.
    pub fn add_friendship(&self, a: u32, b: u32) -> Result<bool> {
        if a == b {
            return Err(Error::Invalid("a character can't befriend itself".into()));
        }
        let n = self.conn().execute(
            "INSERT OR IGNORE INTO friendships (low, high, created_at) VALUES (?1, ?2, ?3)",
            params![a.min(b), a.max(b), now()],
        )?;
        Ok(n > 0)
    }

    /// Removes a friendship; false if there was none.
    pub fn remove_friendship(&self, a: u32, b: u32) -> Result<bool> {
        let n = self.conn().execute("DELETE FROM friendships WHERE low = ?1 AND high = ?2", params![a.min(b), a.max(b)])?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_roundtrip_and_friends() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account("alice", None, 0).unwrap();
        let b = db.create_account("bob", None, 0).unwrap();
        let mut c = Character {
            name: "alice".into(),
            playfield: 1200,
            pos: [1.0, 2.5, -3.0],
            team: vec![(0, 100), (1, 200)],
            stars: 1234,
            upgrades: [(100, 7)].into(),
            tokens: [(5, (10, 3))].into(),
            xp: 4321,
            level: 7,
            minifigure_levels: [(100, 3), (200, 0)].into(),
            purchased_upgrades: [(200, 0b1011)].into(),
            tags: [(385, 1_700_000_000)].into(),
            counters: [(3_000_116, [(0, 12)].into()), (3_000_112, [(100, 4), (200, 1)].into())].into(),
            defeats: [("Skeleton".into(), Defeat { kind: "enemy".into(), count: 3, first_at: 1, last_at: 2 })].into(),
            visits: [(4200, Visit { count: 2, first_at: 1, last_at: 2 })].into(),
            finds: [("4210:collectible_dreadlegs_treasure".into(), 3)].into(),
        };
        assert!(db.load_character(a.id).unwrap().is_none());
        db.save_character(a.id, a.id, &c).unwrap();
        assert_eq!(db.load_character(a.id).unwrap().unwrap(), c);
        c.team.pop();
        c.stars = 5;
        db.save_character(a.id, a.id, &c).unwrap();
        assert_eq!(db.load_character(a.id).unwrap().unwrap(), c);

        db.save_character(b.id, b.id, &Character { name: "bob".into(), ..c.clone() }).unwrap();
        assert!(db.add_friendship(b.id, a.id).unwrap());
        assert!(!db.add_friendship(a.id, b.id).unwrap());
        assert_eq!(db.friends_of(a.id).unwrap(), vec![b.id]);
        assert_eq!(db.friends_of(b.id).unwrap(), vec![a.id]);
        assert!(db.remove_friendship(a.id, b.id).unwrap());
        assert!(db.friends_of(a.id).unwrap().is_empty());

        db.delete_account(a.id).unwrap();
        assert!(db.load_character(a.id).unwrap().is_none());
    }
}
