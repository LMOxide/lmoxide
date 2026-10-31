//! Once: the survey set's records (placements, bouncers, trigger walls, scripts, notes, smash
//! rules) into the content working copy, where the world editor works now
//! (docs/WORLD-EDITOR-CONTENT.md, phase 6). Records the content tree has already (imported from
//! the published set: `spawn/<pf>/<placement id>` ...) are updated when the set's are newer.
//! Placements that replaced a guess: the guess goes. The set's folder stays as it is, with
//! `migrated.json` saying what was done.

use std::collections::HashMap;
use std::path::Path;

use lmoxide_content::id::RecordKind;
use lmoxide_content::views;
use lmoxide_content::work::{Key, Work};
use lmoxide_survey::{Kind as SKind, set::Set};
use serde_json::{Value, json};
use tracing::{info, warn};

use super::{Object, Placeable, bouncers, records, template_of, walks};
use crate::npc::SpawnDef;

#[derive(Default, serde::Serialize)]
struct Report {
    spawns: usize,
    bouncers: usize,
    walls: usize,
    scripts: usize,
    notes: usize,
    rules: usize,
    removed: usize,
    replaced_guesses: usize,
    problems: Vec<String>,
}

fn updated(work: &Work, key: &Key) -> Option<u64> {
    work.get(key)
        .map(|e| e.value.get("updated").and_then(Value::as_u64).unwrap_or(0))
}

