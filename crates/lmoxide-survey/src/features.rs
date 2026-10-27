//! Features: what a record (placement, trigger wall, note, script) is for. Every record names
//! one by id; the ids nest with `/`, the first part being the category:
//!
//! - `live`: mimicking live as it was (the default, and what most surveying is);
//! - `feature/...`: new content, e.g. `feature/christmas-event/2026`;
//! - `other/...`: anything else (fixes of our own, experiments).
//!
//! Servers decide per category, feature or author what to apply (docs/WORLD-CONTENT.md). A
//! feature can be copied with all its records ([`copy`]), so next year's event starts as this
//! year's did.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{FORMAT, ids, set::Set};

/// The default feature: mimicking live.
pub const LIVE: &str = "live";

/// The categories (the first part of a feature id), with their titles.
pub const CATEGORIES: &[(&str, &str)] = &[(LIVE, "Mimic live"), ("feature", "New content"), ("other", "Other")];

pub fn live() -> String {
    LIVE.into()
}

pub fn is_live(f: &String) -> bool {
    f == LIVE
}

/// A feature's category (`live`, `feature`, `other`): its id's first part.
pub fn category(id: &str) -> &str {
    id.split('/').next().unwrap_or(id)
}

/// Whether `id` is `ancestor` or nested under it.
pub fn within(id: &str, ancestor: &str) -> bool {
    id == ancestor || id.strip_prefix(ancestor).is_some_and(|rest| rest.starts_with('/'))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    /// `feature/christmas-event/2026`: lowercase letters, digits and `-`, parts joined by `/`.
    pub id: String,
    /// "2026" (the last part's title; the path's titles come from the parents).
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// The feature it was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copied_from: Option<String>,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub updated: u64,
}

/// `features.json` in a set's folder: the features beyond the categories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeaturesFile {
    pub format: u32,
    #[serde(default)]
    pub features: Vec<Feature>,
}

impl Default for FeaturesFile {
    fn default() -> Self {
        FeaturesFile { format: FORMAT, features: Vec::new() }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("feature ids are parts of lowercase letters, digits and - joined by /, starting with live, feature or other: {0}")]
    BadId(String),
    #[error("feature {0} exists already")]
    Exists(String),
    #[error("no feature {0}")]
    Missing(String),
}

/// A part of an id from a title: "Christmas Event" -> "christmas-event".
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Whether `id` is a well-formed feature id.
pub fn valid(id: &str) -> bool {
    let mut parts = id.split('/');
    let first_ok = parts.next().is_some_and(|c| CATEGORIES.iter().any(|(k, _)| *k == c));
    first_ok && id.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
}

impl FeaturesFile {
    pub fn get(&self, id: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    /// Whether `id` is a category or a known feature.
    pub fn known(&self, id: &str) -> bool {
        CATEGORIES.iter().any(|(k, _)| *k == id) || self.get(id).is_some()
    }

    /// "New content > Christmas Event > 2026".
    pub fn path_title(&self, id: &str) -> String {
        let mut titles = Vec::new();
        let mut prefix = String::new();
        for part in id.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let title = CATEGORIES
                .iter()
                .find(|(k, _)| *k == prefix)
                .map(|(_, t)| t.to_string())
                .or_else(|| self.get(&prefix).map(|f| f.title.clone()).filter(|t| !t.is_empty()))
                .unwrap_or_else(|| part.to_string());
            titles.push(title);
        }
        titles.join(" > ")
    }

    /// Adds a feature (and any missing parents, titled by their id parts).
    pub fn add(&mut self, mut f: Feature) -> Result<&Feature, Error> {
        if !valid(&f.id) || CATEGORIES.iter().any(|(k, _)| *k == f.id) {
            return Err(Error::BadId(f.id));
        }
        if self.get(&f.id).is_some() {
            return Err(Error::Exists(f.id));
        }
        let id = f.id.clone();
        let parts: Vec<&str> = id.split('/').collect();
        for n in 2..parts.len() {
            let parent = parts[..n].join("/");
            if self.get(&parent).is_none() {
                self.features.push(Feature {
                    id: parent,
                    title: parts[n - 1].to_string(),
                    comment: String::new(),
                    copied_from: None,
                    author: f.author.clone(),
                    updated: f.updated,
                });
            }
        }
        if f.title.is_empty() {
            f.title = parts.last().unwrap_or(&"").to_string();
        }
        self.features.push(f);
        self.features.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(self.get(&id).expect("just added"))
    }
}

