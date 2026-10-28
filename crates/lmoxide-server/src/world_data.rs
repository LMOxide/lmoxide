//! World data from the content tree in the shapes the world modules read: each playfield's
//! scenery and jump pads (`simple_dynels`, as world_content.json had them), teleport
//! destinations and portals, return and respawn points, encounter areas, light gates, mirror
//! puzzles, the recorded buildies and the playfields' names (the world editor's records too).

use std::collections::HashMap;
use std::sync::LazyLock;

use lmoxide_content::{AreaRole, Bouncer, Content, ObjectKind, PuzzleKind, Shape, Spawn, SpawnKind};
use serde_json::{Value, json};

fn content() -> &'static Content {
    crate::gamedata::get()
}

/// Worlds in playfield order.
fn worlds() -> impl Iterator<Item = (u32, &'static String, &'static lmoxide_content::World)> {
    let mut list: Vec<_> = content().worlds.iter().map(|(id, w)| (w.playfield, id, w)).collect();
    list.sort_by_key(|(pf, _, _)| *pf);
    list.into_iter()
}

/// A scenery spawn as a simple dynel.
fn dynel_of(s: &Spawn) -> Option<Value> {
    let c = s.client.as_ref()?;
    let guid = c.guid?;
    if c.dynel_type.is_some() {
        return None;
    }
    let kind = match s.kind {
        SpawnKind::Scenery if s.tags.iter().any(|t| t == "jump-target") => "jump_target",
        SpawnKind::Scenery => "marker",
        SpawnKind::Interactable => "interactable",
        SpawnKind::Buildie => "buildie",
        SpawnKind::Smashable => "smashable",
        _ => return None,
    };
    Some(json!({
        "name": s.name, "pos": s.pos, "rot": s.rot, "scale": s.scale, "script_group": c.script_group,
        "visual": c.visual, "bounds": c.bounds, "guid": guid, "tag": c.tag, "kind": kind, "look": c.look,
    }))
}

/// A jump pad as a simple dynel.
fn pad_of(b: &Bouncer) -> Option<Value> {
    let c = b.client.as_ref()?;
    let mut v = json!({
        "name": b.name, "pos": b.pos, "rot": b.rot, "scale": b.scale, "script_group": c.script_group,
        "visual": c.visual, "bounds": c.bounds, "guid": c.guid?, "tag": c.tag, "kind": "jump_pad", "look": c.look,
        "target": b.target, "target_by": b.target_by,
    });
    if let Some(a) = b.apex {
        v["apex"] = json!(a);
    }
    if let Some(f) = b.flight {
        v["duration"] = json!(f.secs());
    }
    Some(v)
}

/// Per playfield: `name`, `simple_dynels` (scenery, interactables and jump pads), `triggers`
/// (cinematic triggers: name, position).
pub static WORLD_CONTENT: LazyLock<HashMap<String, Value>> = LazyLock::new(|| {
    let c = content();
    let mut out = HashMap::new();
    for (pf, id, w) in worlds() {
        let mut dynels: Vec<Value> = c.in_world(&c.spawns, id).filter_map(dynel_of).collect();
        dynels.extend(c.in_world(&c.bouncers, id).filter_map(pad_of));
        let triggers: Vec<Value> = c
            .in_world(&c.spawns, id)
            .filter(|s| s.kind == SpawnKind::Trigger)
            .map(|s| json!({ "name": s.name, "pos": s.pos }))
            .collect();
        out.insert(pf.to_string(), json!({ "name": w.name, "simple_dynels": dynels, "triggers": triggers }));
    }
    out
});

/// How far from a portal its door may stand (the client data puts some portals' trigger points
/// a few metres off their door: Monster Laboratory 4.4 m).
const PORTAL_DOOR_RANGE: f32 = 5.0;

/// The door live sent at a portal (an invisible `empty_monsterdata_1x1x1` door players click,
/// LMO_Captures.cap): the nearest door spawn within `PORTAL_DOOR_RANGE`.
fn portal_door(doors: &[&Spawn], pos: [f32; 3]) -> Option<Value> {
    let d = |s: &&&Spawn| s.pos.iter().zip(pos).map(|(a, b)| (a - b).powi(2)).sum::<f32>().sqrt();
    let s = doors.iter().filter(|s| d(s) <= PORTAL_DOOR_RANGE).min_by(|a, b| d(a).total_cmp(&d(b)))?;
    let c = s.client.as_ref()?;
    let rot = match s.rot.unwrap_or_default() {
        lmoxide_content::Rot::Quat(q) => q,
        lmoxide_content::Rot::Heading(h) => [0.0, (h / 2.0).sin(), 0.0, (h / 2.0).cos()],
    };
    Some(json!({
        "guid": c.guid?, "name": s.name, "pos": s.pos, "rot": rot, "scale": s.scale, "visual": c.visual?,
    }))
}

