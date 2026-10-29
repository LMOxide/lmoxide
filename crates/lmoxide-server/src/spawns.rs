//! The NPCs and smashables every playfield spawns, from the content tree: each world's spawns
//! (content/worlds/*/spawns.json) with their templates (content/npcs, content/objects) and
//! overrides applied, and the world's NPC fights (world.json `npc_fights`). Spawns of script
//! waves are scripts.rs's (content.rs), spawns that appear only during play (`appeared_at`),
//! cinematic triggers and actors, scenery, doors and buildies other modules' (or nobody's yet).

use std::collections::HashMap;

use lmoxide_content::{Content, FightRole, Rot, Source, Spawn, SpawnKind};
use serde_json::Value;

use crate::npc::{Kind, SpawnDef};

/// The old `source` strings survey mode reads ("proposed": a guess).
pub fn source_name(s: Option<Source>) -> Option<String> {
    Some(
        match s? {
            Source::Live => "live",
            Source::Demo => "demo",
            Source::Proposed | Source::Server => "proposed",
            Source::Surveyed => "survey",
            Source::Footage => "footage",
            Source::Guess => "guess",
            Source::Client => "client",
        }
        .into(),
    )
}

/// A spawn as the NPC code's definition, its template and overrides applied.
fn def_of(c: &Content, s: &Spawn) -> Option<SpawnDef> {
    let t: Value = c.spawn_template(s)?;
    def_from(s, &t, &|a| c.skills.get(a).and_then(|s| s.client.as_ref()).map(|c| c.spell), &|kind, loot| own_loot(c, kind, loot))
}

/// A spawn as the NPC code's definition from its template (resolved, overrides applied): the
/// attack's client spell by skill id, and the loot table it drops when it isn't its kind's
/// usual one. None for kinds the NPC code doesn't spawn, or without a look.
pub fn def_from(s: &Spawn, t: &Value, spell_of: &dyn Fn(&str) -> Option<u32>, own_loot: &dyn Fn(Kind, Option<&str>) -> Option<String>) -> Option<SpawnDef> {
    let kind = match (s.kind, s.fight) {
        (_, Some(FightRole::Fighter)) => Kind::Fighter,
        (SpawnKind::Enemy, _) => Kind::Enemy,
        (SpawnKind::Npc, _) => Kind::Npc,
        (SpawnKind::Smashable, _) => Kind::Smashable,
        _ => return None,
    };
    let render = &t["render"];
    let u = |v: &Value| v.as_u64().map(|n| n as u32);
    let (health, spell) = if kind == Kind::Smashable {
        (u(&t["health"]), None)
    } else {
        let attack = t["attack"].as_str().and_then(spell_of);
        (u(&t["stats"]["health"]), attack)
    };
    Some(SpawnDef {
        kind,
        template: u(&render["template"]).unwrap_or(0),
        name: s.name.clone(),
        monster_data: u(&render["monster_data"])?,
        level: u(&t["level"]).unwrap_or(1),
        health: health.unwrap_or(1),
        pos: s.pos,
        spell: spell.unwrap_or(crate::npc::DEFAULT_SPELL),
        cars_group: u(&render["cars_group"]),
        npc_template: u(&render["npc_template"]),
        reach: t["ai"]["reach"].as_f64().map(|r| r as f32),
        anchor: s.anchor.clone(),
        fights: false,
        faction: None,
        patrol: s.patrol.clone(),
        loot: own_loot(kind, t["loot"].as_str()),
        source: source_name(s.source),
        rot: s.rot.map(Rot::heading),
    })
}

/// A template's loot table when it isn't the one its kind rolls anyway (tunables
/// `loot.tables`: `smashable`, `enemy`, which bosses and champions trade for theirs by rank).
pub fn own_loot(c: &Content, kind: Kind, loot: Option<&str>) -> Option<String> {
    let usual = c.tunables.loot.tables.get(if kind == Kind::Smashable { "smashable" } else { "enemy" });
    loot.filter(|l| Some(&l.to_string()) != usual).map(str::to_string)
}

