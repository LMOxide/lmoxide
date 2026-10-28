//! The world editor's survey views (lmoxide-survey's shapes: notes, scripts and their waves)
//! of content records, and back: a note is an area with role `note`, a script a script record
//! (its area an area record). The survey server answers and writes through these.

use serde_json::{Value, json};

use crate::id::{RecordId, RecordKind};
use std::collections::HashMap;

use crate::action::{Action, Destination};
use crate::model::{Achievement, Area, AreaRole, Once, Script, Shape, Trigger, WaveTrigger};
use crate::units::{Duration, Ratio};
use crate::work::{EditError, Key, Work};
use lmoxide_survey as sv;

/// A survey area as a content shape (polygons: their ground between a metre under the lowest
/// point and four above the highest).
pub fn shape_of(a: &sv::Area) -> Shape {
    match a {
        sv::Area::Point(p) => Shape::Point { pos: *p, rot: None },
        sv::Area::Circle { center, radius } => Shape::Circle {
            pos: *center,
            radius: *radius,
            height: None,
        },
        sv::Area::Polygon { points } => {
            let lo = points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
            let hi = points
                .iter()
                .map(|p| p[1])
                .fold(f32::NEG_INFINITY, f32::max);
            let (lo, hi) = if lo.is_finite() {
                (lo - 1.0, hi + 4.0)
            } else {
                (0.0, 0.0)
            };
            Shape::Polygon {
                points: points.iter().map(|p| [p[0], p[2]]).collect(),
                heights: [lo, hi],
            }
        }
    }
}

/// A content shape as a survey area (none for walls and ground spots).
pub fn area_of(s: &Shape) -> Option<sv::Area> {
    Some(match s {
        Shape::Point { pos, .. } => sv::Area::Point(*pos),
        Shape::Circle { pos, radius, .. } => sv::Area::Circle {
            center: *pos,
            radius: *radius,
        },
        Shape::Polygon { points, heights } => sv::Area::Polygon {
            points: points
                .iter()
                .map(|p| [p[0], heights[0] + 1.0, p[1]])
                .collect(),
        },
        Shape::Wall { .. } | Shape::Ground { .. } => return None,
    })
}

fn parse<T: serde::de::DeserializeOwned>(work: &Work, key: &Key) -> Option<T> {
    serde_json::from_value(work.get(key)?.value.clone()).ok()
}

fn review(s: Option<&str>) -> sv::Review {
    match s {
        Some("accepted") => sv::Review::Accepted,
        Some("rejected") => sv::Review::Rejected,
        _ => sv::Review::Unreviewed,
    }
}

fn review_str(r: sv::Review) -> &'static str {
    match r {
        sv::Review::Unreviewed => "unreviewed",
        sv::Review::Accepted => "accepted",
        sv::Review::Rejected => "rejected",
    }
}

/// A note area as a survey note.
pub fn note_view(work: &Work, key: &Key) -> Option<sv::Note> {
    let a: Area = parse(work, key)?;
    // (a script's own area is a note area without a note)
    let n = a.note.clone().filter(|_| a.role == AreaRole::Note)?;
    let kind: sv::NoteKind =
        serde_json::from_value(n["kind"].clone()).unwrap_or(sv::NoteKind::Comment);
    Some(sv::Note {
        id: key.id.to_string(),
        kind,
        area: a.shape.as_ref().and_then(area_of)?,
        reference: n["reference"].as_str().map(str::to_string),
        comment: a.comment,
        status: review(n["status"].as_str()),
        tags: a.tags,
        feature: key.feature.clone(),
        author: a.author,
        updated: a.updated,
    })
}

