//! The world editor's working copy of the content tree (docs/WORLD-EDITOR-CONTENT.md): records
//! as written (not resolved), read once and edited one at a time. Every save merges with what
//! was saved meanwhile ([`crate::merge`]), checks the record and its references, raises `rev`
//! and rewrites the record's file in canonical form.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::load::{AnyRecord, ROOT_FILES, feature_active, merge, typed, walk, world_folder};
use crate::merge::{Conflict, merge3};
use crate::schema::{KIND_INFO, Walker};
use crate::id::{RecordId, RecordKind};
use crate::{KINDS, canon, kind_of, slug};

/// A record's id and feature (the same id can be in several features: a feature's version
/// changes the live one while the feature is on).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize)]
pub struct Key {
    pub id: RecordId,
    #[serde(default = "crate::record::live")]
    pub feature: String,
}

impl Key {
    pub fn new(id: RecordId, feature: impl Into<String>) -> Self {
        Key {
            id,
            feature: feature.into(),
        }
    }

    pub fn parse(id: &str, feature: impl Into<String>) -> Result<Self, String> {
        Ok(Key::new(id.parse()?, feature))
    }

    /// The live version of a well-formed id (panics on a bad one: for ids written in code).
    pub fn live(id: &str) -> Self {
        Key::parse(id, "live").expect("a record id")
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub value: Value,
    /// Relative to the tree.
    pub file: PathBuf,
}

/// A record naming another: where in it (a JSON pointer).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct UsedBy {
    #[serde(flatten)]
    pub key: Key,
    pub path: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "error", content = "details", rename_all = "snake_case")]
pub enum EditError {
    NotFound(String),
    /// The record doesn't parse, or names records that don't exist or don't fit.
    Invalid(Vec<String>),
    /// Fields both this edit and one saved since changed.
    Conflicts(Vec<Conflict>),
    /// Others name it: replace those references first (`replace_with`).
    Referenced(Vec<UsedBy>),
    Io(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            EditError::NotFound(m) | EditError::Io(m) => write!(f, "{m}"),
            EditError::Invalid(list) => write!(f, "{}", list.join("; ")),
            EditError::Conflicts(c) => write!(f, "{} fields changed meanwhile", c.len()),
            EditError::Referenced(r) => write!(f, "{} records name it", r.len()),
        }
    }
}

/// A new record: from `fields`, or a copy of another with `fields` on top.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct NewRecord {
    pub kind: String,
    #[serde(default)]
    pub name: String,
    /// Records kept per world: the playfield.
    #[serde(default)]
    pub world: Option<u32>,
    #[serde(default)]
    pub feature: Option<String>,
    #[serde(default)]
    pub fields: Option<Value>,
    #[serde(default)]
    pub copy_of: Option<Key>,
}

enum Layout {
    Record(Key),
    Sections(Vec<(String, Vec<Key>)>),
}

pub struct Work {
    dir: PathBuf,
    records: BTreeMap<Key, Entry>,
    files: BTreeMap<PathBuf, Layout>,
    walker: Walker,
    /// id -> who names it
    used_by: HashMap<String, BTreeSet<UsedBy>>,
    /// Problems reading the tree (unreadable files, an id twice in a feature).
    pub notes: Vec<String>,
}

fn feature_of(v: &Value) -> String {
    v.get("feature")
        .and_then(Value::as_str)
        .filter(|f| !f.is_empty())
        .unwrap_or("live")
        .to_string()
}

fn rev_of(v: &Value) -> u64 {
    v.get("rev").and_then(Value::as_u64).unwrap_or(0)
}

/// Where a feature's files go: `live` at the top, `feature/x` in `features/x/`, `other/x` in
/// `features/other/x/`.
fn feature_dir(feature: &str) -> PathBuf {
    match feature {
        "live" => PathBuf::new(),
        f => Path::new("features").join(f.strip_prefix("feature/").unwrap_or(f)),
    }
}

