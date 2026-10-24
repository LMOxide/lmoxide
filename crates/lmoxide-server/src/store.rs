//! Saved characters, kept in the database (`lmoxide_db`). Characters are keyed by account
//! instance for now: one character per account, with the account's id.

pub use lmoxide_db::Character;

use crate::state;

pub struct Store;

impl Store {
    pub fn load(&self, character: u32) -> Option<Character> {
        state::or_log(state::db().load_character(character), "load character", None)
    }

    pub fn save(&self, character: u32, c: &Character) -> anyhow::Result<()> {
        Ok(state::db().save_character(character, character, c)?)
    }
}