/// A script as the survey shows it (its area's shape, its waves' triggers).
pub fn script_view(work: &Work, key: &Key) -> Option<sv::Script> {
    let s: Script = parse(work, key)?;
    let area = s
        .area
        .as_deref()
        .and_then(|a| work.resolve(a, &[]))
        .and_then(|(v, _)| serde_json::from_value::<Area>(v).ok())
        .and_then(|a| a.shape)
        .and_then(|s| area_of(&s));
    Some(sv::Script {
        id: key.id.to_string(),
        name: s.name,
        area,
        reference: s.reference,
        comment: s.comment,
        waves: s
            .waves
            .into_iter()
            .map(|w| sv::Wave {
                id: w.id,
                name: w.name,
                trigger: w.trigger.map(|t| match t {
                    WaveTrigger::After(d) => sv::Trigger::After {
                        seconds: d.secs() as f32,
                    },
                    WaveTrigger::HealthBelow { spawn, share } => sv::Trigger::HealthBelow {
                        placement: spawn,
                        percent: (share.fraction() * 100.0) as f32,
                    },
                    WaveTrigger::EnterArea(a) => sv::Trigger::EnterArea { note: a },
                    WaveTrigger::WaveCleared(w) => sv::Trigger::WaveCleared { wave: w },
                    WaveTrigger::Other => sv::Trigger::Other,
                }),
                comment: w.comment,
                tags: w.tags,
            })
            .collect(),
        status: review(s.status.as_deref()),
        tags: s.tags,
        feature: key.feature.clone(),
        author: s.author,
        updated: s.updated,
    })
}

/// A world's notes and scripts.
pub fn notes_of(work: &Work, pf: u32) -> sv::NotesFile {
    let mut out = sv::NotesFile::new(pf);
    out.notes = work
        .shown_in_world(RecordKind::Area, pf)
        .iter()
        .filter_map(|k| note_view(work, k))
        .collect();
    out.scripts = work
        .shown_in_world(RecordKind::Script, pf)
        .iter()
        .filter_map(|k| script_view(work, k))
        .collect();
    out
}

/// The record a survey id names in playfield `pf` (a record id of that kind there), if it does.
fn key_of(id: &str, kind: RecordKind, pf: u32, feature: &str) -> Option<Key> {
    let rid: RecordId = id.parse().ok()?;
    (rid.kind == kind && rid.world == Some(pf)).then(|| Key::new(rid, feature))
}

fn set(r: &mut serde_json::Map<String, Value>, k: &str, v: Value, empty: bool) {
    if empty {
        r.remove(k);
    } else {
        r.insert(k.into(), v);
    }
}

fn shared(r: &mut serde_json::Map<String, Value>, comment: &str, tags: &[String], feature: &str) {
    set(r, "comment", json!(comment), comment.is_empty());
    set(r, "tags", json!(tags), tags.is_empty());
    set(
        r,
        "feature",
        json!(feature),
        feature == "live" || feature.is_empty(),
    );
    r.insert("source".into(), json!("surveyed"));
}

/// Moves a record to another feature: the new version is written, the old one goes.
fn moved(work: &mut Work, old: Option<Key>, new: &Key) -> Result<(), EditError> {
    match old {
        Some(o) if o != *new && work.get(&o).is_some() => work.reset(&o, None),
        _ => Ok(()),
    }
}

/// The record of the note's id there (any feature).
fn existing(work: &Work, id: &str) -> Option<Key> {
    work.versions(id).into_iter().map(|(k, _)| k.clone()).last()
}

/// Saves a survey note as its area (a new id when it has none of that kind yet): the note as
/// saved.
pub fn put_note(
    work: &mut Work,
    pf: u32,
    n: &sv::Note,
    author: &str,
    now: u64,
) -> Result<sv::Note, EditError> {
    let feature = if n.feature.is_empty() {
        "live".to_string()
    } else {
        n.feature.clone()
    };
    let old =
        existing(work, &n.id).filter(|k| k.id.kind == RecordKind::Area && k.id.world == Some(pf));
    let key = match key_of(&n.id, RecordKind::Area, pf, &feature) {
        Some(k) => k,
        None => {
            let name = n
                .reference
                .clone()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| format!("{:?}", n.kind).to_lowercase());
            Key::new(
                work.new_id("area", &format!("note {name}"), Some(pf))
                    .parse()
                    .map_err(|e| EditError::Invalid(vec![e]))?,
                &feature,
            )
        }
    };
    let mut record = old
        .as_ref()
        .and_then(|k| work.get(k))
        .map(|e| e.value.clone())
        .unwrap_or_else(|| json!({}));
    let r = record.as_object_mut().expect("a record");
    r.insert("id".into(), json!(key.id.to_string()));
    r.entry("name")
        .or_insert_with(|| json!(n.reference.clone().unwrap_or_default()));
    r.insert("role".into(), json!("note"));
    r.insert(
        "shape".into(),
        serde_json::to_value(shape_of(&n.area)).expect("shape"),
    );
    r.insert(
        "note".into(),
        json!({ "kind": n.kind, "reference": n.reference, "status": review_str(n.status) }),
    );
    shared(r, &n.comment, &n.tags, &feature);
    work.put(record, author, now)?;
    moved(work, old, &key)?;
    Ok(note_view(work, &key).expect("just saved"))
}

