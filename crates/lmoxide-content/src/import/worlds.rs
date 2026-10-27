//! Worlds and what's in them: world_content.json, destinations, return and resurrection points,
//! encounters, puzzles, NPC fights, data/spawns.json. NPC and object templates are made from the
//! placements (one per distinct name and look).

use std::collections::HashMap;

use anyhow::Result;
use serde_json::{Value, json};

use super::*;
use crate::action::Destination;
use crate::record::{Point, Rot};

/// The playfields that are open worlds (encounter.rs OPEN_WORLDS) and the hub.
const OPEN_WORLDS: [u32; 11] = [
    1200, 4200, 4210, 4600, 5200, 5210, 3200, 3210, 2210, 6200, 1300,
];
const MAX_PLAYER_LEVEL: u32 = 30;

/// progression.rs level_cap: by the playfield's thousands.
fn level_cap(playfield: u32) -> Option<u32> {
    match playfield / 1000 {
        4 => Some(11),
        5 => Some(15),
        3 => Some(19),
        2 => Some(21),
        6 => Some(MAX_PLAYER_LEVEL),
        _ => None,
    }
}

pub fn worlds(tree: &mut Tree, ctx: &mut Ctx, data: &dyn Fn(&str) -> Result<Value>) -> Result<()> {
    let playfields = data("playfields.json")?;
    let achievements = data("achievements.json")?;
    let destinations = data("destinations.json")?;
    let returns = data("return_points.json")?;
    let res = data("resurrection_points.json")?;
    let content = data("world_content.json")?;
    let drops = data("minifigure_drops.json")?;

    let mut list: Vec<(u32, String)> = entries(&playfields["playfields"])
        .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_str()?.to_string())))
        .collect();
    list.sort();
    for (pf, name) in &list {
        let id = ctx.ids.new_id("world", name);
        ctx.world.insert(*pf, id.clone());
        tree.world_dirs
            .insert(id.clone(), format!("{pf}-{}", slug(name)));
    }
    for (pf, name) in list {
        let id = ctx.world[&pf].clone();
        let dest = &destinations["destinations"][pf.to_string()];
        let wc = &content[pf.to_string()];
        let mut w = World {
            id,
            name: name.clone(),
            playfield: pf,
            source: Some(Source::Client),
            kind: match pf {
                1200 => WorldKind::Hub,
                _ if OPEN_WORLDS.contains(&pf) => WorldKind::Open,
                _ if wc.is_object() => WorldKind::Pocket,
                _ => WorldKind::Other,
            },
            ..Default::default()
        };
        if let Some(d) = achievements["playfields"][pf.to_string()]
            .as_str()
            .filter(|d| *d != name)
        {
            w.display_name = Some(d.to_string());
        }
        if let Some(cap) = level_cap(pf) {
            w.level = Some(LevelRange {
                cap: Some(cap),
                ..Default::default()
            });
        }
        if dest.is_object() {
            w.arrival = vec3(&dest["pos"]).map(|pos| Point {
                pos,
                rot: rot(&dest["rot"]),
            });
            w.arrival_guid = u32_of(&dest["guid"]).filter(|g| *g != 0);
            w.arrival_mode = str_of(&dest["arrival"]);
            w.window = u32_of(&dest["window"]).filter(|w| *w != 0);
        }
        for (from, p) in entries(&returns[pf.to_string()]) {
            let Some(from_world) = from.parse().ok().and_then(|f: u32| ctx.world.get(&f)) else {
                tree.notes.push(format!(
                    "return point of {pf} from unknown playfield {from}"
                ));
                continue;
            };
            if let Some(pos) = vec3(&p["pos"]) {
                w.arrival_from.insert(
                    from_world.clone(),
                    Point {
                        pos,
                        rot: rot(&p["rot"]),
                    },
                );
            }
        }
        for r in res[pf.to_string()].as_array().into_iter().flatten() {
            w.respawn_points.push(respawn_point(r));
        }
        for r in wc["resurrection_points"].as_array().into_iter().flatten() {
            let guid = u32_of(&r["guid"]);
            if !w
                .respawn_points
                .iter()
                .any(|p| p.guid.is_some() && p.guid == guid)
            {
                w.respawn_points.push(respawn_point(r));
            }
        }
        if let Some(note) = wc["live"]["note"].as_str() {
            w.comment = format!(
                "{note} (visits {}, seen radius {} m)",
                wc["live"]["visits"], wc["live"]["seen_radius"]
            );
        }
        tree.worlds.push(w);
    }
    for (k, v) in entries(&res) {
        if !k
            .parse()
            .ok()
            .is_some_and(|pf: u32| ctx.world.contains_key(&pf))
        {
            tree.tunables.more.insert(
                format!("teleport.resurrection_points_of_unknown_key.{k}"),
                v.clone(),
            );
        }
    }

    // regions: the worlds a minifigure drop text names
    for (name, pfs) in entries(&drops["worlds"]) {
        let id = ctx.ids.new_id("region", name);
        ctx.region.insert(name.clone(), id.clone());
        let mut region = Region {
            id: id.clone(),
            name: name.clone(),
            source: Some(Source::Client),
            ..Default::default()
        };
        for pf in pfs.as_array().into_iter().flatten().filter_map(u32_of) {
            match ctx.world.get(&pf) {
                Some(w) => {
                    region.worlds.push(w.clone());
                    if let Some(world) = tree.worlds.iter_mut().find(|x| x.playfield == pf) {
                        world.region = Some(id.clone());
                    }
                }
                None => tree
                    .notes
                    .push(format!("region {name}: unknown playfield {pf}")),
            }
        }
        tree.regions.push(region);
    }
    Ok(())
}