/// What every playfield spawns (by playfield, as text like the old table): its spawns in the
/// order of spawns.json, then the NPC fights' own (enemies, fighters), with the fights' rules:
/// proposed enemies at `replace_anchors` go, armed NPCs fight, rival looks take sides.
pub fn table() -> HashMap<String, Vec<SpawnDef>> {
    table_of(crate::gamedata::get())
}

/// [`table`] of content `c` (a preview's: the world editor's working copy, preview.rs).
pub fn table_of(c: &Content) -> HashMap<String, Vec<SpawnDef>> {
    let mut table: HashMap<String, Vec<SpawnDef>> = HashMap::new();
    for (world_id, w) in &c.worlds {
        let spawns: Vec<&Spawn> = c
            .in_world(&c.spawns, world_id)
            .filter(|s| !s.removed && s.wave.is_none() && s.appeared_at.is_none())
            .collect();
        let mut defs: Vec<SpawnDef> = spawns.iter().filter(|s| s.fight.is_none()).filter_map(|s| def_of(c, s)).collect();
        let fights = spawns.iter().filter(|s| s.fight.is_some()).filter_map(|s| def_of(c, s));
        if let Some(f) = &w.npc_fights {
            defs.retain(|d| d.kind != Kind::Enemy || d.anchor.as_ref().is_none_or(|a| !f.replace_anchors.contains(a)));
        }
        defs.extend(fights);
        if let Some(f) = &w.npc_fights {
            for d in defs.iter_mut().filter(|d| d.kind == Kind::Npc && f.armed_npcs.contains(&d.name)) {
                d.fights = true;
            }
            for d in defs.iter_mut().filter(|d| d.kind == Kind::Enemy) {
                d.faction = f.rivals.iter().position(|side| side.contains(&d.monster_data)).map(|i| i as u32);
            }
        }
        if !defs.is_empty() {
            table.insert(w.playfield.to_string(), defs);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table the server built from data/spawns.json, world_content.json and npc_fights.json
    /// (npc.rs before the content tree), to compare with.
    fn old_table() -> HashMap<String, Vec<SpawnDef>> {
        let data = |name: &str| std::fs::read_to_string(format!("{}/../../data/archive/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let mut table: HashMap<String, Vec<SpawnDef>> = serde_json::from_str(&data("spawns.json")).unwrap();
        for d in table.values_mut().flatten() {
            d.source.get_or_insert_with(|| "proposed".into());
        }
        let worlds: HashMap<String, Value> = serde_json::from_str(&data("world_content.json")).unwrap();
        for (playfield, world) in worlds.into_iter().filter(|(_, w)| w.is_object()) {
            let defs = table.entry(playfield).or_default();
            let source = world["source"].as_str().map(str::to_string);
            for (section, kind) in [("enemies", Kind::Enemy), ("npcs", Kind::Npc), ("smashables", Kind::Smashable)] {
                for v in world[section].as_array().into_iter().flatten() {
                    let d: SpawnDef = serde_json::from_value(v.clone()).unwrap();
                    defs.push(SpawnDef { kind, source: d.source.clone().or_else(|| source.clone()), ..d });
                }
            }
        }
        let spots: HashMap<String, Value> = serde_json::from_str(&data("npc_fights.json")).unwrap();
        for (playfield, spot) in spots.into_iter().filter(|(_, s)| s.is_object()) {
            let defs = table.entry(playfield).or_default();
            let anchors: Vec<String> = serde_json::from_value(spot["replace_anchors"].clone()).unwrap_or_default();
            defs.retain(|d| d.kind != Kind::Enemy || d.anchor.as_ref().is_none_or(|a| !anchors.contains(a)));
            for (section, kind) in [("enemies", Kind::Enemy), ("fighters", Kind::Fighter)] {
                for v in spot[section].as_array().into_iter().flatten() {
                    let d: SpawnDef = serde_json::from_value(v.clone()).unwrap();
                    defs.push(SpawnDef { kind, source: d.source.clone().or_else(|| Some("proposed".into())), ..d });
                }
            }
            let armed: Vec<String> = serde_json::from_value(spot["armed_npcs"].clone()).unwrap_or_default();
            let rivals: Vec<Vec<u32>> = serde_json::from_value(spot["rivals"].clone()).unwrap_or_default();
            for d in defs.iter_mut().filter(|d| d.kind == Kind::Npc && armed.contains(&d.name)) {
                d.fights = true;
            }
            for d in defs.iter_mut().filter(|d| d.kind == Kind::Enemy) {
                d.faction = rivals.iter().position(|side| side.contains(&d.monster_data)).map(|i| i as u32);
            }
        }
        table.retain(|_, v| !v.is_empty());
        table
    }

    /// A definition as comparable text (positions rounded to the millimetre), with what the
    /// server tells apart: the source's category (survey mode: proposed, live, demo, the rest
    /// "client") and anchors of enemies (npc_fights' `replace_anchors`).
    fn key(d: &SpawnDef) -> String {
        let source = lmoxide_survey::Category::from_source(d.source.as_deref());
        let anchor = if d.kind == Kind::Enemy { d.anchor.clone() } else { None };
        let r = |x: f32| (x * 1000.0).round() as i64;
        format!(
            "{:?}|{}|{}|{}|{}|{}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}",
            d.kind,
            d.template,
            d.name,
            d.monster_data,
            d.level,
            d.health,
            d.pos.map(r),
            d.spell,
            d.cars_group,
            d.npc_template,
            d.reach,
            anchor,
            d.fights,
            d.faction,
            d.patrol.iter().map(|p| p.map(r)).collect::<Vec<_>>(),
            source,
            d.rot.map(r),
        )
    }

    #[test]
    fn own_loot_tables() {
        let c = crate::gamedata::get();
        // the usual tables aren't "own": bosses and champions still trade them by rank
        assert_eq!(own_loot(c, Kind::Enemy, Some("loot/enemy")), None);
        assert_eq!(own_loot(c, Kind::Smashable, Some("loot/smashable")), None);
        assert_eq!(own_loot(c, Kind::Enemy, Some("loot/boss")).as_deref(), Some("loot/boss"));
        // so far every template has its kind's usual table but the smashable chests and cages
        // (live: always an orb and 4-5 stars)
        assert!(table().values().flatten().all(|d| d.loot.is_none() || d.loot.as_deref() == Some("loot/smashable-chest")));
        assert!(table().values().flatten().any(|d| d.loot.as_deref() == Some("loot/smashable-chest")));
    }

    /// (the world editor's spawns weren't in the old files)
    #[test]
    fn same_as_the_old_files() {
        let mut new = table();
        for defs in new.values_mut() {
            defs.retain(|d| d.source.as_deref() != Some("survey"));
        }
        let old = old_table();
        let mut problems = Vec::new();
        for pf in old.keys().chain(new.keys()).collect::<std::collections::BTreeSet<_>>() {
            let mut a: Vec<String> = old.get(pf).into_iter().flatten().map(key).collect();
            let mut b: Vec<String> = new.get(pf).into_iter().flatten().map(key).collect();
            a.sort();
            b.sort();
            if a != b {
                let only_old: Vec<&String> = a.iter().filter(|x| !b.contains(x)).take(3).collect();
                let only_new: Vec<&String> = b.iter().filter(|x| !a.contains(x)).take(3).collect();
                problems.push(format!("{pf}: {} vs {}\n  old: {only_old:#?}\n  new: {only_new:#?}", a.len(), b.len()));
            }
        }
        if let Ok(path) = std::env::var("SPAWN_DIFF") {
            let all: Vec<String> = old.values().flatten().map(key).collect();
            let alln: Vec<String> = new.values().flatten().map(key).collect();
            std::fs::write(path, serde_json::to_string(&(all, alln)).unwrap()).unwrap();
        }
        assert!(problems.is_empty(), "{} playfields differ:\n{}", problems.len(), problems.iter().take(6).cloned().collect::<Vec<_>>().join("\n"));
    }
}
