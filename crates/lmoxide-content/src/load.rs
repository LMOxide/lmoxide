//! Reading the content tree: features, `extends`, typed records, reference checks.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::model::*;
use crate::tunables::{Manifest, Texts, Tunables};
use crate::{FORMAT, kind_of};

#[derive(Debug, Clone)]
pub struct Options {
    /// Features whose records apply (a feature is on when it or a parent is listed).
    pub features: Vec<String>,
    /// Check client ids (looks, effect packages) against the reference catalogs.
    pub check_client_ids: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            features: vec!["live".into()],
            check_client_ids: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Problem {
    pub severity: Severity,
    pub file: Option<PathBuf>,
    pub id: Option<String>,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let level = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(f, "{level}")?;
        if let Some(file) = &self.file {
            write!(f, " {}", file.display())?;
        }
        if let Some(id) = &self.id {
            write!(f, " [{id}]")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// The loaded content, with the active features and inheritance applied.
#[derive(Debug, Default)]
pub struct Content {
    pub manifest: Option<Manifest>,
    pub tunables: Tunables,
    pub texts: Texts,
    pub worlds: BTreeMap<String, World>,
    pub spawns: BTreeMap<String, Spawn>,
    pub portals: BTreeMap<String, Portal>,
    pub bouncers: BTreeMap<String, Bouncer>,
    pub areas: BTreeMap<String, Area>,
    pub triggers: BTreeMap<String, Trigger>,
    pub puzzles: BTreeMap<String, Puzzle>,
    pub scripts: BTreeMap<String, Script>,
    pub regions: BTreeMap<String, Region>,
    pub npcs: BTreeMap<String, Npc>,
    pub factions: BTreeMap<String, Faction>,
    pub objects: BTreeMap<String, Object>,
    pub skills: BTreeMap<String, Skill>,
    pub buffs: BTreeMap<String, Buff>,
    pub loot: BTreeMap<String, LootTable>,
    pub items: BTreeMap<String, Item>,
    pub minifigures: BTreeMap<String, Minifigure>,
    pub dialogues: BTreeMap<String, Dialogue>,
    pub achievements: BTreeMap<String, Achievement>,
    /// The world of each record kept in a world's folder.
    pub world_of: BTreeMap<String, String>,
    /// Where each record is in the tree (files in name order, records in file order).
    pub order: HashMap<String, usize>,
    /// Every record as resolved JSON (features and `extends` applied), by id.
    pub resolved: BTreeMap<String, Value>,
}

impl Content {
    /// The playfield's world.
    pub fn world_by_playfield(&self, playfield: u32) -> Option<&World> {
        self.worlds.values().find(|w| w.playfield == playfield)
    }

    /// Records of a world's folder of one kind (`spawns` of world `w`, ...), in file order.
    pub fn in_world<'a, T>(
        &'a self,
        records: &'a BTreeMap<String, T>,
        world: &'a str,
    ) -> impl Iterator<Item = &'a T> {
        let mut list: Vec<(&String, &T)> = records
            .iter()
            .filter(|(id, _)| self.world_of.get(*id).is_some_and(|w| w == world))
            .collect();
        list.sort_by_key(|(id, _)| self.order.get(*id).copied().unwrap_or(usize::MAX));
        list.into_iter().map(|(_, r)| r)
    }

    /// A spawn's template with its overrides applied, as JSON (parse it as the template's kind).
    pub fn spawn_template(&self, spawn: &Spawn) -> Option<Value> {
        let mut template = self.resolved.get(spawn.template.as_ref()?)?.clone();
        if let Some(o) = &spawn.overrides {
            merge(&mut template, o);
        }
        Some(template)
    }

    /// The template of an NPC spawn, overrides applied.
    pub fn spawn_npc(&self, spawn: &Spawn) -> Option<Npc> {
        serde_json::from_value(self.spawn_template(spawn)?).ok()
    }

    /// The template of an object spawn, overrides applied.
    pub fn spawn_object(&self, spawn: &Spawn) -> Option<Object> {
        serde_json::from_value(self.spawn_template(spawn)?).ok()
    }
}

/// Is `feature` on when `active` are (a feature is on when it or a parent is listed)?
pub fn feature_active(feature: &str, active: &[String]) -> bool {
    active.iter().any(|a| {
        feature == a
            || feature
                .strip_prefix(a.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Applies `patch` onto `base` (JSON Merge Patch): objects merge, `null` removes a field,
/// anything else (arrays too) replaces.
pub fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    b.remove(k);
                } else {
                    merge(b.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

struct Raw {
    value: Value,
    file: PathBuf,
    feature: String,
    world: Option<String>,
}

pub(crate) const ROOT_FILES: &[&str] = &["content.json", "tunables.json", "texts.json"];

/// Fields whose text is prose, never a reference.
const PROSE: &[&str] = &[
    "id",
    "name",
    "comment",
    "text",
    "title",
    "about",
    "rivals_about",
    "reference",
    "display_name",
    "prompt",
    "client_name",
];

/// Reads the content tree at `dir`. Errors leave the record out; the content is still returned.
pub fn load(dir: &Path, options: &Options) -> (Content, Vec<Problem>) {
    let mut problems = Vec::new();
    let mut content = Content::default();
    let mut err = |severity, file: Option<&Path>, id: Option<&str>, message: String| {
        problems.push(Problem {
            severity,
            file: file.map(Path::to_path_buf),
            id: id.map(str::to_string),
            message,
        })
    };

    // root files
    match read_typed::<Manifest>(&dir.join("content.json")) {
        Ok(m) => {
            if m.format != FORMAT {
                err(
                    Severity::Error,
                    Some(&dir.join("content.json")),
                    None,
                    format!("format {} (this server reads {FORMAT})", m.format),
                );
            }
            content.manifest = Some(m);
        }
        Err(e) => err(Severity::Error, Some(&dir.join("content.json")), None, e),
    }
    match read_typed::<Tunables>(&dir.join("tunables.json")) {
        Ok(t) => content.tunables = t,
        Err(e) => err(Severity::Error, Some(&dir.join("tunables.json")), None, e),
    }
    if dir.join("texts.json").exists() {
        match read_typed::<Texts>(&dir.join("texts.json")) {
            Ok(t) => content.texts = t,
            Err(e) => err(Severity::Error, Some(&dir.join("texts.json")), None, e),
        }
    }

    // every record, by id
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let mut by_id: BTreeMap<String, Vec<Raw>> = BTreeMap::new();
    for file in files {
        let rel = file.strip_prefix(dir).unwrap_or(&file);
        if ROOT_FILES.iter().any(|r| rel == Path::new(r)) || rel.starts_with("schemas") {
            continue;
        }
        let world = world_folder(rel);
        let value = match std::fs::read_to_string(&file)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
        {
            Ok(v) => v,
            Err(e) => {
                err(Severity::Error, Some(&file), None, e);
                continue;
            }
        };
        let records: Vec<Value> = match value {
            Value::Object(ref o) if o.contains_key("id") => vec![value],
            Value::Object(o) => {
                let mut records = Vec::new();
                for (section, v) in o {
                    match v {
                        Value::Array(a) => records.extend(a),
                        _ => err(
                            Severity::Error,
                            Some(&file),
                            None,
                            format!("section {section:?} isn't a list of records"),
                        ),
                    }
                }
                records
            }
            _ => {
                err(
                    Severity::Error,
                    Some(&file),
                    None,
                    "a content file is a record or an object of record lists".into(),
                );
                continue;
            }
        };
        for value in records {
            let Some(id) = value.get("id").and_then(Value::as_str).map(str::to_string) else {
                err(
                    Severity::Error,
                    Some(&file),
                    None,
                    "record without an id".into(),
                );
                continue;
            };
            if kind_of(&id).is_none() {
                err(
                    Severity::Error,
                    Some(&file),
                    Some(&id),
                    "the id doesn't start with a known kind (npc/, spawn/, ...)".into(),
                );
                continue;
            }
            let feature = value
                .get("feature")
                .and_then(Value::as_str)
                .filter(|f| !f.is_empty())
                .unwrap_or("live")
                .to_string();
            let next = content.order.len();
            content.order.entry(id.clone()).or_insert(next);
            by_id.entry(id).or_default().push(Raw {
                value,
                file: file.clone(),
                feature,
                world: world.clone(),
            });
        }
    }

    // features: the shallowest active record is the base, deeper ones change it
    let mut merged: BTreeMap<String, (Value, PathBuf, Option<String>)> = BTreeMap::new();
    for (id, mut raws) in by_id {
        raws.retain(|r| feature_active(&r.feature, &options.features));
        raws.sort_by_key(|r| r.feature.split('/').count());
        let mut seen = HashSet::new();
        for r in &raws {
            if !seen.insert(r.feature.clone()) {
                err(
                    Severity::Error,
                    Some(&r.file),
                    Some(&id),
                    format!("defined twice in feature {:?}", r.feature),
                );
            }
        }
        let mut it = raws.into_iter();
        let Some(first) = it.next() else { continue };
        let (mut value, file, world) = (first.value, first.file, first.world);
        for r in it {
            merge(&mut value, &r.value);
        }
        merged.insert(id, (value, file, world));
    }

    // extends
    let ids: Vec<String> = merged.keys().cloned().collect();
    let mut resolved: BTreeMap<String, Value> = BTreeMap::new();
    for id in &ids {
        resolve(id, &merged, &mut resolved, &mut Vec::new(), &mut err);
    }

    // world folders: their records belong to the world.json there
    let mut folder_world: BTreeMap<String, String> = BTreeMap::new();
    for (id, (_, _, world)) in &merged {
        if let (Some(w), Some("world")) = (world, kind_of(id)) {
            folder_world.insert(w.clone(), id.clone());
        }
    }
    for (id, (_, file, world)) in &merged {
        let Some(w) = world else { continue };
        match folder_world.get(w) {
            Some(world_id) => {
                content.world_of.insert(id.clone(), world_id.clone());
            }
            None => err(
                Severity::Error,
                Some(file),
                Some(id),
                format!("worlds/{w} has no world.json"),
            ),
        }
    }

    // typed records
    for (id, value) in &resolved {
        let file = &merged[id].1;
        let mut typed = |e: Result<(), String>| {
            if let Err(e) = e {
                err(Severity::Error, Some(file), Some(id), e);
            }
        };
        let k = kind_of(id).expect("checked");
        typed(match k {
            "world" => put(&mut content.worlds, id, value),
            "spawn" => put(&mut content.spawns, id, value),
            "portal" => put(&mut content.portals, id, value),
            "bouncer" => put(&mut content.bouncers, id, value),
            "area" => put(&mut content.areas, id, value),
            "trigger" => put(&mut content.triggers, id, value),
            "puzzle" => put(&mut content.puzzles, id, value),
            "script" => put(&mut content.scripts, id, value),
            "region" => put(&mut content.regions, id, value),
            "npc" => put(&mut content.npcs, id, value),
            "faction" => put(&mut content.factions, id, value),
            "object" => put(&mut content.objects, id, value),
            "skill" => put(&mut content.skills, id, value),
            "buff" => put(&mut content.buffs, id, value),
            "loot" => put(&mut content.loot, id, value),
            "item" => put(&mut content.items, id, value),
            "minifig" => put(&mut content.minifigures, id, value),
            "dialogue" => put(&mut content.dialogues, id, value),
            "achievement" => put(&mut content.achievements, id, value),
            _ => Err(format!("no loader for kind {k}")),
        });
    }

    // references
    for (id, value) in &resolved {
        let file = &merged[id].1;
        let mut refs = BTreeSet::new();
        collect_refs(value, None, &mut refs);
        for r in refs {
            if r != *id && !resolved.contains_key(&r) {
                err(
                    Severity::Error,
                    Some(file),
                    Some(id),
                    format!("refers to {r}, which doesn't exist"),
                );
            }
        }
    }
    // the kinds each reference may name (the schemas' x-lmoxide-ref); spawn overrides by
    // their template's schema
    let walker = crate::schema::Walker::new();
    for (id, value) in &resolved {
        let Some(kind) = kind_of(id) else { continue };
        let file = &merged[id].1;
        let mut found = walker.find(kind, value).refs;
        if kind == "spawn"
            && let Some(o) = value.get("overrides")
            && let Some(t) = value.get("template").and_then(Value::as_str).and_then(kind_of)
        {
            found.extend(walker.find(t, o).refs.into_iter().map(|mut r| {
                r.path = format!("/overrides{}", r.path);
                r
            }));
        }
        for r in found {
            if !kind_of(&r.id).is_some_and(|k| r.kinds.iter().any(|x| x == k)) {
                let wanted: Vec<String> = r.kinds.iter().map(|k| format!("{k}/")).collect();
                err(
                    Severity::Error,
                    Some(file),
                    Some(id),
                    format!("{} must name a {} (it names {:?})", &r.path[1..], wanted.join(" or "), r.id),
                );
            }
        }
    }
    for (id, s) in &content.spawns {
        let file = &merged[id].1;
        if s.overrides.is_some()
            && let Some(t) = content.spawn_template(s)
        {
            let parsed = match kind_of(s.template.as_deref().unwrap_or("")) {
                Some("npc") => serde_json::from_value::<Npc>(t).map(|_| ()),
                _ => serde_json::from_value::<Object>(t).map(|_| ()),
            };
            if let Err(e) = parsed {
                err(
                    Severity::Error,
                    Some(file),
                    Some(id),
                    format!("overrides don't fit the template: {e}"),
                );
            }
        }
    }
    for (id, r) in &resolved {
        if let Some(e) = r.get("extends").and_then(Value::as_str)
            && kind_of(e) != kind_of(id)
        {
            err(
                Severity::Error,
                Some(&merged[id].1),
                Some(id),
                format!("extends {e}, a different kind"),
            );
        }
    }

    // client ids
    if options.check_client_ids
        && let Some(m) = &content.manifest
    {
        check_client_ids(&dir.join(&m.reference), &resolved, &merged, &mut err);
    }

    content.resolved = resolved;
    problems.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.file.cmp(&b.file))
    });
    (content, problems)
}

fn put<T: DeserializeOwned>(
    map: &mut BTreeMap<String, T>,
    id: &str,
    value: &Value,
) -> Result<(), String> {
    let t = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    map.insert(id.to_string(), t);
    Ok(())
}

fn read_typed<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub(crate) fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(p);
        }
    }
}