/// Saves a survey script as its record (a new id from its name when it has none yet), its
/// area as an area record of its own; waves keep what the survey view doesn't show (buffs,
/// actions). The script as saved.
pub fn put_script(
    work: &mut Work,
    pf: u32,
    s: &sv::Script,
    author: &str,
    now: u64,
) -> Result<sv::Script, EditError> {
    let feature = if s.feature.is_empty() {
        "live".to_string()
    } else {
        s.feature.clone()
    };
    let old =
        existing(work, &s.id).filter(|k| k.id.kind == RecordKind::Script && k.id.world == Some(pf));
    let key = match key_of(&s.id, RecordKind::Script, pf, &feature) {
        Some(k) => k,
        None => Key::new(
            work.new_id(
                "script",
                if s.name.trim().is_empty() {
                    "script"
                } else {
                    &s.name
                },
                Some(pf),
            )
            .parse()
            .map_err(|e| EditError::Invalid(vec![e]))?,
            &feature,
        ),
    };
    let mut record = old
        .as_ref()
        .and_then(|k| work.get(k))
        .map(|e| e.value.clone())
        .unwrap_or_else(|| json!({}));
    // its area
    let area_id = record
        .get("area")
        .and_then(Value::as_str)
        .map(str::to_string);
    let area_id = match &s.area {
        Some(a) => {
            let id = area_id
                .unwrap_or_else(|| work.new_id("area", &format!("{} area", s.name), Some(pf)));
            let area_key = Key::parse(&id, &feature).map_err(|e| EditError::Invalid(vec![e]))?;
            let mut area = work
                .get(&area_key)
                .map(|e| e.value.clone())
                .unwrap_or_else(|| json!({ "id": id, "name": s.name, "role": "place" }));
            area["shape"] = serde_json::to_value(shape_of(a)).expect("shape");
            if feature != "live" {
                area["feature"] = json!(feature);
            }
            work.put(area, author, now)?;
            Some(id)
        }
        None => None,
    };
    let old_waves: Vec<Value> = record
        .get("waves")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let waves: Vec<Value> = s
        .waves
        .iter()
        .map(|w| {
            let mut v = old_waves
                .iter()
                .find(|o| o["id"] == json!(w.id))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let o = v.as_object_mut().expect("a wave");
            o.insert("id".into(), json!(w.id));
            set(o, "name", json!(w.name), w.name.is_empty());
            let trigger = w.trigger.as_ref().map(|t| match t {
                sv::Trigger::After { seconds } => WaveTrigger::After(Duration(f64::from(*seconds))),
                sv::Trigger::HealthBelow { placement, percent } => WaveTrigger::HealthBelow {
                    spawn: placement.clone(),
                    share: Ratio(f64::from(*percent) / 100.0),
                },
                sv::Trigger::EnterArea { note } => WaveTrigger::EnterArea(note.clone()),
                sv::Trigger::WaveCleared { wave } => WaveTrigger::WaveCleared(wave.clone()),
                sv::Trigger::Other => WaveTrigger::Other,
            });
            set(
                o,
                "trigger",
                serde_json::to_value(&trigger).expect("trigger"),
                trigger.is_none(),
            );
            set(o, "comment", json!(w.comment), w.comment.is_empty());
            set(o, "tags", json!(w.tags), w.tags.is_empty());
            v
        })
        .collect();
    let r = record.as_object_mut().expect("a record");
    r.insert("id".into(), json!(key.id.to_string()));
    r.insert("name".into(), json!(s.name));
    set(r, "area", json!(area_id), area_id.is_none());
    set(
        r,
        "reference",
        json!(s.reference),
        s.reference.as_deref().is_none_or(str::is_empty),
    );
    r.insert("status".into(), json!(review_str(s.status)));
    set(r, "waves", json!(waves), waves.is_empty());
    shared(r, &s.comment, &s.tags, &feature);
    work.put(record, author, now)?;
    moved(work, old, &key)?;
    Ok(script_view(work, &key).expect("just saved"))
}