/// Teleport destinations (`destinations`: name, guid, pos, rot, window) and portals (`portals`:
/// to, pos, window, and the `door` players click), by playfield.
pub static DESTINATIONS: LazyLock<Value> = LazyLock::new(|| {
    let c = content();
    let mut destinations = serde_json::Map::new();
    let mut portals = serde_json::Map::new();
    for (pf, id, w) in worlds() {
        if let Some(a) = &w.arrival {
            let mut d = json!({
                "name": w.display_name.as_ref().unwrap_or(&w.name), "guid": w.arrival_guid.unwrap_or(0),
                "pos": a.pos, "rot": a.rot.map_or(0.0, |r| r.heading()), "window": w.window.unwrap_or(0),
            });
            if let Some(m) = &w.arrival_mode {
                d["arrival"] = json!(m);
            }
            destinations.insert(pf.to_string(), d);
        }
        let doors: Vec<&Spawn> = c.in_world(&c.spawns, id).filter(|s| s.kind == SpawnKind::Door).collect();
        let list: Vec<Value> = c
            .in_world(&c.portals, id)
            .filter_map(|p| {
                let to = c.worlds.get(p.to.world.as_ref()?)?.playfield;
                let mut v = json!({ "to": to, "pos": p.pos, "window": p.window? });
                if let Some(d) = portal_door(&doors, p.pos) {
                    v["door"] = d;
                }
                Some(v)
            })
            .collect();
        if !list.is_empty() {
            portals.insert(pf.to_string(), Value::Array(list));
        }
    }
    json!({ "destinations": destinations, "portals": portals })
});

/// Where teleports back from a world arrive: playfield -> from playfield -> {pos, rot}.
pub static RETURN_POINTS: LazyLock<HashMap<String, HashMap<String, Value>>> = LazyLock::new(|| {
    let c = content();
    worlds()
        .filter(|(_, _, w)| !w.arrival_from.is_empty())
        .map(|(pf, _, w)| {
            let from = w
                .arrival_from
                .iter()
                .filter_map(|(from, p)| Some((c.worlds.get(from)?.playfield.to_string(), json!({ "name": "", "pos": p.pos, "rot": p.rot.map_or(0.0, |r| r.heading()) }))))
                .collect();
            (pf.to_string(), from)
        })
        .collect()
});

/// The client's resurrection points (World Map teleport spots) by playfield.
pub static RESURRECTION_POINTS: LazyLock<HashMap<String, Value>> = LazyLock::new(|| {
    let mut out: HashMap<String, Value> = worlds()
        .filter_map(|(pf, _, w)| {
            // the client's list (with restoration levels); world_content's own points aren't spots
            let spots: Vec<Value> = w
                .respawn_points
                .iter()
                .filter(|r| r.restoration.is_some())
                .map(|r| {
                    json!({ "discoverable": r.discoverable, "guid": r.guid, "name": r.name, "pos": r.pos,
                        "restoration": r.restoration, "rot": r.rot.map_or(0.0, |x| x.heading()), "shady": r.shady })
                })
                .collect();
            (!spots.is_empty()).then(|| (pf.to_string(), Value::Array(spots)))
        })
        .collect();
    for (k, v) in &content().tunables.more {
        if let Some(key) = k.strip_prefix("teleport.resurrection_points_of_unknown_key.") {
            out.insert(key.to_string(), v.clone());
        }
    }
    out
});

/// Encounter areas by playfield: `arenas`, `ambushes`, `explore`, `champion`.
pub static ENCOUNTERS: LazyLock<HashMap<String, Value>> = LazyLock::new(|| {
    let c = content();
    let mut out = HashMap::new();
    for (pf, id, _) in worlds() {
        let mut v = serde_json::Map::new();
        for a in c.in_world(&c.areas, id) {
            let section = match a.role {
                AreaRole::Arena => "arenas",
                AreaRole::Ambush => "ambushes",
                AreaRole::Explore => "explore",
                AreaRole::Champion => {
                    if let Some(Shape::Ground { at }) = a.shape {
                        v.insert("champion".into(), json!(at));
                    }
                    continue;
                }
                _ => continue,
            };
            let Some(Shape::Polygon { points, heights }) = &a.shape else { continue };
            let mut e = json!({ "name": a.name, "polygon": points, "heights": heights });
            if a.role == AreaRole::Arena {
                let d = a.doors.clone().unwrap_or_default();
                e["suffix"] = json!(a.suffix.clone().unwrap_or_default());
                e["doors"] = json!({ "open": d.open, "closed": d.closed, "exit": d.exit });
            }
            v.entry(section).or_insert_with(|| json!([])).as_array_mut().expect("list").push(e);
        }
        if !v.is_empty() {
            out.insert(pf.to_string(), Value::Object(v));
        }
    }
    out
});