impl Work {
    /// Reads the tree at `dir`.
    pub fn open(dir: &Path) -> anyhow::Result<Work> {
        let mut work = Work {
            dir: dir.to_path_buf(),
            records: BTreeMap::new(),
            files: BTreeMap::new(),
            walker: Walker::new(),
            used_by: HashMap::new(),
            notes: Vec::new(),
        };
        let mut files = Vec::new();
        walk(dir, &mut files);
        files.sort();
        anyhow::ensure!(
            dir.join("content.json").exists(),
            "{} is not a content tree (no content.json)",
            dir.display()
        );
        for file in files {
            let rel = file.strip_prefix(dir).unwrap_or(&file).to_path_buf();
            if ROOT_FILES.iter().any(|r| rel == Path::new(r)) || rel.starts_with("schemas") {
                continue;
            }
            let value: Value = match std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
            {
                Ok(v) => v,
                Err(e) => {
                    work.notes.push(format!("{}: {e}", rel.display()));
                    continue;
                }
            };
            let layout = match value {
                Value::Object(ref o) if o.contains_key("id") => match work.add(value, &rel) {
                    Some(k) => Layout::Record(k),
                    None => continue,
                },
                Value::Object(o) => Layout::Sections(
                    o.into_iter()
                        .map(|(section, list)| {
                            let keys = match list {
                                Value::Array(a) => {
                                    a.into_iter().filter_map(|r| work.add(r, &rel)).collect()
                                }
                                _ => Vec::new(),
                            };
                            (section, keys)
                        })
                        .collect(),
                ),
                _ => continue,
            };
            work.files.insert(rel, layout);
        }
        let keys: Vec<Key> = work.records.keys().cloned().collect();
        for k in keys {
            work.index(&k);
        }
        Ok(work)
    }