/// What trigger walls name by client id, by record: achievements by lore id, dialogues by
/// their first text, worlds by playfield.
pub struct Lookups {
    achievement: HashMap<u32, String>,
    lore: HashMap<String, u32>,
    dialogue: HashMap<u32, String>,
    first_text: HashMap<String, u32>,
    world: HashMap<u32, String>,
    playfield: HashMap<String, u32>,
}

impl Lookups {
    pub fn new(work: &Work) -> Self {
        let mut l = Lookups { achievement: HashMap::new(), lore: HashMap::new(), dialogue: HashMap::new(), first_text: HashMap::new(), world: HashMap::new(), playfield: HashMap::new() };
        for (k, e) in work.records().filter(|(k, _)| k.feature == "live") {
            let id = k.id.to_string();
            let num = |p: &str| e.value.pointer(p).and_then(Value::as_u64).map(|n| n as u32);
            match k.id.kind {
                RecordKind::Achievement => {
                    if let Some(n) = num("/lore") {
                        l.achievement.insert(n, id.clone());
                        l.lore.insert(id, n);
                    }
                }
                RecordKind::Dialogue => {
                    if let Some(n) = num("/texts/0") {
                        l.dialogue.entry(n).or_insert_with(|| id.clone());
                        l.first_text.insert(id, n);
                    }
                }
                RecordKind::World => {
                    if let Some(n) = num("/playfield") {
                        l.world.insert(n, id.clone());
                        l.playfield.insert(id, n);
                    }
                }
                _ => {}
            }
        }
        l
    }
}

const NOT_CHOSEN: &str = " (not chosen yet)";

/// A wall's action as a content action.
pub fn action_of(a: &sv::WallAction, l: &Lookups) -> Action {
    let other = |what: &str| Action::Other { comment: format!("{what}{NOT_CHOSEN}") };
    match a {
        sv::WallAction::Script { script: Some(s), wave } => Action::StartScript { script: s.clone(), wave: wave.clone() },
        sv::WallAction::Script { script: None, .. } => other("start a script"),
        sv::WallAction::Discover { name, achievement } => Action::Discover {
            location: name.clone(),
            achievement: achievement.as_ref().and_then(|a| a.parse::<u32>().ok()).and_then(|n| l.achievement.get(&n).cloned()),
        },
        sv::WallAction::Dialog { dialog: Some(d) } => match d.parse::<u32>().ok().and_then(|t| l.dialogue.get(&t)) {
            Some(id) => Action::Dialogue { dialogue: id.clone() },
            None => Action::Other { comment: format!("dialogue {d}") },
        },
        sv::WallAction::Dialog { dialog: None } => other("open a dialogue"),
        sv::WallAction::Portal { window: Some(w), .. } => Action::OpenWindow { window: *w },
        sv::WallAction::Portal { window: None, to: Some(pf) } => match l.world.get(pf) {
            Some(w) => Action::Teleport { to: Destination { world: Some(w.clone()), ..Default::default() } },
            None => Action::Other { comment: format!("a portal to playfield {pf}") },
        },
        sv::WallAction::Portal { .. } => other("a portal"),
        sv::WallAction::Message { text } => Action::Message { text: text.clone() },
        sv::WallAction::Collect { drop: Some(k) } => Action::Collect { key: k.clone() },
        sv::WallAction::Collect { drop: None } => other("give a collectible"),
        sv::WallAction::Other => Action::Other { comment: String::new() },
    }
}