/// Copies feature `from` (and what is nested under it) to `to`, records included: every
/// placement, trigger wall, note and script of it gets a copy with a new id in the new feature
/// (scripts' waves keep their ids, and placements in a copied script's wave point at the copy).
/// Returns how many records were copied.
pub fn copy(set: &mut Set, from: &str, to: Feature, author: &str) -> Result<usize, Error> {
    if !set.features.known(from) {
        return Err(Error::Missing(from.into()));
    }
    let new_root = to.id.clone();
    let renamed = |id: &str| -> String { format!("{new_root}{}", &id[from.len()..]) };
    // the features nested under `from` come along
    let nested: Vec<Feature> = set.features.features.iter().filter(|f| within(&f.id, from) && f.id != from).cloned().collect();
    set.features.add(Feature { copied_from: Some(from.into()), ..to })?;
    for f in nested {
        let id = renamed(&f.id);
        if set.features.get(&id).is_none() {
            set.features.add(Feature { id, copied_from: Some(f.id.clone()), author: author.into(), updated: ids::now(), ..f })?;
        }
    }
    let now = ids::now();
    let mut n = 0;
    // scripts and notes first: the copied scripts' new ids, for the placements in their waves
    let mut scripts = HashMap::new();
    for file in set.notes.values_mut() {
        let copies: Vec<_> = file
            .scripts
            .iter()
            .filter(|s| within(&s.feature, from))
            .map(|s| {
                let id = ids::new_id("s", author);
                scripts.insert(s.id.clone(), id.clone());
                crate::Script { id, feature: renamed(&s.feature), author: author.into(), updated: now, ..s.clone() }
            })
            .collect();
        let notes: Vec<_> = file
            .notes
            .iter()
            .filter(|x| within(&x.feature, from))
            .map(|x| crate::Note { id: ids::new_id("n", author), feature: renamed(&x.feature), author: author.into(), updated: now, ..x.clone() })
            .collect();
        n += copies.len() + notes.len();
        file.scripts.extend(copies);
        file.notes.extend(notes);
    }
    for file in set.placements.values_mut() {
        let placements: Vec<_> = file
            .placements
            .iter()
            .filter(|p| within(&p.feature, from))
            .map(|p| {
                let wave = p.wave.clone().map(|w| crate::WaveRef { script: scripts.get(&w.script).cloned().unwrap_or(w.script), wave: w.wave });
                crate::Placement { id: ids::new_id("p", author), feature: renamed(&p.feature), wave, author: author.into(), updated: now, ..p.clone() }
            })
            .collect();
        let walls: Vec<_> = file
            .triggers
            .iter()
            .filter(|w| within(&w.feature, from))
            .map(|w| crate::TriggerWall { id: ids::new_id("t", author), feature: renamed(&w.feature), author: author.into(), updated: now, ..w.clone() })
            .collect();
        n += placements.len() + walls.len();
        file.placements.extend(placements);
        file.triggers.extend(walls);
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(slug("Christmas Event!"), "christmas-event");
        assert!(valid("feature/christmas-event/2026"));
        assert!(valid("live"));
        assert!(!valid("christmas/2026") && !valid("feature//x") && !valid("feature/Big"));
        assert_eq!(category("feature/christmas-event/2026"), "feature");
        assert!(within("feature/xmas/2026", "feature/xmas") && !within("feature/xmas2", "feature/xmas"));
    }

    #[test]
    fn nesting_and_titles() {
        let mut f = FeaturesFile::default();
        let feat = |id: &str, title: &str| Feature { id: id.into(), title: title.into(), comment: String::new(), copied_from: None, author: "a".into(), updated: 1 };
        f.add(feat("feature/christmas-event/2026", "2026")).unwrap();
        assert!(f.get("feature/christmas-event").is_some(), "parent made");
        assert_eq!(f.add(feat("feature/christmas-event/2026", "")), Err(Error::Exists("feature/christmas-event/2026".into())));
        assert!(matches!(f.add(feat("live", "")), Err(Error::BadId(_))));
        f.features.iter_mut().find(|x| x.id == "feature/christmas-event").unwrap().title = "Christmas Event".into();
        assert_eq!(f.path_title("feature/christmas-event/2026"), "New content > Christmas Event > 2026");
        assert_eq!(f.path_title("live"), "Mimic live");
    }

    #[test]
    fn copying_a_feature() {
        use crate::{Kind, NotesFile, Placement, PlacementsFile, Script, Wave, WaveRef};
        let mut set = Set::default();
        let feat = |id: &str| Feature { id: id.into(), title: String::new(), comment: String::new(), copied_from: None, author: "a".into(), updated: 1 };
        set.features.add(feat("feature/xmas/2026")).unwrap();
        let script = Script {
            id: "s-1".into(),
            name: "Snowmen".into(),
            area: None,
            reference: None,
            comment: String::new(),
            waves: vec![Wave { id: "w-1".into(), name: String::new(), trigger: None, comment: String::new() }],
            status: Default::default(),
            tags: Vec::new(),
            feature: "feature/xmas/2026".into(),
            author: "a".into(),
            updated: 1,
        };
        set.notes.insert(1200, NotesFile { scripts: vec![script], ..NotesFile::new(1200) });
        let p = |id: &str, feature: &str, wave: Option<WaveRef>| Placement {
            id: id.into(),
            kind: Kind::Enemy,
            name: "Snowman".into(),
            monster_data: Some(1),
            template: None,
            level: Some(3),
            pos: [0.0; 3],
            heading: 0.0,
            replaces: None,
            removed: false,
            wave,
            patrol: Vec::new(),
            comment: String::new(),
            tags: Vec::new(),
            feature: feature.into(),
            author: "a".into(),
            updated: 1,
        };
        let in_wave = Some(WaveRef { script: "s-1".into(), wave: "w-1".into() });
        set.placements.insert(1200, PlacementsFile { placements: vec![p("p-1", "feature/xmas/2026", in_wave), p("p-2", LIVE, None)], ..PlacementsFile::new(1200) });
        let n = copy(&mut set, "feature/xmas/2026", feat("feature/xmas/2027"), "b").unwrap();
        assert_eq!(n, 2, "the script and its placement, not the live one");
        let copied = set.placements[&1200].placements.iter().find(|x| x.feature == "feature/xmas/2027").unwrap();
        let new_script = set.notes[&1200].scripts.iter().find(|x| x.feature == "feature/xmas/2027").unwrap();
        assert_eq!(copied.wave.as_ref().unwrap().script, new_script.id);
        assert_eq!(set.features.get("feature/xmas/2027").unwrap().copied_from.as_deref(), Some("feature/xmas/2026"));
    }
}