    fn add(&mut self, value: Value, rel: &Path) -> Option<Key> {
        let raw = value.get("id").and_then(Value::as_str)?;
        let key = match Key::parse(raw, feature_of(&value)) {
            Ok(k) => k,
            Err(e) => {
                self.notes.push(format!("{}: {e}", rel.display()));
                return None;
            }
        };
        if self.records.contains_key(&key) {
            self.notes.push(format!(
                "{}: {} is in {} too",
                rel.display(),
                key.id,
                self.records[&key].file.display()
            ));
            return None;
        }
        self.records.insert(
            key.clone(),
            Entry {
                value,
                file: rel.to_path_buf(),
            },
        );
        Some(key)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn get(&self, key: &Key) -> Option<&Entry> {
        self.records.get(key)
    }

    /// Every version of an id (one per feature).
    pub fn versions(&self, id: &str) -> Vec<(&Key, &Entry)> {
        let Ok(id) = id.parse::<RecordId>() else { return Vec::new() };
        self.records
            .range(Key::new(id.clone(), "")..)
            .take_while(|(k, _)| k.id == id)
            .collect()
    }

    pub fn exists(&self, id: &str) -> bool {
        !self.versions(id).is_empty()
    }

    pub fn records(&self) -> impl Iterator<Item = (&Key, &Entry)> {
        self.records.iter()
    }

    /// The world folder of a record kept in one (`4200-isle-of-yarr`).
    pub fn world_of(&self, key: &Key) -> Option<String> {
        world_folder(&self.records.get(key)?.file)
    }

    /// Who names `id` (in any of their features).
    pub fn used_by(&self, id: &str) -> Vec<UsedBy> {
        self.used_by
            .get(id)
            .map(|s| s.iter().filter(|u| u.key.id.to_string() != id).cloned().collect())
            .unwrap_or_default()
    }

    /// The references in a record (spawn overrides by their template's schema).
    fn refs_of(&self, value: &Value) -> Vec<crate::schema::FoundRef> {
        let Some(kind) = value.get("id").and_then(Value::as_str).and_then(kind_of) else {
            return Vec::new();
        };
        let mut found = self.walker.find(kind, value).refs;
        if kind == "spawn"
            && let Some(o) = value.get("overrides")
            && let Some(t) = value
                .get("template")
                .and_then(Value::as_str)
                .and_then(kind_of)
        {
            found.extend(self.walker.find(t, o).refs.into_iter().map(|mut r| {
                r.path = format!("/overrides{}", r.path);
                r
            }));
        }
        found
    }

    fn index(&mut self, key: &Key) {
        let Some(e) = self.records.get(key) else {
            return;
        };
        for r in self.refs_of(&e.value) {
            self.used_by.entry(r.id).or_default().insert(UsedBy {
                key: key.clone(),
                path: r.path,
            });
        }
    }

    fn unindex(&mut self, key: &Key) {
        let Some(e) = self.records.get(key) else {
            return;
        };
        for r in self.refs_of(&e.value) {
            if let Some(s) = self.used_by.get_mut(&r.id) {
                s.retain(|u| &u.key != key);
            }
        }
    }

    /// What's wrong with a record on its own: it doesn't parse as its kind, or names records
    /// that don't exist or are of a kind that doesn't fit.
    pub fn check(&self, value: &Value) -> Vec<String> {
        let mut problems = Vec::new();
        let id = value.get("id").and_then(Value::as_str).unwrap_or("");
        if let Err(e) = typed(value.clone()) {
            problems.push(e);
        }
        for r in self.refs_of(value) {
            let field = r.path.get(1..).unwrap_or("");
            if !kind_of(&r.id).is_some_and(|k| r.kinds.iter().any(|x| x == k)) {
                problems.push(format!(
                    "{field}: {:?} isn't a {}",
                    r.id,
                    r.kinds
                        .iter()
                        .map(|k| format!("{k}/"))
                        .collect::<Vec<_>>()
                        .join(" or ")
                ));
            } else if r.id != id && !self.exists(&r.id) {
                problems.push(format!("{field}: {} doesn't exist", r.id));
            }
        }
        problems
    }

    /// Saves an edit of a record. `base` is the version the editor started from; when the
    /// record was saved since, the two edits are merged field by field (conflicts: nothing is
    /// saved). Returns the record as saved.
    pub fn save(
        &mut self,
        key: &Key,
        record: Value,
        base: Option<&Value>,
        author: &str,
        now: u64,
    ) -> Result<Value, EditError> {
        let current = self
            .records
            .get(key)
            .ok_or_else(|| {
                EditError::NotFound(format!("{} ({}) doesn't exist", key.id, key.feature))
            })?
            .value
            .clone();
        if record.get("id").and_then(Value::as_str) != Some(key.id.to_string().as_str())
            || feature_of(&record) != key.feature
        {
            return Err(EditError::Invalid(vec![
                "the id and feature can't change (copy it instead)".into(),
            ]));
        }
        let mut merged = if rev_of(&record) == rev_of(&current) {
            record
        } else {
            let base = base.ok_or_else(|| {
                EditError::Invalid(vec![format!(
                    "{} was saved since (rev {}): send the version you edited as base",
                    key.id,
                    rev_of(&current)
                )])
            })?;
            merge3(base, &record, &current).map_err(EditError::Conflicts)?
        };
        if strip(&merged) == strip(&current) {
            return Ok(current);
        }
        stamp(&mut merged, rev_of(&current) + 1, author, now);
        let problems = self.check(&merged);
        if !problems.is_empty() {
            return Err(EditError::Invalid(problems));
        }
        self.unindex(key);
        self.records.get_mut(key).expect("checked").value = merged.clone();
        self.index(key);
        let file = self.records[key].file.clone();
        self.write(&file)?;
        Ok(merged)
    }

    /// A free id for a new record of `kind` named `name` (`npc/pirate`, `npc/pirate-2`, ...).
    pub fn new_id(&self, kind: &str, name: &str, playfield: Option<u32>) -> String {
        let prefix = match playfield {
            Some(pf) if kind != "world" => format!("{kind}/{pf}/"),
            _ => format!("{kind}/"),
        };
        let base = slug(if name.trim().is_empty() { "new" } else { name });
        (1..)
            .map(|n| {
                if n == 1 {
                    format!("{prefix}{base}")
                } else {
                    format!("{prefix}{base}-{n}")
                }
            })
            .find(|id| !self.exists(id))
            .expect("a free id")
    }

    /// The folder of the world with this playfield (`worlds/4200-isle-of-yarr`).
    fn world_dir(&self, playfield: u32) -> Option<PathBuf> {
        self.records
            .iter()
            .find(|(k, e)| {
                k.id.kind == RecordKind::World
                    && e.value.get("playfield").and_then(Value::as_u64) == Some(playfield as u64)
            })
            .and_then(|(_, e)| e.file.parent().map(Path::to_path_buf))
            .and_then(|p| {
                p.strip_prefix(feature_dir("live"))
                    .ok()
                    .map(Path::to_path_buf)
            })
    }

    /// Makes a record: its id from its name (numbered when taken), its file from its kind,
    /// world and feature.
    pub fn create(
        &mut self,
        new: NewRecord,
        author: &str,
        now: u64,
    ) -> Result<(Key, Value), EditError> {
        let kind = KINDS
            .iter()
            .find(|(k, _)| *k == new.kind)
            .map(|(k, _)| *k)
            .ok_or_else(|| EditError::Invalid(vec![format!("no kind {:?}", new.kind)]))?;
        let per_world = KIND_INFO.iter().any(|k| k.kind == kind && k.per_world);
        let mut value = match &new.copy_of {
            Some(from) => {
                let e = self.records.get(from).ok_or_else(|| {
                    EditError::NotFound(format!("{} ({}) doesn't exist", from.id, from.feature))
                })?;
                let mut v = e.value.clone();
                if let Value::Object(o) = &mut v {
                    for k in ["id", "rev", "author", "updated"] {
                        o.remove(k);
                    }
                }
                v
            }
            None => Value::Object(Map::new()),
        };
        if kind_of(
            &new.copy_of
                .as_ref()
                .map_or(format!("{kind}/"), |k| k.id.to_string()),
        ) != Some(kind)
        {
            return Err(EditError::Invalid(vec![
                "a copy is of the same kind".into(),
            ]));
        }
        if let Some(f) = &new.fields {
            merge(&mut value, f);
        }
        let playfield = match (per_world, new.world, &new.copy_of) {
            (false, _, _) => None,
            (true, Some(pf), _) => Some(pf),
            // a copy stays in its world
            (true, None, Some(from)) => from.id.world,
            (true, None, None) if kind == "world" => value
                .get("playfield")
                .and_then(Value::as_u64)
                .map(|p| p as u32),
            (true, None, None) => {
                return Err(EditError::Invalid(vec![format!(
                    "a {kind} needs its world"
                )]));
            }
        };
        let name = match (new.name.trim(), &new.copy_of) {
            ("", Some(_)) => format!(
                "{} copy",
                value.get("name").and_then(Value::as_str).unwrap_or("")
            )
            .trim()
            .to_string(),
            (n, _) => n.to_string(),
        };
        let id = self.new_id(kind, &name, playfield);
        let feature = new
            .feature
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| "live".into());
        let o = value
            .as_object_mut()
            .ok_or_else(|| EditError::Invalid(vec!["fields must be an object".into()]))?;
        o.insert("id".into(), json!(id));
        if !name.is_empty() {
            o.insert("name".into(), json!(name));
        }
        if feature == "live" {
            o.remove("feature");
        } else {
            o.insert("feature".into(), json!(feature));
        }
        stamp(&mut value, 1, author, now);
        let problems = self.check(&value);
        if !problems.is_empty() {
            return Err(EditError::Invalid(problems));
        }
        let key = Key::parse(&id, feature).map_err(|e| EditError::Invalid(vec![e]))?;
        self.insert_new(&key, value.clone())?;
        Ok((key, value))
    }