/// A content action as a wall's (what walls can't do: other).
pub fn wall_action(a: &Action, l: &Lookups) -> sv::WallAction {
    match a {
        Action::StartScript { script, wave } => sv::WallAction::Script { script: Some(script.clone()), wave: wave.clone() },
        Action::Discover { location, achievement } => sv::WallAction::Discover { name: location.clone(), achievement: achievement.as_ref().and_then(|a| l.lore.get(a)).map(|n| n.to_string()) },
        Action::Dialogue { dialogue } => sv::WallAction::Dialog { dialog: l.first_text.get(dialogue).map(|n| n.to_string()) },
        Action::OpenWindow { window } => sv::WallAction::Portal { window: Some(*window), to: None },
        Action::Teleport { to } => sv::WallAction::Portal { window: None, to: to.world.as_ref().and_then(|w| l.playfield.get(w)).copied() },
        Action::Message { text } => sv::WallAction::Message { text: text.clone() },
        Action::Collect { key } => sv::WallAction::Collect { drop: Some(key.clone()) },
        Action::Other { comment } if comment.ends_with(NOT_CHOSEN) => match comment.trim_end_matches(NOT_CHOSEN) {
            "start a script" => sv::WallAction::Script { script: None, wave: None },
            "open a dialogue" => sv::WallAction::Dialog { dialog: None },
            "a portal" => sv::WallAction::Portal { window: None, to: None },
            "give a collectible" => sv::WallAction::Collect { drop: None },
            _ => sv::WallAction::Other,
        },
        _ => sv::WallAction::Other,
    }
}

/// A trigger as a wall (walls and radii with one action; others aren't walls).
pub fn wall_view(work: &Work, key: &Key, l: &Lookups) -> Option<sv::TriggerWall> {
    let t: Trigger = parse(work, key)?;
    if t.actions.len() > 1 {
        return None;
    }
    let (pos, heading, size, radius) = match t.shape? {
        Shape::Wall { pos, heading, size } => (pos, heading, size, None),
        Shape::Circle { pos, radius, height } => (pos, 0.0, [radius * 2.0, height.unwrap_or(3.0), radius * 2.0], Some(radius)),
        _ => return None,
    };
    Some(sv::TriggerWall {
        id: key.id.to_string(),
        name: t.name,
        pos,
        heading,
        size,
        radius,
        color: t.look.unwrap_or_else(|| "green".into()),
        action: t.actions.first().map_or(sv::WallAction::Other, |a| wall_action(a, l)),
        once: t.once != Once::Never,
        comment: t.comment,
        removed: false,
        tags: t.tags,
        feature: key.feature.clone(),
        author: t.author,
        updated: t.updated,
    })
}

/// A world's trigger walls.
pub fn walls_of(work: &Work, pf: u32) -> Vec<sv::TriggerWall> {
    let l = Lookups::new(work);
    work.shown_in_world(RecordKind::Trigger, pf).iter().filter_map(|k| wall_view(work, k, &l)).collect()
}

/// Saves a wall as its trigger (a new id from its name when it has none yet): the wall as saved.
pub fn put_wall(work: &mut Work, pf: u32, w: &sv::TriggerWall, author: &str, now: u64) -> Result<sv::TriggerWall, EditError> {
    let feature = if w.feature.is_empty() { "live".to_string() } else { w.feature.clone() };
    let old = existing(work, &w.id).filter(|k| k.id.kind == RecordKind::Trigger && k.id.world == Some(pf));
    let key = match key_of(&w.id, RecordKind::Trigger, pf, &feature) {
        Some(k) => k,
        None => Key::new(work.new_id("trigger", if w.name.trim().is_empty() { "trigger" } else { &w.name }, Some(pf)).parse().map_err(|e| EditError::Invalid(vec![e]))?, &feature),
    };
    let l = Lookups::new(work);
    let mut record = old.as_ref().and_then(|k| work.get(k)).map(|e| e.value.clone()).unwrap_or_else(|| json!({}));
    let shape = match w.radius {
        Some(radius) => Shape::Circle { pos: w.pos, radius, height: Some(w.size[1]) },
        None => Shape::Wall { pos: w.pos, heading: w.heading, size: w.size },
    };
    let r = record.as_object_mut().expect("a record");
    r.insert("id".into(), json!(key.id.to_string()));
    set(r, "name", json!(w.name), w.name.is_empty());
    r.insert("shape".into(), serde_json::to_value(shape).expect("shape"));
    set(r, "once", json!("per_player"), !w.once);
    r.insert("look".into(), json!(w.color));
    r.insert("actions".into(), json!([serde_json::to_value(action_of(&w.action, &l)).expect("action")]));
    shared(r, &w.comment, &w.tags, &feature);
    work.put(record, author, now)?;
    moved(work, old, &key)?;
    Ok(wall_view(work, &key, &l).expect("just saved"))
}

