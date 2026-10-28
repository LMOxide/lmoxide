//! The survey server's objects as content records (docs/WORLD-EDITOR-CONTENT.md): every spawn of
//! a world in the working copy is an object, named by its record id; what editors do to one in
//! game writes its record.

use lmoxide_content::id::{RecordId, RecordKind};
use lmoxide_content::work::{EditError, Key, Work};
use lmoxide_content::{Source, Spawn, SpawnKind};
use lmoxide_survey::{Kind as SKind, WaveRef, Category};
use serde_json::{Value, json};

use super::{Error, Object, Placeable, bare_def, health_at, nkind, walks};
use crate::npc::{Kind, SpawnDef};

impl From<EditError> for Error {
    fn from(e: EditError) -> Self {
        match e {
            EditError::NotFound(m) => Error::NotFound(m),
            EditError::Referenced(users) => Error::Invalid(format!(
                "other records name it: {} (change them first, or delete it on its page in Definitions)",
                users
                    .iter()
                    .take(5)
                    .map(|u| u.key.id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            e => Error::Invalid(e.to_string()),
        }
    }
}

/// How the world editor shows a record by where it came from: guesses can be confirmed, edited
/// and deleted; recorded content only hidden.
pub fn category(source: Option<Source>) -> Category {
    match source {
        Some(Source::Proposed | Source::Server | Source::Guess) => Category::Guessed,
        Some(Source::Live) => Category::Live,
        Some(Source::Demo) => Category::Demo,
        Some(Source::Surveyed | Source::Footage) => Category::Surveyed,
        Some(Source::Client) | None => Category::Client,
    }
}

fn spawn_kind(k: SKind) -> SpawnKind {
    match k {
        SKind::Enemy => SpawnKind::Enemy,
        SKind::Npc => SpawnKind::Npc,
        SKind::Smashable => SpawnKind::Smashable,
        SKind::Buildie => SpawnKind::Buildie,
        _ => SpawnKind::Interactable,
    }
}

/// A spawn record as an object: its template (with the features of its version) and
/// overrides applied, as the game spawns it.
pub fn object_of(work: &Work, key: &Key) -> Option<Object> {
    let features: Vec<String> = if key.feature == "live" {
        Vec::new()
    } else {
        vec![key.feature.clone()]
    };
    let (value, _) = work.resolve(&key.id.to_string(), &features)?;
    let s: Spawn = serde_json::from_value(value).ok()?;
    let mut t = work.resolve(s.template.as_deref()?, &features)?.0;
    if let Some(o) = &s.overrides {
        lmoxide_content::load::merge(&mut t, o);
    }
    let c = crate::gamedata::get();
    let spell_of = |a: &str| {
        work.resolve(a, &features)
            .and_then(|(v, _)| v.pointer("/client/spell").and_then(Value::as_u64))
            .map(|n| n as u32)
    };
    let mut def = crate::spawns::def_from(&s, &t, &spell_of, &|kind, loot| {
        crate::spawns::own_loot(c, kind, loot)
    })?;
    def.rot = Some(def.rot.unwrap_or(0.0));
    Some(Object {
        id: key.id.to_string(),
        key: key.clone(),
        template: s.template.clone(),
        playfield: key.id.world?,
        category: category(s.source),
        def,
        wave: s.wave.map(|w| WaveRef {
            script: w.script,
            wave: w.wave,
        }),
        replaces: None,
        comment: s.comment,
        feature: key.feature.clone(),
        tags: s.tags,
        author: s.author,
        updated: s.updated,
        npc: None,
    })
}

/// Every object of playfield `pf` in the working copy (spawns the game spawns: enemies, NPCs,
/// smashables), in file order.
pub fn objects_of(work: &Work, pf: u32) -> Vec<Object> {
    work.shown_in_world(RecordKind::Spawn, pf)
        .iter()
        .filter_map(|k| object_of(work, k))
        .collect()
}

/// The template record of something placeable: the one objects of that look use, else a
/// template of that look, else a new one (made in the working copy).
pub fn template_for(
    work: &mut Work,
    objects: &[&Object],
    what: &Placeable,
    author: &str,
    now: u64,
) -> Result<String, Error> {
    let same = |o: &&&Object| {
        super::skind(o.def.kind) == what.kind
            && super::nonzero(o.def.monster_data) == what.monster_data
            && super::nonzero(o.def.template) == what.template
    };
    if let Some(t) = objects
        .iter()
        .filter(same)
        .find(|o| o.def.name == what.name)
        .or_else(|| objects.iter().find(same))
        .and_then(|o| o.template.clone())
    {
        return Ok(t);
    }
    let npc = matches!(what.kind, SKind::Enemy | SKind::Npc);
    let kind = if npc {
        RecordKind::Npc
    } else {
        RecordKind::Object
    };
    let look = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_u64).map(|n| n as u32);
    if let Some((k, _)) = work
        .records()
        .filter(|(k, _)| k.id.kind == kind && k.feature == "live")
        .find(|(_, e)| {
            look(&e.value, "/render/monster_data") == what.monster_data
                && (npc || look(&e.value, "/render/template") == what.template)
        })
    {
        return Ok(k.id.to_string());
    }
    // a look nothing uses yet: its own template
    let level = what.level.unwrap_or(1).max(1);
    let def = bare_def(
        nkind(what.kind)?,
        what.monster_data.unwrap_or(0),
        what.template.unwrap_or(0),
        level,
    );
    let id = work.new_id(kind.as_str(), &what.name, None);
    let record = if npc {
        json!({ "id": id, "name": what.name, "kind": if what.kind == SKind::Npc { "npc" } else { "enemy" }, "render": { "monster_data": what.monster_data }, "level": level, "stats": { "health": def.health }, "source": "surveyed" })
    } else {
        let okind = match what.kind {
            SKind::Buildie => "buildie",
            SKind::Interactable => "interactable",
            _ => "smashable",
        };
        json!({ "id": id, "name": what.name, "kind": okind, "render": { "template": what.template, "monster_data": what.monster_data }, "level": level, "health": def.health, "source": "surveyed" })
    };
    work.put(record, author, now)?;
    Ok(id)
}

/// Writes an object into its spawn record (the fields the world editor edits; the rest of the
/// record stays): a level other than its template's goes in `overrides` with its health.
pub fn store(work: &mut Work, o: &Object, author: &str, now: u64) -> Result<Value, Error> {
    let mut record = work
        .get(&o.key)
        .map(|e| e.value.clone())
        .unwrap_or_else(|| json!({ "id": o.id }));
    let r = record.as_object_mut().expect("a record");
    let kind = spawn_kind(super::skind(o.def.kind));
    r.insert("kind".into(), serde_json::to_value(kind).expect("kind"));
    r.insert("name".into(), json!(o.def.name));
    if let Some(t) = &o.template {
        r.insert("template".into(), json!(t));
    }
    r.insert("pos".into(), json!(o.def.pos));
    r.insert(
        "rot".into(),
        json!((o.def.rot.unwrap_or(0.0) * 1000.0).round() / 1000.0),
    );
    let set_or_drop = |r: &mut serde_json::Map<String, Value>, k: &str, v: Value, empty: bool| {
        if empty {
            r.remove(k);
        } else {
            r.insert(k.into(), v);
        }
    };
    set_or_drop(
        r,
        "patrol",
        json!(o.def.patrol),
        o.def.patrol.is_empty() || !walks(o.def.kind),
    );
    set_or_drop(
        r,
        "wave",
        json!(
            o.wave
                .as_ref()
                .map(|w| json!({ "script": w.script, "wave": w.wave }))
        ),
        o.wave.is_none(),
    );
    set_or_drop(r, "comment", json!(o.comment), o.comment.is_empty());
    set_or_drop(r, "tags", json!(o.tags), o.tags.is_empty());
    if o.feature == "live" {
        r.remove("feature");
    } else {
        r.insert("feature".into(), json!(o.feature));
    }
    r.insert("source".into(), json!("surveyed"));
    // its level, when it isn't its template's
    let template_level = o
        .template
        .as_deref()
        .and_then(|t| work.resolve(t, &[]))
        .and_then(|(v, _)| v.get("level").and_then(Value::as_u64))
        .unwrap_or(1) as u32;
    let mut overrides = r.get("overrides").cloned().unwrap_or_else(|| json!({}));
    if let Value::Object(ov) = &mut overrides {
        if o.def.level != template_level {
            ov.insert("level".into(), json!(o.def.level));
            if o.def.kind != Kind::Smashable {
                ov.entry("stats").or_insert_with(|| json!({}))["health"] = json!(o.def.health);
            }
        } else {
            ov.remove("level");
            if let Some(Value::Object(st)) = ov.get_mut("stats") {
                st.remove("health");
            }
            if ov
                .get("stats")
                .is_some_and(|s| s.as_object().is_some_and(|s| s.is_empty()))
            {
                ov.remove("stats");
            }
        }
    }
    set_or_drop(
        r,
        "overrides",
        overrides.clone(),
        overrides.as_object().is_none_or(|o| o.is_empty()),
    );
    Ok(work.put(record, author, now)?)
}

/// A new spawn's key in playfield `pf`, from its name (numbered when taken).
pub fn new_key(work: &Work, pf: u32, name: &str, feature: &str) -> Key {
    let id: RecordId = work
        .new_id(
            "spawn",
            if name.trim().is_empty() {
                "spawn"
            } else {
                name
            },
            Some(pf),
        )
        .parse()
        .expect("an id");
    Key::new(id, feature)
}

/// A spawn made from a placeable look, at a place (what `place` and pastes make).
pub fn new_object(
    key: Key,
    template: String,
    what: &Placeable,
    def: Option<SpawnDef>,
) -> Result<Object, Error> {
    let kind = nkind(what.kind)?;
    let mut def = def.unwrap_or_else(|| {
        bare_def(
            kind,
            what.monster_data.unwrap_or(0),
            what.template.unwrap_or(0),
            what.level.unwrap_or(1),
        )
    });
    let level = what.level.unwrap_or(def.level).max(1);
    def.health = health_at(&def, level);
    def.level = level;
    def.kind = if kind == Kind::Npc && def.kind == Kind::Fighter {
        Kind::Fighter
    } else {
        kind
    };
    if !what.name.is_empty() {
        def.name = what.name.clone();
    }
    def.patrol.clear();
    Ok(Object {
        id: key.id.to_string(),
        feature: key.feature.clone(),
        key,
        template: Some(template),
        playfield: 0,
        category: Category::Surveyed,
        def,
        wave: None,
        replaces: None,
        comment: String::new(),
        tags: Vec::new(),
        author: String::new(),
        updated: 0,
        npc: None,
    })
}

/// Playfield `pf`'s spawns of script waves, as scripts.rs runs them (play servers: they spawn
/// when their wave starts).
pub(crate) fn wave_spawns(work: &Work, pf: u32) -> Vec<(lmoxide_survey::Placement, SpawnDef)> {
    objects_of(work, pf)
        .into_iter()
        .filter(|o| o.wave.is_some())
        .map(|o| {
            let p = lmoxide_survey::Placement {
                id: o.id.clone(),
                kind: super::skind(o.def.kind),
                name: o.def.name.clone(),
                monster_data: super::nonzero(o.def.monster_data),
                template: super::nonzero(o.def.template),
                level: Some(o.def.level),
                pos: o.def.pos,
                heading: o.def.rot.unwrap_or(0.0),
                target: None,
                replaces: None,
                removed: false,
                wave: o.wave.clone(),
                patrol: o.def.patrol.clone(),
                comment: o.comment.clone(),
                tags: o.tags.clone(),
                feature: o.feature.clone(),
                author: o.author.clone(),
                updated: o.updated,
            };
            (p, o.def)
        })
        .collect()
}