    /// Where a record goes: its kind's folder or world file, in its feature's folder.
    pub fn file_for(&self, key: &Key) -> Result<PathBuf, EditError> {
        let kind = key.id.kind;
        let path = KINDS.iter().find(|(k, _)| *k == kind.as_str()).map(|(_, p)| *p).expect("kind");
        let slug_part = &key.id.slug;
        Ok(match path.strip_prefix("worlds/<world>/") {
            Some(rest) => {
                let world = if kind == RecordKind::World {
                    let pf = self.records.get(key).and_then(|e| e.value.get("playfield")).and_then(Value::as_u64).unwrap_or(0);
                    PathBuf::from("worlds").join(format!("{pf}-{slug_part}"))
                } else {
                    let pf = key.id.world.ok_or_else(|| EditError::Invalid(vec![format!("{} names no world", key.id)]))?;
                    self.world_dir(pf).ok_or_else(|| EditError::Invalid(vec![format!("no world with playfield {pf}")]))?
                };
                let rest = if rest.ends_with('/') { format!("{rest}{slug_part}.json") } else { rest.to_string() };
                feature_dir(&key.feature).join(world).join(rest)
            }
            None => feature_dir(&key.feature).join(path).join(format!("{slug_part}.json")),
        })
    }

    /// Adds a record that isn't there yet, in its file (checked before).
    fn insert_new(&mut self, key: &Key, value: Value) -> Result<(), EditError> {
        let file = self.file_for(key)?;
        let section = file.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let sectioned = key.id.kind.per_world() && key.id.kind != RecordKind::Script;
        match self.files.get_mut(&file) {
            Some(Layout::Sections(list)) if sectioned => match list.iter_mut().find(|(s, _)| *s == section) {
                Some((_, keys)) => keys.push(key.clone()),
                None => list.push((section, vec![key.clone()])),
            },
            Some(_) => return Err(EditError::Invalid(vec![format!("{} is taken", file.display())])),
            None if sectioned => {
                self.files.insert(file.clone(), Layout::Sections(vec![(section, vec![key.clone()])]));
            }
            None => {
                self.files.insert(file.clone(), Layout::Record(key.clone()));
            }
        }
        self.records.insert(key.clone(), Entry { value, file: file.clone() });
        self.index(key);
        self.write(&file)
    }

