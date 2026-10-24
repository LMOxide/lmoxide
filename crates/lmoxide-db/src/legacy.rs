//! Import of the JSON files the server used before the database: `<dir>/characters/<id>.json`
//! (one per account, named by the account instance) and `<dir>/friends.json`.
//!
//! Each character becomes an account with the same id (so friendships and the client's ids
//! stay valid), named after the character (the old server used the login name), without a
//! password: set one (`lmoxide-server account passwd`), or let the first login set it with
//! `--auto-register`.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use rusqlite::params;
use serde::Deserialize;
use tracing::{info, warn};

use crate::{Character, Db, Result, accounts::validate_username, now};

#[derive(Debug, Default)]
pub struct ImportReport {
    pub accounts: usize,
    pub friendships: usize,
    /// Characters whose account id already existed (left untouched).
    pub skipped: Vec<u32>,
    /// (id, username) of accounts not named after their character (invalid or taken name).
    pub renamed: Vec<(u32, String)>,
}

#[derive(Deserialize)]
struct FriendsFile {
    pairs: BTreeSet<(u32, u32)>,
    #[serde(default)]
    #[allow(dead_code)]
    names: BTreeMap<u32, String>,
}

pub fn import(db: &Db, dir: &Path) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    let mut files: Vec<_> = std::fs::read_dir(dir.join("characters"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for path in files {
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else {
            warn!(path = %path.display(), "not a character file, skipped");
            continue;
        };
        let character: Character = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        if db.account(id)?.is_some() {
            report.skipped.push(id);
            continue;
        }
        let mut username = character.name.clone();
        if validate_username(&username).is_err() || db.account_by_name(&username)?.is_some() {
            username = format!("player{id}");
            report.renamed.push((id, username.clone()));
        }
        db.conn().execute(
            "INSERT INTO accounts (id, username, password_hash, gm_level, created_at) VALUES (?1, ?2, NULL, 0, ?3)",
            params![id, username, now()],
        )?;
        db.save_character(id, id, &character)?;
        info!(id, %username, "imported character {}", character.name);
        report.accounts += 1;
    }
    let friends = dir.join("friends.json");
    if friends.exists() {
        let file: FriendsFile = serde_json::from_str(&std::fs::read_to_string(friends)?)?;
        for (a, b) in file.pairs {
            if db.character_exists(a)? && db.character_exists(b)? && db.add_friendship(a, b)? {
                report.friendships += 1;
            }
        }
    }
    db.audit(
        "cli",
        "import-json",
        Some(&dir.display().to_string()),
        Some(&format!("{} accounts, {} friendships", report.accounts, report.friendships)),
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_characters_and_friends() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("characters")).unwrap();
        std::fs::write(
            dir.path().join("characters/881199774.json"),
            r#"{"name":"friend","playfield":1200,"pos":[1,2,3],"team":[[0,5]],"stars":7}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("characters/1974518773.json"),
            r#"{"name":"bad name!","playfield":4600,"pos":[0,0,0],"team":[],"upgrades":{"5":3},"tokens":{"9":[4,1]}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("friends.json"), r#"{"pairs":[[881199774,1974518773]],"names":{}}"#).unwrap();
        let db = Db::open_in_memory().unwrap();
        let r = import(&db, dir.path()).unwrap();
        assert_eq!((r.accounts, r.friendships), (2, 1));
        assert_eq!(r.renamed, vec![(1974518773, "player1974518773".to_string())]);
        let a = db.account(881199774).unwrap().unwrap();
        assert!(a.username == "friend" && !a.has_password);
        assert_eq!(db.load_character(1974518773).unwrap().unwrap().tokens[&9], (4, 1));
        assert_eq!(db.friends_of(881199774).unwrap(), vec![1974518773]);
        // a second run changes nothing
        assert_eq!(import(&db, dir.path()).unwrap().skipped.len(), 2);
    }
}