/// Light gates (gates.rs) by playfield.
pub static LIGHT_GATES: LazyLock<HashMap<String, Value>> = LazyLock::new(|| {
    let c = content();
    let mut out: HashMap<String, Value> = HashMap::new();
    for (pf, id, _) in worlds() {
        for p in c.in_world(&c.puzzles, id) {
            if let PuzzleKind::LightGate { .. } = &p.puzzle {
                let v = serde_json::to_value(&p.puzzle).expect("puzzle")["light_gate"].clone();
                out.entry(pf.to_string()).or_insert_with(|| json!([])).as_array_mut().expect("list").push(v);
            }
        }
    }
    out
});

/// Medusa mirror puzzles (mirrors.rs) by playfield.
pub static MIRROR_PUZZLES: LazyLock<HashMap<String, Value>> = LazyLock::new(|| {
    let c = content();
    let mut out = HashMap::new();
    for (pf, id, _) in worlds() {
        for p in c.in_world(&c.puzzles, id) {
            if let PuzzleKind::Mirrors { .. } = &p.puzzle {
                out.insert(pf.to_string(), serde_json::to_value(&p.puzzle).expect("puzzle")["mirrors"].clone());
            }
        }
    }
    out
});

/// The recorded buildies (buildie.rs): the buildie spawns whose template has the recorded
/// spawn data (its `client.display_name`).
pub static BUILDIES: LazyLock<Vec<Value>> = LazyLock::new(|| {
    let c = content();
    let mut out = Vec::new();
    for (pf, id, _) in worlds() {
        for s in c.in_world(&c.spawns, id).filter(|s| s.kind == SpawnKind::Buildie && s.client.as_ref().is_some_and(|c| c.cars_group.is_some())) {
            let Some(o) = s.template.as_ref().and_then(|t| c.objects.get(t)).filter(|o| o.kind == ObjectKind::Buildie) else { continue };
            let Some(oc) = o.client.as_ref().filter(|oc| oc.display_name.is_some()) else { continue };
            out.push(json!({
                "name": o.name, "display_name": oc.display_name, "template": o.render.template,
                "cars_group": s.client.as_ref().and_then(|c| c.cars_group), "playfield": pf, "pos": s.pos, "rot": s.rot,
                "monster_data": o.render.monster_data, "visual_type": o.render.visual.map(|v| v[0]), "particles": oc.particles,
                "bounds": oc.bounds, "scale": s.scale, "health": 0, "max": o.build.as_ref().and_then(|b| b.points),
                "destructible": oc.destructible, "tail": oc.tail, "built_visual": o.build.as_ref().and_then(|b| b.built_visual),
            }));
        }
    }
    out
});