/// Takes the set in `dir` into the working copy, unless that was done (its `migrated.json`).
pub fn run(dir: &Path, work: &mut Work, catalog: &[(Placeable, SpawnDef)], objects: &[&Object]) {
    let marker = dir.join("migrated.json");
    if marker.exists() || !dir.join("set.json").exists() {
        return;
    }
    let set = match Set::load(dir) {
        Ok(s) => s,
        Err(e) => {
            warn!("survey set {}: {e}; not migrated", dir.display());
            return;
        }
    };
    let mut r = Report::default();
    let now = super::ids::now();
    let problem = |r: &mut Report, what: String| {
        warn!("migrating the survey set: {what}");
        r.problems.push(what);
    };

    // scripts first (placements name them): the content's script of that name, else a new one
    let mut scripts: HashMap<String, String> = HashMap::new();
    for (pf, n) in &set.notes {
        for s in &n.scripts {
            let existing = work
                .shown_in_world(RecordKind::Script, *pf)
                .into_iter()
                .find(|k| {
                    work.get(k)
                        .and_then(|e| e.value.get("name"))
                        .and_then(Value::as_str)
                        == Some(s.name.as_str())
                });
            let id = existing
                .as_ref()
                .map_or_else(|| s.id.clone(), |k| k.id.to_string());
            if existing
                .as_ref()
                .is_none_or(|k| updated(work, k).unwrap_or(0) < s.updated)
            {
                match views::put_script(
                    work,
                    *pf,
                    &lmoxide_survey::Script { id, ..s.clone() },
                    &s.author,
                    s.updated.max(1),
                ) {
                    Ok(saved) => {
                        scripts.insert(s.id.clone(), saved.id);
                        r.scripts += 1;
                    }
                    Err(e) => problem(&mut r, format!("script {} in {pf}: {e}", s.id)),
                }
            } else if let Some(k) = existing {
                scripts.insert(s.id.clone(), k.id.to_string());
            }
        }
        for note in &n.notes {
            let id = format!(
                "area/{pf}/{}",
                lmoxide_content::slug(&format!("note {}", note.id))
            );
            let key = Key::parse(&id, note.feature.clone()).ok();
            if key
                .as_ref()
                .is_some_and(|k| updated(work, k).unwrap_or(0) >= note.updated)
            {
                continue;
            }
            match views::put_note(
                work,
                *pf,
                &lmoxide_survey::Note { id, ..note.clone() },
                &note.author,
                note.updated.max(1),
            ) {
                Ok(_) => r.notes += 1,
                Err(e) => problem(&mut r, format!("note {} in {pf}: {e}", note.id)),
            }
        }
    }

    for (pf, f) in &set.placements {
        for p in &f.placements {
            let kind = if p.kind == SKind::Bouncer {
                RecordKind::Bouncer
            } else {
                RecordKind::Spawn
            };
            let Ok(key) = Key::parse(
                &format!("{}/{pf}/{}", kind.as_str(), p.id),
                p.feature.clone(),
            ) else {
                continue;
            };
            if p.removed {
                if work.get(&key).is_some() {
                    match work.delete(&key, None, &p.author, now) {
                        Ok(_) => r.removed += 1,
                        Err(e) => problem(&mut r, format!("{}: {e}", key.id)),
                    }
                }
                continue;
            }
            if updated(work, &key).is_some_and(|u| u >= p.updated) {
                continue;
            }
            let done = if kind == RecordKind::Bouncer {
                bouncers::store(work, &key, p, &p.author, p.updated.max(1)).map(|_| r.bouncers += 1)
            } else {
                let what = Placeable {
                    kind: p.kind,
                    name: p.name.clone(),
                    monster_data: p.monster_data,
                    template: p.template,
                    level: p.level,
                    used: None,
                    names: Vec::new(),
                };
                records::template_for(work, objects, &what, &p.author, now).and_then(|t| {
                    let mut o = records::new_object(
                        key.clone(),
                        t,
                        &what,
                        template_of(catalog, p.kind, p.monster_data, p.template),
                    )?;
                    o.playfield = *pf;
                    o.def.pos = p.pos;
                    o.def.rot = Some(p.heading);
                    o.def.patrol = if walks(o.def.kind) {
                        p.patrol.clone()
                    } else {
                        Vec::new()
                    };
                    o.wave = p.wave.clone().map(|w| lmoxide_survey::WaveRef {
                        script: scripts.get(&w.script).cloned().unwrap_or(w.script),
                        wave: w.wave,
                    });
                    o.comment = p.comment.clone();
                    o.tags = p.tags.clone();
                    records::store(work, &o, &p.author, p.updated.max(1)).map(|_| r.spawns += 1)
                })
            };
            if let Err(e) = done {
                problem(&mut r, format!("{}: {e:?}", key.id));
            }
        }
        for w in &f.triggers {
            let id = format!("trigger/{pf}/{}", w.id);
            let Ok(key) = Key::parse(&id, w.feature.clone()) else {
                continue;
            };
            if w.removed {
                if work.get(&key).is_some() && work.delete(&key, None, &w.author, now).is_ok() {
                    r.removed += 1;
                }
                continue;
            }
            if updated(work, &key).is_some_and(|u| u >= w.updated) {
                continue;
            }
            let mut wall = w.clone();
            wall.id = id;
            if let lmoxide_survey::WallAction::Script {
                script: Some(s), ..
            } = &mut wall.action
                && let Some(new) = scripts.get(s)
            {
                *s = new.clone();
            }
            match views::put_wall(work, *pf, &wall, &w.author, w.updated.max(1)) {
                Ok(_) => r.walls += 1,
                Err(e) => problem(&mut r, format!("wall {} in {pf}: {e}", w.id)),
            }
        }
    }
    let existing: HashMap<String, lmoxide_survey::SmashRule> = views::rules(work)
        .into_iter()
        .map(|x| (x.id.clone(), x))
        .collect();
    for rule in &set.smash_rules {
        if existing
            .get(&rule.id)
            .is_some_and(|x| x.updated >= rule.updated)
        {
            continue;
        }
        match views::put_rule(work, rule, &rule.author, rule.updated.max(1)) {
            Ok(_) => r.rules += 1,
            Err(e) => problem(&mut r, format!("smash rule {}: {e}", rule.id)),
        }
    }

    // placements that replaced a guess: the guess goes, and so does `replaces`
    let replacing: Vec<(Key, String)> = work
        .records()
        .filter(|(k, _)| matches!(k.id.kind, RecordKind::Spawn | RecordKind::Bouncer))
        .filter_map(|(k, e)| Some((k.clone(), e.value.get("replaces")?.as_str()?.to_string())))
        .collect();
    for (key, guess) in replacing {
        if let Some(mut v) = work.get(&key).map(|e| e.value.clone()) {
            v.as_object_mut().expect("a record").remove("replaces");
            let author = v
                .get("author")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Err(e) = work.put(v, &author, now) {
                problem(&mut r, format!("{}: {e}", key.id));
                continue;
            }
        }
        if let Some(g) = work
            .versions(&guess)
            .into_iter()
            .map(|(k, _)| k.clone())
            .find(|k| k.feature == "live")
        {
            match work.delete(&g, None, "", now) {
                Ok(_) => r.replaced_guesses += 1,
                Err(e) => problem(&mut r, format!("{guess} (replaced by {}): {e}", key.id)),
            }
        }
    }

    info!(
        spawns = r.spawns,
        bouncers = r.bouncers,
        walls = r.walls,
        scripts = r.scripts,
        notes = r.notes,
        rules = r.rules,
        removed = r.removed,
        replaced_guesses = r.replaced_guesses,
        problems = r.problems.len(),
        "survey set migrated into the content working copy"
    );
    let text =
        serde_json::to_string_pretty(&json!({ "migrated": now, "report": r })).expect("json");
    if let Err(e) = std::fs::write(&marker, text) {
        warn!("{}: {e}", marker.display());
    }
}
