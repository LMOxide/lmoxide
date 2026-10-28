//! Bouncers (jump pads, jumppad.rs) as content records on the survey server: each world's
//! bouncer records are its pads; what editors change writes them.

use lmoxide_content::id::RecordKind;
use lmoxide_content::work::{Key, Work};
use lmoxide_content::{Bouncer, Rot, Source};
use lmoxide_survey::{Kind as SKind, Placement, apply::Category};
use serde_json::{Value, json};

use super::Error;
use crate::jumppad::RecordPad;

/// How a pad shows: surveyed ones are the editors'; the recorded session's landing points are
/// demo content; the rest guesses, to check.
fn category(b: &Bouncer) -> Category {
    match b.source {
        Some(Source::Surveyed | Source::Footage) => Category::Surveyed,
        _ if b.target_by.as_deref() == Some("demo") => Category::Demo,
        _ => Category::Guessed,
    }
}

/// Playfield `pf`'s pads from its bouncer records.
pub fn pads_of(work: &Work, pf: u32) -> Vec<RecordPad> {
    work.shown_in_world(RecordKind::Bouncer, pf)
        .iter()
        .filter_map(|key| {
            let features: Vec<String> = if key.feature == "live" {
                Vec::new()
            } else {
                vec![key.feature.clone()]
            };
            let b: Bouncer =
                serde_json::from_value(work.resolve(&key.id.to_string(), &features)?.0).ok()?;
            let template = b
                .template
                .as_deref()
                .and_then(|t| work.resolve(t, &features))
                .map(|(v, _)| v)
                .unwrap_or(Value::Null);
            let client = b.client.clone().unwrap_or_default();
            let visual = client.visual.or_else(|| {
                serde_json::from_value::<[u32; 2]>(template.pointer("/render/visual")?.clone()).ok()
            });
            let look = client.look.clone().or_else(|| {
                template
                    .pointer("/render/look")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            let heading = b.rot.map_or(0.0, Rot::heading);
            Some(RecordPad {
                id: key.id.to_string(),
                category: category(&b),
                placement: Placement {
                    id: key.id.to_string(),
                    kind: SKind::Bouncer,
                    name: b.name.clone(),
                    monster_data: None,
                    template: visual.map(|v| v[1]),
                    level: None,
                    pos: b.pos,
                    heading,
                    target: b.target,
                    replaces: None,
                    removed: false,
                    wave: None,
                    patrol: Vec::new(),
                    comment: b.comment.clone(),
                    tags: b.tags.clone(),
                    feature: key.feature.clone(),
                    author: b.author.clone(),
                    updated: b.updated,
                },
                visual,
                bounds: client.bounds,
                guid: client.guid,
                look,
                rot: match b.rot {
                    Some(Rot::Quat(q)) => Some(q),
                    _ => None,
                },
                apex: b.apex,
                target_by: b.target_by.clone(),
                tag: client.tag.clone(),
            })
        })
        .collect()
}

/// Puts playfield `pf`'s bouncer records in the world as its pads.
pub fn apply(work: &Work, pf: u32) {
    crate::jumppad::set_records(pf, pads_of(work, pf));
}

/// The bouncer template (an object of kind bouncer) of a pad look: one there is, else a new one.
fn template_for(work: &mut Work, visual: u32, author: &str, now: u64) -> Result<String, Error> {
    let found = work
        .records()
        .filter(|(k, _)| k.id.kind == RecordKind::Object && k.feature == "live")
        .find(|(_, e)| {
            e.value.get("kind").and_then(Value::as_str) == Some("bouncer")
                && e.value.pointer("/render/visual/1").and_then(Value::as_u64)
                    == Some(u64::from(visual))
        })
        .map(|(k, _)| k.id.to_string());
    if let Some(id) = found {
        return Ok(id);
    }
    let id = work.new_id("object", &format!("bouncer {visual}"), None);
    work.put(json!({ "id": id, "kind": "bouncer", "render": { "visual": [1_010_201, visual] }, "source": "surveyed" }), author, now)?;
    Ok(id)
}

/// Writes a bouncer's record from the editor's fields (`p`): its look (template), place,
/// heading, landing point, name, notes. A heading that didn't change keeps the record's
/// rotation as it was (a quaternion stays one).
pub fn store(
    work: &mut Work,
    key: &Key,
    p: &Placement,
    author: &str,
    now: u64,
) -> Result<(), Error> {
    let mut record = work
        .get(key)
        .map(|e| e.value.clone())
        .unwrap_or_else(|| json!({ "id": key.id.to_string() }));
    let current_visual = pads_of_key(work, key).and_then(|r| r.visual.map(|v| v[1]));
    if let Some(v) = p.template.filter(|v| Some(*v) != current_visual) {
        let t = template_for(work, v, author, now)?;
        record["template"] = json!(t);
        // (a recorded pad's own visual goes: its new look is the template's)
        if let Some(c) = record.get_mut("client").and_then(Value::as_object_mut) {
            c.remove("visual");
            c.remove("look");
        }
    }
    let r = record.as_object_mut().expect("a record");
    let same_heading = r
        .get("rot")
        .cloned()
        .and_then(|v| serde_json::from_value::<Rot>(v).ok())
        .is_some_and(|rot| (rot.heading() - p.heading).abs() < 1e-3);
    if !same_heading {
        r.insert("rot".into(), json!((p.heading * 1000.0).round() / 1000.0));
    }
    let set = |r: &mut serde_json::Map<String, Value>, k: &str, v: Value, empty: bool| {
        if empty {
            r.remove(k);
        } else {
            r.insert(k.into(), v);
        }
    };
    set(r, "name", json!(p.name), p.name.is_empty());
    r.insert("pos".into(), json!(p.pos));
    set(r, "target", json!(p.target), p.target.is_none());
    set(r, "comment", json!(p.comment), p.comment.is_empty());
    set(r, "tags", json!(p.tags), p.tags.is_empty());
    set(r, "feature", json!(key.feature), key.feature == "live");
    r.insert("source".into(), json!("surveyed"));
    work.put(record, author, now)?;
    Ok(())
}

fn pads_of_key(work: &Work, key: &Key) -> Option<RecordPad> {
    pads_of(work, key.id.world?)
        .into_iter()
        .find(|r| r.id == key.id.to_string())
}