/// `[features/<feature>/]worlds/<folder>/...` -> `<folder>`.
pub(crate) fn world_folder(rel: &Path) -> Option<String> {
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let at = parts.iter().position(|p| p == "worlds")?;
    // the folder, not a file directly in worlds/
    (at + 2 < parts.len()).then(|| parts[at + 1].clone())
}

type ErrFn<'a> = dyn FnMut(Severity, Option<&Path>, Option<&str>, String) + 'a;

fn resolve(
    id: &str,
    merged: &BTreeMap<String, (Value, PathBuf, Option<String>)>,
    resolved: &mut BTreeMap<String, Value>,
    stack: &mut Vec<String>,
    err: &mut ErrFn,
) -> Option<Value> {
    if let Some(v) = resolved.get(id) {
        return Some(v.clone());
    }
    let (value, file, _) = merged.get(id)?;
    if stack.iter().any(|s| s == id) {
        err(
            Severity::Error,
            Some(file),
            Some(id),
            format!("extends itself: {} -> {id}", stack.join(" -> ")),
        );
        return None;
    }
    let out = match value.get("extends").and_then(Value::as_str) {
        Some(parent) => {
            stack.push(id.to_string());
            let base = resolve(parent, merged, resolved, stack, err);
            stack.pop();
            match base {
                Some(mut base) => {
                    // what isn't inherited
                    if let Value::Object(o) = &mut base {
                        for k in [
                            "id", "extends", "feature", "rev", "author", "updated", "source",
                            "tags", "comment",
                        ] {
                            o.remove(k);
                        }
                    }
                    merge(&mut base, value);
                    base
                }
                None => {
                    err(
                        Severity::Error,
                        Some(file),
                        Some(id),
                        format!("extends {parent}, which doesn't exist"),
                    );
                    value.clone()
                }
            }
        }
        None => value.clone(),
    };
    resolved.insert(id.to_string(), out.clone());
    Some(out)
}

