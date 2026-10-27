//! A play server's approved content as a history (docs/WORLD-CONTENT.md): a baseline set plus
//! the approved changes, one file each, in order. Each change keeps every record it touched as it
//! was before and after, so any change can be undone and redone later (the approved set is built
//! again from the baseline and the changes that apply) and previewed: applied for a while, then
//! kept or sent back to the queue.
//!
//! On disk (`<dir>` = `world-content/`): `baseline/` (a set folder, written once) and
//! `changes/<seq>.json` (an `Entry`, numbered from 1).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    Note, Placement, Script, SmashRule, TriggerWall,
    review::{Change, ChangeKey, RecordKind},
    set::{self, Set},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Approved: part of the approved set.
    Applied,
    /// Undone: not applied, and its versions stay out of the queue (as rejected ones) until the
    /// records change again; redo applies it again.
    Undone,
    /// Applied until `preview_until` (or until kept or reverted).
    Preview,
    /// A preview that ended: not applied, its records back in the queue.
    PreviewEnded,
}

impl Status {
    pub fn applies(self) -> bool {
        matches!(self, Status::Applied | Status::Preview)
    }
}

/// A record of any kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum Record {
    Placement(Placement),
    Trigger(TriggerWall),
    Note(Note),
    Script(Script),
    SmashRule(SmashRule),
}

impl Record {
    pub fn tags(&self) -> &[String] {
        match self {
            Record::Placement(x) => &x.tags,
            Record::Trigger(x) => &x.tags,
            Record::Note(x) => &x.tags,
            Record::Script(x) => &x.tags,
            Record::SmashRule(x) => &x.tags,
        }
    }
}

/// One record a change touched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    pub kind: RecordKind,
    pub id: String,
    /// What it is ("Crate", "Trigger wall", ...) and what changed, as the queue said.
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    /// The version taken (the record's `updated`), to keep it out of the queue once undone.
    pub updated: u64,
    /// The approved record before and after (`None`: absent).
    pub before: Option<Record>,
    pub after: Option<Record>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    /// unix seconds
    pub at: u64,
    /// Who approved it: an admin's username, or "rules".
    pub by: String,
    #[serde(flatten)]
    pub key: ChangeKey,
    pub summary: String,
    pub status: Status,
    /// Previews: unix seconds when it ends by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_until: Option<u64>,
    /// Who last undid, redid, kept or reverted it, and when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_action: Option<(String, u64)>,
    pub edits: Vec<Edit>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Set(#[from] set::Error),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
}

pub struct Ledger {
    dir: PathBuf,
    pub baseline: Set,
    pub entries: Vec<Entry>,
}

impl Ledger {
    /// A history of nothing but `baseline` (when `dir` couldn't be read; nothing is saved over it
    /// until a change is approved).
    pub fn empty(dir: &Path, baseline: Set) -> Ledger {
        Ledger { dir: dir.into(), baseline, entries: Vec::new() }
    }

    /// Reads `dir`; the first time, the approved set in `approved` (the copy before histories)
    /// becomes the baseline.
    pub fn open(dir: &Path, approved: &Path) -> Result<Ledger, Error> {
        let base_dir = dir.join("baseline");
        if !base_dir.exists() {
            Set::load(approved)?.save(&base_dir)?;
        }
        let baseline = Set::load(&base_dir)?;
        let mut entries = Vec::new();
        let changes = dir.join("changes");
        if let Ok(rd) = std::fs::read_dir(&changes) {
            for e in rd {
                let path = e.map_err(|source| Error::Io { path: changes.clone(), source })?.path();
                if path.extension().is_some_and(|x| x == "json") {
                    let text = std::fs::read_to_string(&path).map_err(|source| Error::Io { path: path.clone(), source })?;
                    entries.push(serde_json::from_str::<Entry>(&text).map_err(|source| Error::Json { path: path.clone(), source })?);
                }
            }
        }
        entries.sort_by_key(|e| e.seq);
        Ok(Ledger { dir: dir.into(), baseline, entries })
    }

    /// The approved set: the baseline with the changes that apply, in order.
    pub fn build(&self) -> Set {
        let mut set = self.baseline.clone();
        for e in self.entries.iter().filter(|e| e.status.applies()) {
            for edit in &e.edits {
                put(&mut set, e.key.playfield, edit.kind, &edit.id, edit.after.clone());
            }
        }
        set
    }