    /// Makes the record exactly `record` (its id and feature from it): a new one, or the one
    /// there replaced without merging (undo and redo, paste, in-game edits). Its `rev` goes on
    /// from the one there.
    pub fn put(&mut self, mut record: Value, author: &str, now: u64) -> Result<Value, EditError> {
        let id = record.get("id").and_then(Value::as_str).ok_or_else(|| EditError::Invalid(vec!["a record needs its id".into()]))?;
        let key = Key::parse(id, feature_of(&record)).map_err(|e| EditError::Invalid(vec![e]))?;
        let rev = self.records.get(&key).map(|e| rev_of(&e.value));
        if let Some(rev) = rev {
            if let Value::Object(o) = &mut record {
                o.insert("rev".into(), json!(rev));
            }
            return self.save(&key, record, None, author, now);
        }
        stamp(&mut record, 1, author, now);
        let problems = self.check(&record);
        if !problems.is_empty() {
            return Err(EditError::Invalid(problems));
        }
        self.insert_new(&key, record.clone())?;
        Ok(record)
    }

    /// Sets a record back to `to` exactly (its last committed version: discarding unpublished
    /// changes), or takes it out when it had none (`None`), without checks.
    pub fn reset(&mut self, key: &Key, to: Option<Value>) -> Result<(), EditError> {
        self.reset_before(key, to, None)
    }

    /// As [`Work::reset`]; a record put back goes before `before` in its file (where it was).
    pub fn reset_before(&mut self, key: &Key, to: Option<Value>, before: Option<&Key>) -> Result<(), EditError> {
        match to {
            Some(v) if self.records.contains_key(key) => {
                self.unindex(key);
                self.records.get_mut(key).expect("checked").value = v;
                self.index(key);
                let file = self.records[key].file.clone();
                self.write(&file)
            }
            Some(v) => {
                self.insert_new(key, v)?;
                if let Some(b) = before {
                    self.move_before(key, b);
                    let file = self.records[key].file.clone();
                    self.write(&file)?;
                }
                Ok(())
            }
            None if self.records.contains_key(key) => self.take_out(key),
            None => Ok(()),
        }
    }

    /// Moves `key` before `before` in their file's section (when both are there).
    fn move_before(&mut self, key: &Key, before: &Key) {
        let Some(file) = self.records.get(key).map(|e| e.file.clone()) else { return };
        if let Some(Layout::Sections(list)) = self.files.get_mut(&file) {
            for (_, keys) in list.iter_mut() {
                if let (Some(from), true) = (keys.iter().position(|k| k == key), keys.contains(before)) {
                    let k = keys.remove(from);
                    let to = keys.iter().position(|k| k == before).expect("checked");
                    keys.insert(to, k);
                }
            }
        }
    }

    /// Removes a record from its file (and the file when it was the last).
    fn take_out(&mut self, key: &Key) -> Result<(), EditError> {
        self.unindex(key);
        let entry = self.records.remove(key).expect("checked");
        let remove_file = match self.files.get_mut(&entry.file) {
            Some(Layout::Sections(list)) => {
                for (_, keys) in list.iter_mut() {
                    keys.retain(|k| k != key);
                }
                list.retain(|(_, keys)| !keys.is_empty());
                list.is_empty()
            }
            _ => true,
        };
        if remove_file {
            self.files.remove(&entry.file);
            std::fs::remove_file(self.dir.join(&entry.file)).map_err(|e| EditError::Io(e.to_string()))
        } else {
            self.write(&entry.file)
        }
    }