fn respawn_point(r: &Value) -> RespawnPoint {
    RespawnPoint {
        name: str_of(&r["name"]).unwrap_or_default(),
        guid: u32_of(&r["guid"]),
        pos: vec3(&r["pos"]).unwrap_or_default(),
        rot: rot(&r["rot"]),
        discoverable: r["discoverable"].as_bool(),
        restoration: u32_of(&r["restoration"]),
        shady: r["shady"].as_bool(),
    }
}

/// NPC and object templates made from placements: key -> id, and what the template has (to
/// write only differences as a spawn's overrides).
#[derive(Default)]
pub struct Templates {
    npc: HashMap<String, String>,
    object: HashMap<String, String>,
    base: HashMap<String, Value>,
}

impl Templates {
    /// The NPC template of a placement entry (name, monster_data, look, level, health,
    /// npc_template, cars_group, spell, reach), made the first time.
    pub fn npc(
        &mut self,
        tree: &mut Tree,
        ctx: &mut Ctx,
        e: &Value,
        kind: NpcKind,
        npc_spells: &Value,
        npc_life: &Value,
    ) -> String {
        let name = str_of(&e["name"]).unwrap_or_else(|| "unnamed".into());
        let md = u32_of(&e["monster_data"]);
        let key = format!(
            "{kind:?}|{name}|{md:?}|{}|{}",
            e["npc_template"], e["spell"]
        );
        if let Some(id) = self.npc.get(&key) {
            return id.clone();
        }
        let id = ctx.ids.new_id("npc", &name);
        let mut npc = Npc {
            id: id.clone(),
            name: name.clone(),
            kind,
            source: Some(Source::Client),
            level: u32_of(&e["level"]),
            render: Render {
                monster_data: md,
                npc_template: u32_of(&e["npc_template"]),
                look: str_of(&e["look"]),
                cars_group: u32_of(&e["cars_group"]),
                ..Default::default()
            },
            stats: NpcStats {
                health: u32_of(&e["health"]),
                ..Default::default()
            },
            ..Default::default()
        };
        if let Some(spell) = u32_of(&e["spell"]) {
            npc.attack = Some(super::defs::skill_ref(tree, ctx, spell, None));
        }
        npc.ai.reach = f32_of(&e["reach"]);
        if let Some(m) = md
            .map(|m| &npc_spells["monsters"][m.to_string()])
            .filter(|m| m.is_object())
        {
            for s in m["spells"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(u32_of)
            {
                let skill = super::defs::skill_ref(tree, ctx, s, None);
                npc.skills.push(SkillUse { skill, weight: 1 });
            }
            npc.skills_from = str_of(&m["source"]);
        }
        if let Some(f) = md.and_then(|m| npc_life["monsters"][m.to_string()]["factor"].as_f64()) {
            npc.stats.life_factor = Some(f as f32);
        }
        npc.loot = match kind {
            NpcKind::Enemy | NpcKind::Boss | NpcKind::Champion => Some("loot/enemy".into()),
            _ => None,
        };
        self.base.insert(id.clone(), json!({ "level": npc.level, "health": npc.stats.health, "cars_group": npc.render.cars_group }));
        tree.npcs.push(npc);
        self.npc.insert(key, id.clone());
        id
    }

    /// An object template by its key (made by `make` the first time).
    pub fn object(
        &mut self,
        tree: &mut Tree,
        ctx: &mut Ctx,
        key: String,
        name: &str,
        make: impl FnOnce(String) -> Object,
    ) -> String {
        if let Some(id) = self.object.get(&key) {
            return id.clone();
        }
        let id = ctx.ids.new_id("object", name);
        let o = make(id.clone());
        self.base.insert(
            id.clone(),
            json!({ "level": o.level, "health": o.health, "cars_group": o.render.cars_group }),
        );
        tree.objects.push(o);
        self.object.insert(key, id.clone());
        id
    }

    /// What a placement changes on its template (`level`, health, `cars_group`).
    pub fn overrides(&self, template: &str, e: &Value, health_path: &str) -> Option<Value> {
        let base = self.base.get(template)?;
        let mut o = serde_json::Map::new();
        if !e["level"].is_null() && e["level"] != base["level"] {
            o.insert("level".into(), e["level"].clone());
        }
        if !e["health"].is_null() && e["health"] != base["health"] {
            match health_path {
                "stats" => {
                    o.insert("stats".into(), json!({ "health": e["health"] }));
                }
                _ => {
                    o.insert("health".into(), e["health"].clone());
                }
            }
        }
        // (null: this placement has none, though its template has)
        if e["cars_group"] != base["cars_group"] {
            o.insert("render".into(), json!({ "cars_group": e["cars_group"] }));
        }
        (!o.is_empty()).then_some(Value::Object(o))
    }
}

/// Per-world record ids: `<kind>/<playfield>/<slug>`, numbered when taken.
fn local_id(ctx: &mut Ctx, kind: &str, pf: u32, name: &str) -> String {
    ctx.ids.new_id(&format!("{kind}/{pf}"), name)
}

pub fn world_records(
    tree: &mut Tree,
    ctx: &mut Ctx,
    data: &dyn Fn(&str) -> Result<Value>,
) -> Result<()> {
    let content = data("world_content.json")?;
    let destinations = data("destinations.json")?;
    let encounters = data("encounters.json")?;
    let fights = data("npc_fights.json")?;
    let mirrors = data("mirror_puzzles.json")?;
    let gates = data("light_gates.json")?;
    let old_spawns = data("spawns.json")?;
    let npc_spells = data("npc_spells.json")?;
    let npc_life = data("npc_life.json")?;
    let loot_items = data("loot_items.json")?;
    let buildies = data("buildies.json")?;

    let mut t = std::mem::take(&mut ctx.templates);
    // client loot table ids of destructible templates
    let client_loot: HashMap<u32, (u32, Option<u32>)> = loot_items["loot_tables"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| {
            Some((
                u32_of(&l["template"])?,
                (u32_of(&l["loot_table"])?, u32_of(&l["dynel_type"])),
            ))
        })
        .collect();
    let recorded_buildies: HashMap<u32, &Value> = buildies
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| Some((u32_of(&b["template"])?, b)))
        .collect();