/// The achievement record a smash rule key belongs to: an achievement's lore id names it, any
/// other key (`category:`, `drop:`, `find:`, `minifigure:`) a rule-only record by `client_name`.
fn rule_record(work: &Work, key: &str) -> Option<Key> {
    let lore: Option<u64> = key.parse().ok();
    work.records()
        .filter(|(k, _)| k.id.kind == RecordKind::Achievement && k.feature == "live")
        .find(|(_, e)| match lore {
            Some(n) => e.value.get("lore").and_then(Value::as_u64) == Some(n),
            None => e.value.get("lore").is_none() && e.value.get("client_name").and_then(Value::as_str) == Some(key),
        })
        .map(|(k, _)| k.clone())
}

fn rule_view(key: String, a: Achievement) -> Option<sv::SmashRule> {
    let r = a.rule?;
    Some(sv::SmashRule { id: key, enemies: r.enemies, once: r.once, item: r.item, comment: r.comment, tags: a.tags, feature: a.feature, author: a.author, updated: a.updated })
}

/// Every smash rule (achievements' and rule-only records').
pub fn rules(work: &Work) -> Vec<sv::SmashRule> {
    work.records()
        .filter(|(k, _)| k.id.kind == RecordKind::Achievement && k.feature == "live")
        .filter_map(|(_, e)| {
            let a: Achievement = serde_json::from_value(e.value.clone()).ok()?;
            let key = a.lore.map(|n| n.to_string()).or_else(|| a.client_name.clone())?;
            rule_view(key, a)
        })
        .collect()
}

/// Saves a smash rule on its achievement record (a rule-only record for other keys, made when
/// missing): the rule as saved.
pub fn put_rule(work: &mut Work, rule: &sv::SmashRule, author: &str, now: u64) -> Result<sv::SmashRule, EditError> {
    let mut record = match rule_record(work, &rule.id) {
        Some(k) => work.get(&k).expect("found").value.clone(),
        None if rule.id.parse::<u32>().is_ok() => return Err(EditError::NotFound(format!("no achievement with lore id {}", rule.id))),
        None => {
            let kind = rule.id.split(':').next().unwrap_or("rule");
            json!({ "id": work.new_id("achievement", &rule.id.replace(':', " "), None), "client_name": rule.id, "kind": kind, "source": "surveyed" })
        }
    };
    let r = record.as_object_mut().expect("a record");
    r.insert("rule".into(), json!({ "enemies": rule.enemies, "once": rule.once, "item": rule.item, "comment": rule.comment }));
    set(r, "tags", json!(rule.tags), rule.tags.is_empty());
    let saved = work.put(record, author, now)?;
    let a: Achievement = serde_json::from_value(saved).map_err(|e| EditError::Invalid(vec![e.to_string()]))?;
    Ok(rule_view(rule.id.clone(), a).expect("just saved"))
}

