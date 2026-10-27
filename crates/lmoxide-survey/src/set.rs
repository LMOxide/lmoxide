//! A survey set on disk: a folder with `set.json` and, per playfield,
//! `<playfield>.placements.json` and `<playfield>.notes.json`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{NotesFile, PlacementsFile};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
    #[error("{path}: is for playfield {found}, its name says {named}")]
    Playfield { path: PathBuf, named: u32, found: u32 },
}

/// `set.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Set {
    pub info: SetInfo,
    pub placements: BTreeMap<u32, PlacementsFile>,
    pub notes: BTreeMap<u32, NotesFile>,
}

impl Set {
    /// Reads a set's folder (a missing folder is an empty set). Other files are ignored.
    pub fn load(dir: &Path) -> Result<Set, Error> {
        let mut set = Set::default();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(set),
            Err(source) => return Err(Error::Io { path: dir.into(), source }),
        };
        for entry in entries {
            let path = entry.map_err(|source| Error::Io { path: dir.into(), source })?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if name == "set.json" {
                set.info = read(&path)?;
            } else if let Some(pf) = name.strip_suffix(".placements.json").and_then(|n| n.parse().ok()) {
                let f: PlacementsFile = read(&path)?;
                check_playfield(&path, pf, f.playfield)?;
                set.placements.insert(pf, f);
            } else if let Some(pf) = name.strip_suffix(".notes.json").and_then(|n| n.parse().ok()) {
                let f: NotesFile = read(&path)?;
                check_playfield(&path, pf, f.playfield)?;
                set.notes.insert(pf, f);
            }
        }
        Ok(set)
    }

    /// Writes every file of the set (records sorted by id, so diffs stay small). Each file is
    /// written to a temporary file first and renamed over the old one.
    pub fn save(&self, dir: &Path) -> Result<(), Error> {
        std::fs::create_dir_all(dir).map_err(|source| Error::Io { path: dir.into(), source })?;
        write(&dir.join("set.json"), &self.info)?;
        for (pf, f) in &self.placements {
            let mut f = f.clone();
            f.placements.sort_by(|a, b| a.id.cmp(&b.id));
            write(&dir.join(format!("{pf}.placements.json")), &f)?;
        }
        for (pf, f) in &self.notes {
            let mut f = f.clone();
            f.notes.sort_by(|a, b| a.id.cmp(&b.id));
            f.scripts.sort_by(|a, b| a.id.cmp(&b.id));
            write(&dir.join(format!("{pf}.notes.json")), &f)?;
        }
        Ok(())
    }

    /// The placements file of a playfield, made if missing.
    pub fn placements_mut(&mut self, playfield: u32) -> &mut PlacementsFile {
        self.placements.entry(playfield).or_insert_with(|| PlacementsFile::new(playfield))
    }

    /// The notes file of a playfield, made if missing.
    pub fn notes_mut(&mut self, playfield: u32) -> &mut NotesFile {
        self.notes.entry(playfield).or_insert_with(|| NotesFile::new(playfield))
    }
}

fn check_playfield(path: &Path, named: u32, found: u32) -> Result<(), Error> {
    if named == found { Ok(()) } else { Err(Error::Playfield { path: path.into(), named, found }) }
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, Error> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io { path: path.into(), source })?;
    serde_json::from_str(&text).map_err(|source| Error::Json { path: path.into(), source })
}

fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    let mut text = serde_json::to_string_pretty(value).map_err(|source| Error::Json { path: path.into(), source })?;
    text.push('\n');
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|source| Error::Io { path: tmp.clone(), source })?;
    std::fs::rename(&tmp, path).map_err(|source| Error::Io { path: path.into(), source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Area, Kind, Note, NoteKind, Placement, Review};

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(crate::ids::new_id("survey-test", "set"));
        let mut set = Set { info: SetInfo { name: "Pirate World".into(), ..Default::default() }, ..Default::default() };
        set.placements_mut(1200).placements.push(Placement {
            id: "p-b".into(),
            kind: Kind::Smashable,
            name: "Crate".into(),
            monster_data: Some(152417),
            template: Some(153171),
            level: None,
            pos: [1.0, 2.0, 3.0],
            heading: 0.5,
            replaces: None,
            removed: false,
            wave: None,
            patrol: Vec::new(),
            comment: String::new(),
            author: "a".into(),
            updated: 5,
        });
        set.notes_mut(1200).notes.push(Note {
            id: "n-a".into(),
            kind: NoteKind::Dialog,
            area: Area::Point([0.0; 3]),
            reference: None,
            comment: "the parrot talks here".into(),
            status: Review::Unreviewed,
            author: "a".into(),
            updated: 5,
        });
        set.save(&dir).unwrap();
        assert_eq!(Set::load(&dir).unwrap(), set);
        assert_eq!(Set::load(&dir.join("missing")).unwrap(), Set::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
