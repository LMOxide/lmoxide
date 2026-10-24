//! Saved characters, kept in the database (`lmoxide_db`). An account's first character has the
//! account's id, further ones their own (territory.rs).

pub use lmoxide_db::{Character, Defeat, Visit};

use crate::state;

pub struct Store;

impl Store {
    pub fn load(&self, character: u32) -> Option<Character> {
        state::or_log(state::db().load_character(character), "load character", None)
    }

    /// Saves an existing character (its account stays).
    pub fn save(&self, character: u32, c: &Character) -> anyhow::Result<()> {
        Ok(state::db().save_character(character, character, c)?)
    }

    /// Saves a new character of `account`.
    pub fn create(&self, character: u32, account: u32, c: &Character) -> anyhow::Result<()> {
        Ok(state::db().save_character(character, account, c)?)
    }
}