    let pfs: Vec<u32> = ctx
        .world
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for pf in pfs {
        let world = ctx.world[&pf].clone();
        let wc = &content[pf.to_string()];
        let mut spawns: Vec<Spawn> = Vec::new();
        let mut points: HashMap<String, (crate::Vec3, Option<Rot>)> = HashMap::new();
        for p in wc["static_points"].as_array().into_iter().flatten() {
            if let (Some(n), Some(pos)) = (p["name"].as_str(), vec3(&p["pos"])) {
                points.insert(n.to_string(), (pos, rot(&p["rot"])));
            }
        }

        // enemies, NPCs, cinematic triggers, enemies that appeared during the capture
        let npc_sections: [(&str, SpawnKind, NpcKind); 4] = [
            ("enemies", SpawnKind::Enemy, NpcKind::Enemy),
            ("npcs", SpawnKind::Npc, NpcKind::Npc),
            ("triggers", SpawnKind::Trigger, NpcKind::Trigger),
            ("live_dynamic", SpawnKind::Enemy, NpcKind::Enemy),
        ];
        for (section, spawn_kind, npc_kind) in npc_sections {
            for e in wc[section].as_array().into_iter().flatten() {
                let (spawn_kind, npc_kind) = match (section, u32_of(&e["cars_group"])) {
                    ("live_dynamic", Some(2)) => (SpawnKind::Npc, NpcKind::Npc),
                    _ => (spawn_kind, npc_kind),
                };
                let template = t.npc(tree, ctx, e, npc_kind, &npc_spells, &npc_life);
                let mut s = npc_spawn(ctx, pf, e, spawn_kind, &template, &t);
                s.appeared_at = e["t"].as_f64();
                s.anchor = str_of(&e["anchor"]).or_else(|| str_of(&e["point"]));
                spawns.push(s);
            }
        }
        // old test spawns (data/spawns.json)
        for e in old_spawns[pf.to_string()].as_array().into_iter().flatten() {
            if e["kind"] == "smashable" {
                let template = smashable_template(&mut t, tree, ctx, e, &client_loot);
                let mut s = object_spawn(ctx, pf, e, SpawnKind::Smashable, &template, &t);
                s.source = Some(Source::Server);
                spawns.push(s);
            } else {
                let template = t.npc(tree, ctx, e, NpcKind::Enemy, &npc_spells, &npc_life);
                let mut s = npc_spawn(ctx, pf, e, SpawnKind::Enemy, &template, &t);
                s.source = Some(Source::Server);
                s.tags.push("test-spawn".into());
                spawns.push(s);
            }
        }
        // NPC fights
        if let Some(f) = fights.get(pf.to_string()) {
            let mut nf = NpcFights {
                about: str_of(&f["about"]).unwrap_or_default(),
                rivals_about: str_of(&f["rivals_about"]).unwrap_or_default(),
                ..Default::default()
            };
            nf.replace_anchors = f["replace_anchors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(str_of)
                .collect();
            nf.armed_npcs = f["armed_npcs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(str_of)
                .collect();
            nf.rivals = strict(&f["rivals"]).unwrap_or_default();
            for (section, role, spawn_kind, npc_kind) in [
                (
                    "enemies",
                    FightRole::Enemy,
                    SpawnKind::Enemy,
                    NpcKind::Enemy,
                ),
                ("fighters", FightRole::Fighter, SpawnKind::Npc, NpcKind::Npc),
            ] {
                for e in f[section].as_array().into_iter().flatten() {
                    let template = t.npc(tree, ctx, e, npc_kind, &npc_spells, &npc_life);
                    let mut s = npc_spawn(ctx, pf, e, spawn_kind, &template, &t);
                    s.fight = Some(role);
                    s.source = Some(Source::Server);
                    spawns.push(s);
                }
            }
            if let Some(w) = tree.worlds.iter_mut().find(|w| w.playfield == pf) {
                w.npc_fights = Some(nf);
            }
        }
        // smashables and buildies
        for e in wc["smashables"].as_array().into_iter().flatten() {
            let template = smashable_template(&mut t, tree, ctx, e, &client_loot);
            let mut s = object_spawn(ctx, pf, e, SpawnKind::Smashable, &template, &t);
            s.anchor = str_of(&e["anchor"]);
            spawns.push(s);
        }
        for e in wc["buildies"].as_array().into_iter().flatten() {
            let tpl = u32_of(&e["template"]);
            let name = str_of(&e["name"]).unwrap_or_default();
            let key = format!("buildie|{tpl:?}|{}", e["monster_data"]);
            let recorded = tpl.and_then(|t| recorded_buildies.get(&t)).copied();
            let template = t.object(tree, ctx, key, &name, |id| {
                let mut o = object_from(id, e, ObjectKind::Buildie, &client_loot);
                o.states = vec!["unbuilt".into(), "built".into()];
                if let Some(b) = recorded {
                    o.build = Some(Build {
                        points: u32_of(&b["max"]),
                        built_visual: u32_of(&b["built_visual"]),
                        ..Default::default()
                    });
                    let c = o.client.get_or_insert_with(Default::default);
                    c.display_name = str_of(&b["display_name"]);
                    c.particles = b["particles"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(strict)
                        .collect();
                    c.bounds = strict(&b["bounds"]);
                    c.destructible = Some(b["destructible"].clone()).filter(|v| !v.is_null());
                    c.tail = Some(b["tail"].clone()).filter(|v| !v.is_null());
                    o.render.visual = u32_of(&b["visual_type"]).map(|v| [v, 0]);
                }
                o
            });
            let mut s = object_spawn(ctx, pf, e, SpawnKind::Buildie, &template, &t);
            if let Some(b) = recorded.filter(|b| u32_of(&b["playfield"]) == Some(pf)) {
                s.rot = rot(&b["rot"]).or(s.rot);
                s.pos = vec3(&b["pos"]).unwrap_or(s.pos);
                s.client.get_or_insert_with(Default::default).cars_group = u32_of(&b["cars_group"]);
            }
            spawns.push(s);
        }
        // scenery the server sends: simple dynels, doors
        for (section, default_kind) in [
            ("simple_dynels", SpawnKind::Scenery),
            ("doors", SpawnKind::Door),
        ] {
            for e in wc[section].as_array().into_iter().flatten() {
                if e["kind"] == "jump_pad" {
                    continue; // bouncers.json
                }
                let (spawn_kind, object_kind) = match (default_kind, e["kind"].as_str()) {
                    (SpawnKind::Door, _) => (SpawnKind::Door, ObjectKind::Door),
                    (_, Some("interactable")) => {
                        (SpawnKind::Interactable, ObjectKind::Interactable)
                    }
                    (_, Some("buildie")) => (SpawnKind::Buildie, ObjectKind::Buildie),
                    (_, Some("smashable")) => (SpawnKind::Smashable, ObjectKind::Smashable),
                    _ => (SpawnKind::Scenery, ObjectKind::Scenery),
                };
                let look =
                    str_of(&e["look"]).unwrap_or_else(|| str_of(&e["name"]).unwrap_or_default());
                let key = format!("dynel|{object_kind:?}|{look}|{}", e["visual"]);
                let template = t.object(tree, ctx, key, &look, |id| Object {
                    id,
                    name: look.clone(),
                    kind: object_kind,
                    source: Some(Source::Client),
                    render: Render {
                        look: Some(look.clone()),
                        visual: strict(&e["visual"]),
                        ..Default::default()
                    },
                    states: if object_kind == ObjectKind::Door {
                        vec!["open".into(), "closed".into()]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                });
                let name = str_of(&e["name"]).unwrap_or_default();
                let mut s = Spawn {
                    id: local_id(ctx, "spawn", pf, &name),
                    name,
                    kind: spawn_kind,
                    template: Some(template),
                    pos: vec3(&e["pos"]).unwrap_or_default(),
                    rot: rot(&e["rot"]),
                    scale: f32_of(&e["scale"]).unwrap_or(1.0),
                    source: Some(Source::Client),
                    ..Default::default()
                };
                s.client = Some(SpawnClient {
                    guid: u32_of(&e["guid"]),
                    tag: str_of(&e["tag"]),
                    script_group: e["script_group"].as_i64().map(|n| n as i32),
                    look: str_of(&e["look"]),
                    bounds: strict(&e["bounds"]),
                    visual: strict(&e["visual"]),
                    ..Default::default()
                });
                if e["kind"] == "jump_target" {
                    s.tags.push("jump-target".into());
                }
                spawns.push(s);
            }
        }
        // dynels the live server (or the demo) sent
        for (section, src) in [("live_dynels", Source::Live), ("demo_dynels", Source::Demo)] {
            for e in wc[section].as_array().into_iter().flatten() {
                let look = str_of(&e["look"])
                    .unwrap_or_else(|| str_of(&e["name"]).unwrap_or_default().trim().to_string());
                let key = format!(
                    "live|{}|{}|{look}|{}",
                    e["type"], e["template"], e["visual"]
                );
                let template = t.object(tree, ctx, key, &look, |id| Object {
                    id,
                    name: str_of(&e["name"]).unwrap_or_default().trim().to_string(),
                    kind: ObjectKind::Scenery,
                    source: Some(src),
                    render: Render {
                        template: u32_of(&e["template"]),
                        look: Some(look.clone()),
                        visual: strict(&e["visual"]),
                        ..Default::default()
                    },
                    ..Default::default()
                });
                let name = str_of(&e["name"]).unwrap_or_default().trim().to_string();
                spawns.push(Spawn {
                    id: local_id(ctx, "spawn", pf, &name),
                    name,
                    kind: SpawnKind::Interactable,
                    template: Some(template),
                    pos: vec3(&e["pos"]).unwrap_or_default(),
                    rot: rot(&e["rot"]),
                    scale: 1.0,
                    source: Some(src),
                    client: Some(SpawnClient {
                        dynel_type: u32_of(&e["type"]),
                        cars_group: u32_of(&e["cars_group"]),
                        flags: u32_of(&e["flags"]),
                        hidden: e["is_hidden"].as_bool(),
                        interaction_radius: u32_of(&e["interaction_radius"]),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
        }
        // cinematic actors the client places
        for e in wc["client_npcs"].as_array().into_iter().flatten() {
            let name = str_of(&e["name"]).unwrap_or_default();
            let point = str_of(&e["ai_point"]);
            let (pos, r) = point
                .as_ref()
                .and_then(|p| points.get(p).copied())
                .unwrap_or(([0.0; 3], None));
            spawns.push(Spawn {
                id: local_id(ctx, "spawn", pf, &name),
                name,
                kind: SpawnKind::Actor,
                pos,
                rot: r,
                scale: 1.0,
                anchor: point.clone(),
                source: Some(Source::Client),
                client: Some(SpawnClient {
                    item_template: u32_of(&e["item_template"]),
                    ai_point: point,
                    actor_alias: str_of(&e["actor_alias"]),
                    cinematic: e["cinematic"].as_bool(),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        tree.spawns.insert(world.clone(), spawns);

        // portals
        let mut portals: Vec<Portal> = Vec::new();
        let listed = destinations["portals"][pf.to_string()]
            .as_array()
            .into_iter()
            .flatten();
        let found = wc["portals"].as_array().into_iter().flatten();
        for p in listed.chain(found) {
            let (Some(to), Some(pos)) = (u32_of(&p["to"]), vec3(&p["pos"])) else {
                continue;
            };
            let window = u32_of(&p["window"]).filter(|w| *w != 0);
            let close = |q: &Portal| {
                q.pos
                    .iter()
                    .zip(pos.iter())
                    .all(|(a, b)| (a - b).abs() < 0.5)
            };
            let target = ctx.world.get(&to).cloned();
            if let Some(q) = portals
                .iter_mut()
                .find(|q| close(q) && q.to.world == target)
            {
                q.window = q.window.or(window);
                continue;
            }
            let Some(target) = target else {
                tree.notes
                    .push(format!("portal in {pf} to unknown playfield {to}"));
                continue;
            };
            let to_name = target.split_once('/').map_or("", |x| x.1).to_string();
            portals.push(Portal {
                id: local_id(ctx, "portal", pf, &format!("to {to_name}")),
                pos,
                window,
                to: Destination {
                    world: Some(target),
                    ..Default::default()
                },
                source: Some(Source::Client),
                ..Default::default()
            });
        }
        tree.portals.insert(world.clone(), portals);

        // bouncers
        let mut bouncers = Vec::new();
        for e in wc["simple_dynels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["kind"] == "jump_pad")
        {
            let look = str_of(&e["look"]).unwrap_or_else(|| "jump pad".into());
            let template = t.object(
                tree,
                ctx,
                format!("bouncer|{look}|{}", e["visual"]),
                &look,
                |id| Object {
                    id,
                    name: look.clone(),
                    kind: ObjectKind::Bouncer,
                    source: Some(Source::Client),
                    render: Render {
                        look: Some(look.clone()),
                        visual: strict(&e["visual"]),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            let name = str_of(&e["name"]).unwrap_or_default();
            bouncers.push(Bouncer {
                id: local_id(ctx, "bouncer", pf, &name),
                name,
                template: Some(template),
                pos: vec3(&e["pos"]).unwrap_or_default(),
                rot: rot(&e["rot"]),
                scale: f32_of(&e["scale"]).unwrap_or(1.0),
                target: vec3(&e["target"]),
                target_by: str_of(&e["target_by"]),
                apex: f32_of(&e["apex"]),
                flight: e["duration"].as_f64().map(crate::Duration),
                source: Some(Source::Client),
                client: Some(SpawnClient {
                    guid: u32_of(&e["guid"]),
                    tag: str_of(&e["tag"]),
                    script_group: e["script_group"].as_i64().map(|n| n as i32),
                    look: str_of(&e["look"]),
                    bounds: strict(&e["bounds"]),
                    visual: strict(&e["visual"]),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        tree.bouncers.insert(world.clone(), bouncers);

        // areas: named points, encounter areas
        let mut areas = Vec::new();
        for p in wc["static_points"].as_array().into_iter().flatten() {
            let name = str_of(&p["name"]).unwrap_or_default();
            areas.push(Area {
                id: local_id(ctx, "area", pf, &name),
                name: name.clone(),
                role: AreaRole::Marker,
                shape: vec3(&p["pos"]).map(|pos| Shape::Point {
                    pos,
                    rot: rot(&p["rot"]),
                }),
                source: Some(Source::Client),
                ..Default::default()
            });
        }
        let enc = &encounters[pf.to_string()];
        for (section, role) in [
            ("arenas", AreaRole::Arena),
            ("ambushes", AreaRole::Ambush),
            ("explore", AreaRole::Explore),
        ] {
            for a in enc[section].as_array().into_iter().flatten() {
                let name = str_of(&a["name"]).unwrap_or_default();
                let points: Vec<[f32; 2]> = a["polygon"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| Some([f32_of(&p[0])?, f32_of(&p[1])?]))
                    .collect();
                let heights = [
                    f32_of(&a["heights"][0]).unwrap_or(0.0),
                    f32_of(&a["heights"][1]).unwrap_or(0.0),
                ];
                let doors = (role == AreaRole::Arena).then(|| ArenaDoors {
                    open: a["doors"]["open"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(u32_of)
                        .collect(),
                    closed: a["doors"]["closed"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(u32_of)
                        .collect(),
                    exit: a["doors"]["exit"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(u32_of)
                        .collect(),
                });
                areas.push(Area {
                    id: local_id(
                        ctx,
                        "area",
                        pf,
                        &format!("{name} {}", a["suffix"].as_str().unwrap_or("")),
                    ),
                    name: name.clone(),
                    role,
                    shape: Some(Shape::Polygon { points, heights }),
                    suffix: str_of(&a["suffix"]).filter(|s| !s.is_empty()),
                    doors,
                    source: Some(Source::Client),
                    ..Default::default()
                });
            }
        }
        if let Some(c) = enc["champion"].as_array() {
            areas.push(Area {
                id: local_id(ctx, "area", pf, "champion"),
                name: "Champion".into(),
                role: AreaRole::Champion,
                shape: Some(Shape::Ground {
                    at: [f32_of(&c[0]).unwrap_or(0.0), f32_of(&c[1]).unwrap_or(0.0)],
                }),
                source: Some(Source::Client),
                ..Default::default()
            });
        }
        tree.areas.insert(world.clone(), areas);

        // puzzles
        let mut puzzles = Vec::new();
        if let Some(m) = mirrors.get(pf.to_string()) {
            match serde_json::from_value::<PuzzleKind>(json!({ "mirrors": {
                "mirrors": m["mirrors"], "roots": m["roots"], "targets": m["targets"], "opens": m["opens"] } }))
            {
                Ok(puzzle) => puzzles.push(Puzzle {
                    id: local_id(ctx, "puzzle", pf, "mirrors"),
                    name: "Medusa mirrors".into(),
                    puzzle,
                    source: Some(Source::Client),
                    ..Default::default()
                }),
                Err(e) => tree.notes.push(format!("mirror puzzle of {pf}: {e}")),
            }
        }
        for g in gates[pf.to_string()].as_array().into_iter().flatten() {
            match serde_json::from_value::<PuzzleKind>(json!({ "light_gate": g })) {
                Ok(puzzle) => puzzles.push(Puzzle {
                    id: local_id(ctx, "puzzle", pf, &format!("light gate {}", g["gate"])),
                    name: "Light gate".into(),
                    puzzle,
                    source: Some(Source::Client),
                    ..Default::default()
                }),
                Err(e) => tree.notes.push(format!("light gate of {pf}: {e}")),
            }
        }
        tree.puzzles.insert(world.clone(), puzzles);
    }
    ctx.templates = t;
    Ok(())
}

fn object_from(
    id: String,
    e: &Value,
    kind: ObjectKind,
    client_loot: &HashMap<u32, (u32, Option<u32>)>,
) -> Object {
    let tpl = u32_of(&e["template"]);
    let mut o = Object {
        id,
        name: str_of(&e["name"]).unwrap_or_default(),
        kind,
        source: Some(Source::Client),
        level: u32_of(&e["level"]),
        health: u32_of(&e["health"]),
        render: Render {
            template: tpl,
            monster_data: u32_of(&e["monster_data"]),
            look: str_of(&e["look"]),
            cars_group: u32_of(&e["cars_group"]),
            ..Default::default()
        },
        loot: match kind {
            ObjectKind::Smashable => Some("loot/smashable".into()),
            _ => None,
        },
        ..Default::default()
    };
    if let Some((table, dynel)) = tpl.and_then(|t| client_loot.get(&t)) {
        o.client = Some(ObjectClient {
            loot_table: Some(*table),
            dynel_type: *dynel,
            ..Default::default()
        });
    }
    o
}

fn smashable_template(
    t: &mut Templates,
    tree: &mut Tree,
    ctx: &mut Ctx,
    e: &Value,
    client_loot: &HashMap<u32, (u32, Option<u32>)>,
) -> String {
    let name = str_of(&e["name"]).unwrap_or_default();
    let key = format!("smashable|{}|{}|{name}", e["template"], e["monster_data"]);
    t.object(tree, ctx, key, &name, |id| {
        object_from(id, e, ObjectKind::Smashable, client_loot)
    })
}

fn npc_spawn(
    ctx: &mut Ctx,
    pf: u32,
    e: &Value,
    kind: SpawnKind,
    template: &str,
    t: &Templates,
) -> Spawn {
    let name = str_of(&e["name"]).unwrap_or_default();
    Spawn {
        id: local_id(ctx, "spawn", pf, &name),
        name,
        kind,
        template: Some(template.to_string()),
        pos: vec3(&e["pos"]).unwrap_or_default(),
        rot: rot(&e["rot"]),
        anchor: str_of(&e["anchor"]),
        scale: 1.0,
        overrides: t.overrides(template, e, "stats"),
        patrol: e["patrol"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(vec3)
            .collect(),
        source: source_of(&e["source"]).or(Some(Source::Client)),
        ..Default::default()
    }
}

fn object_spawn(
    ctx: &mut Ctx,
    pf: u32,
    e: &Value,
    kind: SpawnKind,
    template: &str,
    t: &Templates,
) -> Spawn {
    let name = str_of(&e["name"]).unwrap_or_default();
    Spawn {
        id: local_id(ctx, "spawn", pf, &name),
        name,
        kind,
        template: Some(template.to_string()),
        pos: vec3(&e["pos"]).unwrap_or_default(),
        rot: rot(&e["rot"]),
        scale: 1.0,
        overrides: t.overrides(template, e, "object"),
        source: source_of(&e["source"]).or(Some(Source::Client)),
        ..Default::default()
    }
}