/// The client's playfield names (RDB 1070003).
pub fn playfield_names() -> std::collections::BTreeMap<u32, String> {
    worlds().map(|(pf, _, w)| (pf, w.name.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(format!("{}/../../data/archive/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap()
    }

    /// Equal as the server reads them: numbers as f32, nulls as missing.
    fn same(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => x.as_f64().map(|v| v as f32) == y.as_f64().map(|v| v as f32),
            (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
            (Value::Object(x), Value::Object(y)) => {
                x.keys().chain(y.keys()).all(|k| same(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null)))
            }
            (Value::String(x), Value::String(y)) => x.replace("&apos;", "'") == y.replace("&apos;", "'"),
            _ => a == b,
        }
    }

    fn by_key(list: &Value, key: &str) -> HashMap<String, Value> {
        list.as_array().into_iter().flatten().map(|v| (format!("{}|{}", v[key], v["name"]), v.clone())).collect()
    }

    #[test]
    fn same_as_the_old_files() {
        let problems = std::cell::RefCell::new(Vec::<String>::new());
        let check = |what: String, old: &Value, new: Option<&Value>| match new {
            Some(n) if same(old, n) => {}
            Some(n) => problems.borrow_mut().push(format!("{what}: {old} vs {n}")),
            None => problems.borrow_mut().push(format!("{what}: missing")),
        };
        let wc = data("world_content.json");
        for (pf, w) in wc.as_object().unwrap().iter().filter(|(_, w)| w.is_object()) {
            let new = WORLD_CONTENT.get(pf);
            let old = by_key(&w["simple_dynels"], "guid");
            let got = new.map(|n| by_key(&n["simple_dynels"], "guid")).unwrap_or_default();
            for (k, v) in &old {
                check(format!("{pf} dynel {k}"), v, got.get(k));
            }
            if got.len() != old.len() {
                problems.borrow_mut().push(format!("{pf}: {} dynels vs {}", old.len(), got.len()));
            }
            let sorted = |v: &Value| {
                let mut l: Vec<Value> = v.as_array().into_iter().flatten().map(|t| json!({ "name": t["name"], "pos": t["pos"] })).collect();
                l.sort_by_key(|t| t["name"].to_string());
                json!(l)
            };
            check(format!("{pf} triggers"), &sorted(&w["triggers"]), new.map(|n| sorted(&n["triggers"])).as_ref());
        }
        let d = data("destinations.json");
        for (pf, v) in d["destinations"].as_object().unwrap() {
            check(format!("destination {pf}"), v, DESTINATIONS["destinations"].get(pf));
        }
        for (pf, list) in d["portals"].as_object().unwrap() {
            // (the old list had exact duplicates)
            let mut old: Vec<Value> = list.as_array().unwrap().clone();
            old.dedup();
            let old: std::collections::BTreeSet<String> = old.iter().map(|p| format!("{}|{:?}|{}", p["to"], p["pos"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect::<Vec<_>>(), p["window"])).collect();
            let new: std::collections::BTreeSet<String> = DESTINATIONS["portals"][pf].as_array().into_iter().flatten().map(|p| format!("{}|{:?}|{}", p["to"], p["pos"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect::<Vec<_>>(), p["window"])).collect();
            if old != new {
                problems.borrow_mut().push(format!("portals {pf}: {old:?} vs {new:?}"));
            }
        }
        for (pf, m) in data("return_points.json").as_object().unwrap().iter().filter(|(_, m)| m.is_object()) {
            for (from, p) in m.as_object().unwrap() {
                check(format!("return {pf} from {from}"), &json!({ "name": "", "pos": p["pos"], "rot": p["rot"] }), RETURN_POINTS.get(pf).and_then(|r| r.get(from)));
            }
        }
        for (pf, list) in data("resurrection_points.json").as_object().unwrap() {
            check(format!("resurrection points {pf}"), list, RESURRECTION_POINTS.get(pf));
        }
        for (pf, e) in data("encounters.json").as_object().unwrap().iter().filter(|(_, e)| e.is_object()) {
            check(format!("encounters {pf}"), e, ENCOUNTERS.get(pf));
        }
        for (pf, g) in data("light_gates.json").as_object().unwrap().iter().filter(|(_, g)| g.is_array()) {
            check(format!("light gates {pf}"), g, LIGHT_GATES.get(pf));
        }
        for (pf, m) in data("mirror_puzzles.json").as_object().unwrap().iter().filter(|(_, m)| m.is_object()) {
            check(format!("mirrors {pf}"), m, MIRROR_PUZZLES.get(pf));
        }
        let old_buildies = data("buildies.json");
        let new_buildies = json!(*BUILDIES);
        let (ob, nb) = (old_buildies.as_array().unwrap(), new_buildies.as_array().unwrap());
        if ob.len() != nb.len() {
            problems.borrow_mut().push(format!("buildies: {} vs {}", ob.len(), nb.len()));
        }
        for (o, n) in ob.iter().zip(nb) {
            for (k, v) in o.as_object().unwrap() {
                if !same(v, &n[k]) {
                    problems.borrow_mut().push(format!("buildie {} {k}: {v} vs {}", o["name"], n[k]));
                }
            }
        }
        let names = playfield_names();
        for (pf, n) in data("playfields.json")["playfields"].as_object().unwrap() {
            if names.get(&pf.parse::<u32>().unwrap()).map(String::as_str) != n.as_str() {
                problems.borrow_mut().push(format!("playfield name {pf}"));
            }
        }
        let problems = problems.into_inner();
        assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.iter().take(8).cloned().collect::<Vec<_>>().join("\n"));
    }
}