    /// Records `change` (only the records `only` names) taken from `working` into `approved`
    /// (the set as built now), as a new entry; applies it to `approved`.
    pub fn record(&mut self, approved: &mut Set, working: &Set, change: &Change, only: Option<&[String]>, by: &str, status: Status, now: u64, preview_until: Option<u64>) -> Result<&Entry, Error> {
        let pf = change.key.playfield;
        let mut edits = Vec::new();
        for item in change.items.iter().filter(|i| only.is_none_or(|o| o.contains(&i.id))) {
            let before = get(approved, pf, item.kind, &item.id);
            let after = get(working, pf, item.kind, &item.id);
            put(approved, pf, item.kind, &item.id, after.clone());
            edits.push(Edit { kind: item.kind, id: item.id.clone(), name: item.name.clone(), details: item.details.clone(), updated: item.updated, before, after });
        }
        approved.features = working.features.clone();
        let seq = self.entries.last().map_or(1, |e| e.seq + 1);
        let entry = Entry { seq, at: now, by: by.into(), key: change.key.clone(), summary: change.summary.clone(), status, preview_until, last_action: None, edits };
        self.save(&entry)?;
        self.entries.push(entry);
        Ok(self.entries.last().expect("just pushed"))
    }

    /// Sets an entry's status (undo, redo, keep, revert); `None` if there is no such entry.
    pub fn set_status(&mut self, seq: u64, status: Status, by: &str, now: u64) -> Result<Option<&Entry>, Error> {
        let Some(i) = self.entries.iter().position(|e| e.seq == seq) else { return Ok(None) };
        let e = &mut self.entries[i];
        e.status = status;
        if status != Status::Preview {
            e.preview_until = None;
        }
        e.last_action = Some((by.into(), now));
        let e = e.clone();
        self.save(&e)?;
        Ok(Some(&self.entries[i]))
    }

    /// Record versions undone changes took (kept out of the queue): (id, updated).
    pub fn undone_versions(&self) -> impl Iterator<Item = (&str, u64)> {
        self.entries.iter().filter(|e| e.status == Status::Undone).flat_map(|e| e.edits.iter().map(|x| (x.id.as_str(), x.updated)))
    }

    /// Previews past their time.
    pub fn expired_previews(&self, now: u64) -> Vec<u64> {
        self.entries.iter().filter(|e| e.status == Status::Preview && e.preview_until.is_some_and(|t| t <= now)).map(|e| e.seq).collect()
    }

    fn save(&self, e: &Entry) -> Result<(), Error> {
        let dir = self.dir.join("changes");
        std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir.clone(), source })?;
        let path = dir.join(format!("{:06}.json", e.seq));
        let tmp = path.with_extension("json.tmp");
        let mut text = serde_json::to_string_pretty(e).map_err(|source| Error::Json { path: path.clone(), source })?;
        text.push('\n');
        std::fs::write(&tmp, text).map_err(|source| Error::Io { path: tmp.clone(), source })?;
        std::fs::rename(&tmp, &path).map_err(|source| Error::Io { path, source })
    }
}

/// A record of a set, by playfield, kind and id (smash rules belong to no playfield).
pub fn get(set: &Set, pf: u32, kind: RecordKind, id: &str) -> Option<Record> {
    match kind {
        RecordKind::Placement => set.placements.get(&pf)?.placements.iter().find(|x| x.id == id).cloned().map(Record::Placement),
        RecordKind::Trigger => set.placements.get(&pf)?.triggers.iter().find(|x| x.id == id).cloned().map(Record::Trigger),
        RecordKind::Note => set.notes.get(&pf)?.notes.iter().find(|x| x.id == id).cloned().map(Record::Note),
        RecordKind::Script => set.notes.get(&pf)?.scripts.iter().find(|x| x.id == id).cloned().map(Record::Script),
        RecordKind::SmashRule => set.smash_rule(id).cloned().map(Record::SmashRule),
    }
}