    /// Deletes a record. While others name it (and no other feature's version stays), it is
    /// refused with the list, unless `replace_with` names another record to put in their place.
    /// Returns the records changed to name `replace_with`.
    pub fn delete(
        &mut self,
        key: &Key,
        replace_with: Option<&str>,
        author: &str,
        now: u64,
    ) -> Result<Vec<Key>, EditError> {
        if !self.records.contains_key(key) {
            return Err(EditError::NotFound(format!(
                "{} ({}) doesn't exist",
                key.id, key.feature
            )));
        }
        let last = self.versions(&key.id.to_string()).len() == 1;
        let users = if last {
            self.used_by(&key.id.to_string())
        } else {
            Vec::new()
        };
        let mut changed = Vec::new();
        if !users.is_empty() {
            let Some(with) = replace_with else {
                return Err(EditError::Referenced(users));
            };
            if kind_of(with) != Some(key.id.kind.as_str()) || !self.exists(with) {
                return Err(EditError::Invalid(vec![format!(
                    "{with} isn't a {} that exists",
                    key.id.kind.as_str()
                )]));
            }
            let mut by_record: BTreeMap<Key, Vec<String>> = BTreeMap::new();
            for u in users {
                by_record.entry(u.key).or_default().push(u.path);
            }
            for (k, paths) in by_record {
                let mut v = self.records[&k].value.clone();
                for p in paths {
                    replace_ref(&mut v, &p, &key.id.to_string(), with);
                }
                self.save(&k, v, None, author, now)?;
                changed.push(k);
            }
        }
        self.take_out(key)?;
        Ok(changed)
    }

    /// A record as the game sees it with these features on: the feature versions over the live
    /// one, then `extends`. `origin` says, per top-level field, which record (and feature) gave
    /// it.
    pub fn resolve(
        &self,
        id: &str,
        features: &[String],
    ) -> Option<(Value, BTreeMap<String, String>)> {
        self.resolve_inner(id, features, &mut Vec::new())
    }

    fn resolve_inner(
        &self,
        id: &str,
        features: &[String],
        stack: &mut Vec<String>,
    ) -> Option<(Value, BTreeMap<String, String>)> {
        if stack.iter().any(|s| s == id) {
            return None;
        }
        let mut versions: Vec<(&Key, &Entry)> = self
            .versions(id)
            .into_iter()
            .filter(|(k, _)| k.feature == "live" || feature_active(&k.feature, features))
            .collect();
        versions.sort_by_key(|(k, _)| {
            if k.feature == "live" {
                0
            } else {
                k.feature.matches('/').count() + 1
            }
        });
        let mut origin = BTreeMap::new();
        let mut value = Value::Null;
        for (k, e) in versions {
            let by = if k.feature == "live" {
                k.id.to_string()
            } else {
                format!("{} ({})", k.id, k.feature)
            };
            for f in e.value.as_object().into_iter().flat_map(|o| o.keys()) {
                origin.insert(f.clone(), by.clone());
            }
            if value.is_null() {
                value = e.value.clone()
            } else {
                merge(&mut value, &e.value)
            }
        }
        if value.is_null() {
            return None;
        }
        if let Some(parent) = value
            .get("extends")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            stack.push(id.to_string());
            if let Some((mut base, mut from)) = self.resolve_inner(&parent, features, stack) {
                if let Value::Object(o) = &mut base {
                    for k in [
                        "id", "extends", "feature", "rev", "author", "updated", "source", "tags",
                        "comment",
                    ] {
                        o.remove(k);
                        from.remove(k);
                    }
                }
                merge(&mut base, &value);
                from.extend(origin);
                value = base;
                origin = from;
            }
            stack.pop();
        }
        Some((value, origin))
    }

    /// Writes a file of the tree anew (canonical form), through a temporary file.
    fn write(&self, rel: &Path) -> Result<(), EditError> {
        let io = |e: &dyn std::fmt::Display| EditError::Io(format!("{}: {e}", rel.display()));
        let parsed = |k: &Key| typed(self.records[k].value.clone()).map_err(|e| io(&e));
        let text = match self.files.get(rel) {
            Some(Layout::Record(k)) => canon::to_string(&parsed(k)?),
            Some(Layout::Sections(list)) => {
                let mut out: BTreeMap<&str, Vec<AnyRecord>> = BTreeMap::new();
                for (section, keys) in list {
                    out.insert(section, keys.iter().map(parsed).collect::<Result<_, _>>()?);
                }
                canon::to_string(&out)
            }
            None => return Ok(()),
        }
        .map_err(|e| io(&e))?;
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(&e))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| io(&e))
    }
}

/// A record without what every save changes.
fn strip(v: &Value) -> Value {
    let mut v = v.clone();
    if let Value::Object(o) = &mut v {
        for k in ["rev", "author", "updated"] {
            o.remove(k);
        }
    }
    v
}