/// Strings in `value` that are ids of records (`npc/...`), outside prose fields.
fn collect_refs(value: &Value, key: Option<&str>, out: &mut BTreeSet<String>) {
    match value {
        Value::String(s)
            if !key.is_some_and(|k| PROSE.contains(&k))
                && kind_of(s).is_some()
                && !s.contains(' ') =>
        {
            out.insert(s.clone());
        }
        Value::Array(a) => a.iter().for_each(|v| collect_refs(v, key, out)),
        Value::Object(o) => {
            for (k, v) in o {
                // map keys can be ids too (arrival_from)
                if kind_of(k).is_some() && !k.contains(' ') {
                    out.insert(k.clone());
                }
                collect_refs(v, Some(k), out);
            }
        }
        _ => {}
    }
}

fn check_client_ids(
    reference: &Path,
    resolved: &BTreeMap<String, Value>,
    merged: &BTreeMap<String, (Value, PathBuf, Option<String>)>,
    err: &mut ErrFn,
) {
    let looks: HashSet<u64> = std::fs::read_to_string(reference.join("monster_data.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            v.get("monster_data")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|m| m.get("id")?.as_u64()).collect())
        })
        .unwrap_or_default();
    let effects: HashSet<u64> = std::fs::read_to_string(reference.join("effect_package_ids.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<u64>>(&t).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default();
    if looks.is_empty() {
        err(
            Severity::Warning,
            Some(&reference.join("monster_data.json")),
            None,
            "no look catalog: looks aren't checked".into(),
        );
        return;
    }
    for (id, value) in resolved {
        let mut bad = BTreeSet::new();
        find_numbers(value, "monster_data", &mut |n| {
            if !looks.contains(&n) {
                bad.insert(format!("look (monster data) {n}"));
            }
        });
        if !effects.is_empty() {
            find_numbers(value, "effect_package", &mut |n| {
                if !effects.contains(&n) {
                    bad.insert(format!("effect package {n}"));
                }
            });
        }
        for b in bad {
            err(
                Severity::Warning,
                Some(&merged[id].1),
                Some(id),
                format!("the client has no {b}"),
            );
        }
    }
}

fn find_numbers(value: &Value, key: &str, f: &mut dyn FnMut(u64)) {
    match value {
        Value::Array(a) => a.iter().for_each(|v| find_numbers(v, key, f)),
        Value::Object(o) => {
            for (k, v) in o {
                match v.as_u64() {
                    Some(n) if k == key => f(n),
                    _ => find_numbers(v, key, f),
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_patch() {
        let mut a = json!({ "a": 1, "b": { "c": 2, "d": 3 }, "l": [1, 2] });
        merge(&mut a, &json!({ "b": { "c": 5, "d": null }, "l": [9] }));
        assert_eq!(a, json!({ "a": 1, "b": { "c": 5 }, "l": [9] }));
    }

    #[test]
    fn features() {
        let on = vec!["live".to_string(), "feature/christmas".to_string()];
        assert!(feature_active("live", &on));
        assert!(feature_active("feature/christmas/2026", &on));
        assert!(!feature_active("feature/christmasy", &on));
        assert!(!feature_active("other/x", &on));
    }

    fn write(dir: &Path, rel: &str, v: Value) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }

    fn tree() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lmoxide-content-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        write(
            &dir,
            "content.json",
            json!({ "format": 1, "reference": "nowhere" }),
        );
        write(
            &dir,
            "tunables.json",
            serde_json::to_value(Tunables::default()).unwrap(),
        );
        write(
            &dir,
            "npcs/_ghost.json",
            json!({ "id": "npc/_ghost", "level": 4, "ai": { "aggro": 10.0 }, "loot": "loot/ghosts" }),
        );
        write(
            &dir,
            "npcs/specter.json",
            json!({ "id": "npc/specter", "extends": "npc/_ghost", "name": "Specter", "ai": { "prefer_ranged": true } }),
        );
        write(
            &dir,
            "loot/ghosts.json",
            json!({ "id": "loot/ghosts", "entries": [{ "weight": 1, "drop": { "stars": [1, 3] } }] }),
        );
        write(
            &dir,
            "worlds/4200-isle/world.json",
            json!({ "id": "world/isle", "playfield": 4200 }),
        );
        write(
            &dir,
            "worlds/4200-isle/spawns.json",
            json!({ "spawns": [{ "id": "spawn/4200/a", "kind": "enemy", "template": "npc/specter", "pos": [1, 2, 3], "overrides": { "level": 7 } }] }),
        );
        write(
            &dir,
            "feature/christmas/specter.json",
            json!({ "id": "npc/specter", "feature": "feature/christmas", "level": 9 }),
        );
        dir
    }

    #[test]
    fn loads_with_extends_features_and_overrides() {
        let dir = tree();
        let opts = Options {
            check_client_ids: false,
            ..Default::default()
        };
        let (c, problems) = load(&dir, &opts);
        assert!(problems.is_empty(), "{problems:?}");
        let s = &c.npcs["npc/specter"];
        assert_eq!(s.level, Some(4));
        assert_eq!(s.ai.aggro, Some(10.0));
        assert!(s.ai.prefer_ranged);
        assert_eq!(s.loot.as_deref(), Some("loot/ghosts"));
        assert_eq!(c.world_of["spawn/4200/a"], "world/isle");
        assert_eq!(
            c.spawn_npc(&c.spawns["spawn/4200/a"]).unwrap().level,
            Some(7)
        );

        let opts = Options {
            features: vec!["live".into(), "feature/christmas".into()],
            check_client_ids: false,
        };
        let (c, problems) = load(&dir, &opts);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(c.npcs["npc/specter"].level, Some(9));
        assert!(c.npcs["npc/specter"].ai.prefer_ranged);

        write(
            &dir,
            "npcs/bad.json",
            json!({ "id": "npc/bad", "loot": "loot/missing", "levle": 3 }),
        );
        let (_, problems) = load(
            &dir,
            &Options {
                check_client_ids: false,
                ..Default::default()
            },
        );
        let text: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
        assert!(text.iter().any(|t| t.contains("loot/missing")), "{text:?}");
        assert!(text.iter().any(|t| t.contains("levle")), "{text:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Rewrites every content file under `dir` in its canonical form (each record parsed as its
/// kind, so fields come out in their declared order); with `check_only`, writes nothing.
/// Returns the files that changed (or would).
pub fn format_tree(dir: &Path, check_only: bool) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let mut changed = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)?;
        let value: Value =
            serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        let rel = file.strip_prefix(dir).unwrap_or(&file);
        let canonical = match rel.to_str() {
            Some("content.json") => {
                crate::canon::to_string(&serde_json::from_value::<Manifest>(value)?)?
            }
            Some("tunables.json") => {
                crate::canon::to_string(&serde_json::from_value::<Tunables>(value)?)?
            }
            Some("texts.json") => {
                crate::canon::to_string(&serde_json::from_value::<Texts>(value)?)?
            }
            _ if rel.starts_with("schemas") => continue,
            _ => match value {
                Value::Object(ref o) if o.contains_key("id") => crate::canon::to_string(
                    &typed(value).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?,
                )?,
                Value::Object(o) => {
                    let mut out: BTreeMap<String, Vec<AnyRecord>> = BTreeMap::new();
                    for (section, records) in o {
                        let list = records.as_array().cloned().unwrap_or_default();
                        let typed: Result<Vec<AnyRecord>, String> =
                            list.into_iter().map(typed).collect();
                        out.insert(
                            section,
                            typed.map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?,
                        );
                    }
                    crate::canon::to_string(&out)?
                }
                _ => continue,
            },
        };
        if canonical != text {
            if !check_only {
                std::fs::write(&file, canonical)?;
            }
            changed.push(file);
        }
    }
    Ok(changed)
}

/// Any record, serialised as its own kind (declared field order).
#[derive(serde::Serialize)]
#[serde(untagged)]
pub(crate) enum AnyRecord {
    World(World),
    Spawn(Spawn),
    Portal(Portal),
    Bouncer(Bouncer),
    Area(Area),
    Trigger(Trigger),
    Puzzle(Puzzle),
    Script(Script),
    Region(Region),
    Npc(Npc),
    Faction(Faction),
    Object(Object),
    Skill(Skill),
    Buff(Buff),
    Loot(LootTable),
    Item(Item),
    Minifigure(Minifigure),
    Dialogue(Dialogue),
    Achievement(Achievement),
}

/// A record parsed as its kind (by its id).
pub(crate) fn typed(value: Value) -> Result<AnyRecord, String> {
    fn via<T: DeserializeOwned>(v: Value, f: fn(T) -> AnyRecord) -> Result<AnyRecord, String> {
        serde_json::from_value(v).map(f).map_err(|e| e.to_string())
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match kind_of(&id) {
        Some("world") => via(value, AnyRecord::World),
        Some("spawn") => via(value, AnyRecord::Spawn),
        Some("portal") => via(value, AnyRecord::Portal),
        Some("bouncer") => via(value, AnyRecord::Bouncer),
        Some("area") => via(value, AnyRecord::Area),
        Some("trigger") => via(value, AnyRecord::Trigger),
        Some("puzzle") => via(value, AnyRecord::Puzzle),
        Some("script") => via(value, AnyRecord::Script),
        Some("region") => via(value, AnyRecord::Region),
        Some("npc") => via(value, AnyRecord::Npc),
        Some("faction") => via(value, AnyRecord::Faction),
        Some("object") => via(value, AnyRecord::Object),
        Some("skill") => via(value, AnyRecord::Skill),
        Some("buff") => via(value, AnyRecord::Buff),
        Some("loot") => via(value, AnyRecord::Loot),
        Some("item") => via(value, AnyRecord::Item),
        Some("minifig") => via(value, AnyRecord::Minifigure),
        Some("dialogue") => via(value, AnyRecord::Dialogue),
        Some("achievement") => via(value, AnyRecord::Achievement),
        _ => Err(format!("unknown kind of id {id:?}")),
    }
}