/// Removes a smash rule: from its achievement, or its rule-only record.
pub fn delete_rule(work: &mut Work, key: &str, author: &str, now: u64) -> Result<(), EditError> {
    let Some(k) = rule_record(work, key) else { return Ok(()) };
    let mut record = work.get(&k).expect("found").value.clone();
    if record.get("lore").is_none() {
        work.delete(&k, None, author, now)?;
        return Ok(());
    }
    record.as_object_mut().expect("a record").remove("rule");
    work.put(record, author, now)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon;

    fn tree() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lmoxide-views-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("worlds/4200-isle")).unwrap();
        std::fs::write(
            dir.join("content.json"),
            canon::to_string(&json!({ "format": 1, "reference": "../data" })).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("worlds/4200-isle/world.json"),
            canon::to_string(&json!({ "id": "world/isle", "playfield": 4200 })).unwrap(),
        )
        .unwrap();
        crate::load::format_tree(&dir, false).unwrap();
        dir
    }

    #[test]
    fn notes_and_scripts_round_trip() {
        let dir = tree();
        let mut work = Work::open(&dir).unwrap();
        let note = sv::Note {
            id: "n-web-1".into(),
            kind: sv::NoteKind::Dialog,
            area: sv::Area::Circle {
                center: [1.0, 2.0, 3.0],
                radius: 4.0,
            },
            reference: Some("Captain".into()),
            comment: "talks".into(),
            status: sv::Review::Accepted,
            tags: vec!["todo".into()],
            feature: "live".into(),
            author: String::new(),
            updated: 0,
        };
        let saved = put_note(&mut work, 4200, &note, "ann", 5).unwrap();
        assert_eq!(saved.id, "area/4200/note-captain");
        assert_eq!(
            (
                saved.kind,
                saved.area.clone(),
                saved.status,
                saved.author.as_str()
            ),
            (note.kind, note.area.clone(), note.status, "ann")
        );
        let script = sv::Script {
            id: "s-web-2".into(),
            name: "Ambush".into(),
            area: Some(sv::Area::Point([5.0, 6.0, 7.0])),
            reference: None,
            comment: String::new(),
            waves: vec![
                sv::Wave {
                    id: "w1".into(),
                    name: "first".into(),
                    trigger: Some(sv::Trigger::After { seconds: 1.5 }),
                    comment: String::new(),
                    tags: vec![],
                },
                sv::Wave {
                    id: "w2".into(),
                    name: String::new(),
                    trigger: Some(sv::Trigger::EnterArea {
                        note: Some(saved.id.clone()),
                    }),
                    comment: String::new(),
                    tags: vec![],
                },
            ],
            status: sv::Review::Unreviewed,
            tags: vec![],
            feature: "live".into(),
            author: String::new(),
            updated: 0,
        };
        let s = put_script(&mut work, 4200, &script, "ann", 6).unwrap();
        assert_eq!(s.id, "script/4200/ambush");
        assert_eq!((s.area.clone(), s.waves.len()), (script.area.clone(), 2));
        assert_eq!(
            s.waves[0].trigger,
            Some(sv::Trigger::After { seconds: 1.5 })
        );
        // saved again under its id: the same records
        let again = put_script(
            &mut work,
            4200,
            &sv::Script {
                name: "Ambush!".into(),
                ..s.clone()
            },
            "bob",
            7,
        )
        .unwrap();
        assert_eq!(
            (
                again.id.as_str(),
                again.name.as_str(),
                again.author.as_str()
            ),
            ("script/4200/ambush", "Ambush!", "bob")
        );
        let all = notes_of(&work, 4200);
        assert_eq!(
            (all.notes.len(), all.scripts.len()),
            (1, 1),
            "the script's own area isn't a note: {:?}",
            all.notes
        );
        // a wall starting the script's second wave
        let wall = sv::TriggerWall {
            id: "t-web-3".into(),
            name: "Gate".into(),
            pos: [1.0, 2.0, 3.0],
            heading: 0.5,
            size: [6.0, 3.0, 1.0],
            radius: None,
            color: "red".into(),
            action: sv::WallAction::Script { script: Some(s.id.clone()), wave: Some("w2".into()) },
            once: true,
            comment: String::new(),
            removed: false,
            tags: vec![],
            feature: "live".into(),
            author: String::new(),
            updated: 0,
        };
        let w = put_wall(&mut work, 4200, &wall, "ann", 8).unwrap();
        assert_eq!(w.id, "trigger/4200/gate");
        assert_eq!((w.action.clone(), w.once, w.size, w.heading), (wall.action.clone(), true, wall.size, 0.5));
        let w = put_wall(&mut work, 4200, &sv::TriggerWall { radius: Some(2.0), action: sv::WallAction::Dialog { dialog: None }, ..w }, "ann", 9).unwrap();
        assert_eq!((w.radius, w.action.clone()), (Some(2.0), sv::WallAction::Dialog { dialog: None }));
        assert_eq!(walls_of(&work, 4200).len(), 1);
        // smash rules: a rule-only record for a category, made and removed
        let rule = sv::SmashRule { id: "category:pirates".into(), enemies: vec!["Pirate".into()], once: false, item: None, comment: "all of them".into(), tags: vec![], feature: "live".into(), author: String::new(), updated: 0 };
        let saved = put_rule(&mut work, &rule, "ann", 10).unwrap();
        assert_eq!((saved.enemies.clone(), saved.comment.as_str()), (rule.enemies.clone(), "all of them"));
        assert_eq!(rules(&work).len(), 1);
        delete_rule(&mut work, "category:pirates", "ann", 11).unwrap();
        assert!(rules(&work).is_empty());
        assert!(crate::load::format_tree(&dir, true).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