fn stamp(v: &mut Value, rev: u64, author: &str, now: u64) {
    if let Value::Object(o) = v {
        o.insert("rev".into(), json!(rev));
        if author.is_empty() {
            o.remove("author");
        } else {
            o.insert("author".into(), json!(author));
        }
        o.insert("updated".into(), json!(now));
    }
}

/// Puts `new` where `old` is named at `path`: the string there, or a map key (`arrival_from`).
fn replace_ref(v: &mut Value, path: &str, old: &str, new: &str) {
    if let Some(s) = v.pointer_mut(path)
        && s.as_str() == Some(old)
    {
        *s = json!(new);
        return;
    }
    let parent = &path[..path.rfind('/').unwrap_or(0)];
    if let Some(Value::Object(o)) = v.pointer_mut(parent)
        && let Some(x) = o.remove(old)
    {
        o.insert(new.to_string(), x);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small tree in a temporary folder.
    fn tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lmoxide-work-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let put = |rel: &str, v: Value| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, canon::to_string(&v).unwrap()).unwrap();
        };
        put(
            "content.json",
            json!({ "format": 1, "reference": "../data" }),
        );
        put(
            "worlds/4200-isle/world.json",
            json!({ "id": "world/isle", "playfield": 4200 }),
        );
        put(
            "worlds/4200-isle/spawns.json",
            json!({ "spawns": [
            { "id": "spawn/4200/a", "kind": "enemy", "template": "npc/pirate", "pos": [1.0, 2.0, 3.0] },
            { "id": "spawn/4200/b", "kind": "enemy", "template": "npc/pirate", "pos": [4.0, 5.0, 6.0] }
        ] }),
        );
        put(
            "npcs/pirate.json",
            json!({ "id": "npc/pirate", "name": "Pirate", "level": 3, "loot": "loot/pirate" }),
        );
        put(
            "npcs/captain.json",
            json!({ "id": "npc/captain", "extends": "npc/pirate", "name": "Captain", "level": 5 }),
        );
        put(
            "loot/pirate.json",
            json!({ "id": "loot/pirate", "entries": [] }),
        );
        crate::load::format_tree(&dir, false).unwrap();
        dir
    }

    fn read(dir: &Path, rel: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join(rel)).unwrap()).unwrap()
    }

    #[test]
    fn saves_merges_and_checks() {
        let dir = tree("save");
        let mut w = Work::open(&dir).unwrap();
        let key = Key::live("npc/pirate");
        let base = w.get(&key).unwrap().value.clone();
        // one editor changes the level
        let mut a = base.clone();
        a["level"] = json!(4);
        let saved = w.save(&key, a, Some(&base), "ann", 100).unwrap();
        assert_eq!(
            (saved["rev"].clone(), saved["author"].clone()),
            (json!(1), json!("ann"))
        );
        // another, from the same base, the name: merged
        let mut b = base.clone();
        b["name"] = json!("Scallywag");
        let saved = w.save(&key, b, Some(&base), "bob", 101).unwrap();
        assert_eq!(
            (
                saved["level"].clone(),
                saved["name"].clone(),
                saved["rev"].clone()
            ),
            (json!(4), json!("Scallywag"), json!(2))
        );
        assert_eq!(read(&dir, "npcs/pirate.json")["name"], "Scallywag");
        // both the level: a conflict, nothing saved
        let mut c = base.clone();
        c["level"] = json!(9);
        assert!(
            matches!(w.save(&key, c, Some(&base), "cy", 102), Err(EditError::Conflicts(c)) if c[0].path == "/level")
        );
        // without a base after a save meanwhile: refused
        let mut d = base.clone();
        d["level"] = json!(9);
        assert!(matches!(
            w.save(&key, d, None, "cy", 102),
            Err(EditError::Invalid(_))
        ));
        // a wrong reference
        let mut e = w.get(&key).unwrap().value.clone();
        e["loot"] = json!("npc/captain");
        let Err(EditError::Invalid(p)) = w.save(&key, e, None, "cy", 103) else {
            panic!()
        };
        assert!(p[0].contains("isn't a loot/"), "{p:?}");
        let mut e = w.get(&key).unwrap().value.clone();
        e["loot"] = json!("loot/missing");
        assert!(
            matches!(w.save(&key, e, None, "cy", 103), Err(EditError::Invalid(p)) if p[0].contains("doesn't exist"))
        );
        // the file is still canonical and loads
        assert!(crate::load::format_tree(&dir, true).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_copies_and_deletes() {
        let dir = tree("create");
        let mut w = Work::open(&dir).unwrap();
        // a spawn goes in its world's spawns.json, numbered when the name is taken
        let (k, _) = w
            .create(
                NewRecord {
                    kind: "spawn".into(),
                    name: "A".into(),
                    world: Some(4200),
                    fields: Some(
                        json!({ "kind": "enemy", "template": "npc/captain", "pos": [0, 0, 0] }),
                    ),
                    ..Default::default()
                },
                "ann",
                1,
            )
            .unwrap();
        assert_eq!(k.id.to_string(), "spawn/4200/a-2");
        assert_eq!(
            read(&dir, "worlds/4200-isle/spawns.json")["spawns"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        // a copy of an NPC gets its own file
        let (k, v) = w
            .create(
                NewRecord {
                    kind: "npc".into(),
                    copy_of: Some(Key::live("npc/pirate")),
                    ..Default::default()
                },
                "ann",
                2,
            )
            .unwrap();
        assert_eq!(
            (k.id.to_string().as_str(), v["name"].as_str(), v["level"].clone()),
            ("npc/pirate-copy", Some("Pirate copy"), json!(3))
        );
        assert!(dir.join("npcs/pirate-copy.json").exists());
        // a feature's version goes under features/
        let (k, _) = w
            .create(
                NewRecord {
                    kind: "npc".into(),
                    name: "Elf".into(),
                    feature: Some("feature/christmas".into()),
                    ..Default::default()
                },
                "ann",
                3,
            )
            .unwrap();
        assert_eq!(
            w.get(&k).unwrap().file,
            Path::new("features/christmas/npcs/elf.json")
        );
        // pirate is used by the spawns and the captain: refused, then replaced
        let Err(EditError::Referenced(users)) = w.delete(&Key::live("npc/pirate"), None, "ann", 4)
        else {
            panic!()
        };
        assert_eq!(
            users.iter().map(|u| u.key.id.to_string()).collect::<Vec<_>>(),
            ["spawn/4200/a", "spawn/4200/b", "npc/captain"]
        );
        let changed = w
            .delete(&Key::live("npc/pirate"), Some("npc/pirate-copy"), "ann", 5)
            .unwrap();
        assert_eq!(changed.len(), 3);
        assert!(!dir.join("npcs/pirate.json").exists());
        assert_eq!(
            read(&dir, "npcs/captain.json")["extends"],
            "npc/pirate-copy"
        );
        assert_eq!(
            read(&dir, "worlds/4200-isle/spawns.json")["spawns"][0]["template"],
            "npc/pirate-copy"
        );
        assert!(w.used_by("npc/pirate").is_empty());
        assert_eq!(w.used_by("npc/pirate-copy").len(), 3);
        // removing the last spawns leaves the file without them
        for id in ["spawn/4200/a", "spawn/4200/b", "spawn/4200/a-2"] {
            w.delete(&Key::live(id), None, "ann", 6).unwrap();
        }
        assert!(!dir.join("worlds/4200-isle/spawns.json").exists());
        assert!(crate::load::format_tree(&dir, true).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_features_and_extends() {
        let dir = tree("resolve");
        let mut w = Work::open(&dir).unwrap();
        w.create(
            NewRecord {
                kind: "npc".into(),
                name: "x".into(),
                feature: Some("feature/christmas".into()),
                ..Default::default()
            },
            "",
            1,
        )
        .unwrap();
        // a christmas version of the pirate
        let p = dir.join("features/christmas/npcs/pirate.json");
        std::fs::write(
            &p,
            canon::to_string(
                &json!({ "id": "npc/pirate", "feature": "feature/christmas", "level": 7 }),
            )
            .unwrap(),
        )
        .unwrap();
        let w = Work::open(&dir).unwrap();
        let (v, origin) = w.resolve("npc/captain", &[]).unwrap();
        assert_eq!(
            (v["level"].clone(), v["loot"].clone()),
            (json!(5), json!("loot/pirate"))
        );
        assert_eq!(
            (origin["loot"].as_str(), origin["level"].as_str()),
            ("npc/pirate", "npc/captain")
        );
        let (v, origin) = w
            .resolve("npc/pirate", &["feature/christmas".into()])
            .unwrap();
        assert_eq!(
            (v["level"].clone(), origin["level"].as_str()),
            (json!(7), "npc/pirate (feature/christmas)")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