/// Replaces (or with `None` removes) a record of a set.
pub fn put(set: &mut Set, pf: u32, kind: RecordKind, id: &str, record: Option<Record>) {
    match kind {
        RecordKind::Placement => {
            let list = &mut set.placements_mut(pf).placements;
            list.retain(|x| x.id != id);
            list.extend(record.and_then(|r| if let Record::Placement(x) = r { Some(x) } else { None }));
        }
        RecordKind::Trigger => {
            let list = &mut set.placements_mut(pf).triggers;
            list.retain(|x| x.id != id);
            list.extend(record.and_then(|r| if let Record::Trigger(x) = r { Some(x) } else { None }));
        }
        RecordKind::Note => {
            let list = &mut set.notes_mut(pf).notes;
            list.retain(|x| x.id != id);
            list.extend(record.and_then(|r| if let Record::Note(x) = r { Some(x) } else { None }));
        }
        RecordKind::Script => {
            let list = &mut set.notes_mut(pf).scripts;
            list.retain(|x| x.id != id);
            list.extend(record.and_then(|r| if let Record::Script(x) = r { Some(x) } else { None }));
        }
        RecordKind::SmashRule => {
            set.smash_rules.retain(|x| x.id != id);
            set.smash_rules.extend(record.and_then(|r| if let Record::SmashRule(x) = r { Some(x) } else { None }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, PlacementsFile, review};

    fn crate_at(id: &str, x: f32, updated: u64) -> Placement {
        Placement {
            id: id.into(),
            kind: Kind::Smashable,
            name: "Crate".into(),
            monster_data: Some(1),
            template: Some(2),
            level: None,
            pos: [x, 0.0, 0.0],
            heading: 0.0,
            replaces: None,
            removed: false,
            wave: None,
            patrol: Vec::new(),
            comment: String::new(),
            tags: Vec::new(),
            feature: "live".into(),
            author: "ed".into(),
            updated,
        }
    }

    fn with(placements: Vec<Placement>) -> Set {
        let mut s = Set::default();
        s.placements.insert(1200, PlacementsFile { placements, ..PlacementsFile::new(1200) });
        s
    }

    fn xs(set: &Set) -> Vec<(String, f32)> {
        let mut v: Vec<_> = set.placements.get(&1200).map_or(vec![], |f| f.placements.iter().map(|p| (p.id.clone(), p.pos[0])).collect());
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    #[test]
    fn undo_redo_and_previews() {
        let dir = std::env::temp_dir().join(format!("lmoxide-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // baseline: a at 1
        with(vec![crate_at("a", 1.0, 1)]).save(&dir.join("approved")).unwrap();
        let mut ledger = Ledger::open(&dir, &dir.join("approved")).unwrap();
        let mut approved = ledger.build();
        // change 1 moves a to 2 and adds b; change 2 moves a to 3
        let working = with(vec![crate_at("a", 2.0, 2), crate_at("b", 5.0, 2)]);
        let c = review::changes(&working, &approved).remove(0);
        ledger.record(&mut approved, &working, &c, None, "admin", Status::Applied, 10, None).unwrap();
        let working = with(vec![crate_at("a", 3.0, 3), crate_at("b", 5.0, 2)]);
        let c = review::changes(&working, &approved).remove(0);
        ledger.record(&mut approved, &working, &c, None, "admin", Status::Applied, 11, None).unwrap();
        assert_eq!(xs(&ledger.build()), xs(&approved));
        assert_eq!(xs(&approved), [("a".into(), 3.0), ("b".into(), 5.0)]);
        // undoing change 1 removes b; a keeps change 2's position
        ledger.set_status(1, Status::Undone, "admin", 12).unwrap();
        assert_eq!(xs(&ledger.build()), [("a".into(), 3.0)]);
        assert!(ledger.undone_versions().any(|v| v == ("b", 2)));
        // undoing change 2 too: back to the baseline
        ledger.set_status(2, Status::Undone, "admin", 13).unwrap();
        assert_eq!(xs(&ledger.build()), [("a".into(), 1.0)]);
        // the history survives a restart
        let mut ledger = Ledger::open(&dir, &dir.join("approved")).unwrap();
        ledger.set_status(1, Status::Applied, "admin", 14).unwrap();
        assert_eq!(xs(&ledger.build()), [("a".into(), 2.0), ("b".into(), 5.0)]);
        // a preview applies until its time
        let mut approved = ledger.build();
        let working = with(vec![crate_at("a", 2.0, 2), crate_at("b", 9.0, 4)]);
        let c = review::changes(&working, &approved).remove(0);
        let seq = ledger.record(&mut approved, &working, &c, None, "admin", Status::Preview, 20, Some(100)).unwrap().seq;
        assert_eq!(xs(&ledger.build())[1], ("b".into(), 9.0));
        assert!(ledger.expired_previews(99).is_empty());
        assert_eq!(ledger.expired_previews(100), [seq]);
        ledger.set_status(seq, Status::PreviewEnded, "time", 100).unwrap();
        assert_eq!(xs(&ledger.build())[1], ("b".into(), 5.0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
