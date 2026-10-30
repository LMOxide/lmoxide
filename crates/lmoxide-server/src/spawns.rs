//! The NPCs and smashables every playfield spawns, from the content tree: each world's spawns
//! (content/worlds/*/spawns.json) with their templates (content/npcs, content/objects) and
//! overrides applied, and the world's NPC fights (world.json `npc_fights`). Spawns of script
//! waves are scripts.rs's (content.rs), spawns that appear only during play (`appeared_at`),
//! cinematic triggers and actors, scenery, doors and buildies other modules' (or nobody's yet).

use std::collections::HashMap;

use lmoxide_content::{Content, FightRole, NpcKind, Rot, Source, Spawn, SpawnKind};
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
    let mut def = def_from(s, &t, &|a| c.skills.get(a).and_then(|s| s.client.as_ref()).map(|c| c.spell), &|kind, loot| own_loot(c, kind, loot))?;
    def.spread = s.at.as_ref().and_then(|a| c.areas.get(a)).and_then(|a| a.spread);
    Some(def)
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
        icon: u(&render["icon"]),
        reach: t["ai"]["reach"].as_f64().map(|r| r as f32),
        stationary: t["ai"]["stationary"].as_bool().unwrap_or(false),
        unarmed: t["ai"]["unarmed"].as_bool().unwrap_or(false),
        anchor: s.anchor.clone(),
        fights: false,
        faction: None,
        patrol: s.patrol.clone(),
        loot: own_loot(kind, t["loot"].as_str()),
        power_level: u(&t["stats"]["power_level"]),
        source: source_name(s.source),
        rot: s.rot.map(Rot::heading),
        variants: s.variants.clone(),
        requires: s.requires.clone(),
        cinematic: false,
        count: s.count.max(1),
        spread: None,
        speeds: serde_json::from_value(t["movement"].clone()).ok().and_then(|m| crate::npc::movement_speeds(&m)),
        refused_hits: t["refused"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["actions"].as_array().is_some_and(|a| a.iter().any(|a| a == "hit")))
            .filter_map(|r| r["goal"].as_str().map(String::from))
            .collect(),
    })
}

/// Where the factions of every world (tunables `npc.factions`) are numbered from (a world's own
/// rival sides are 0, 1, ...).
pub const GLOBAL_FACTIONS: u32 = 1000;

/// What SpawnMonster game actions spawn, by the original server's template hash (content npcs
/// `spawn_hash`): the template as the NPC code's definition (enemies, bosses and champions
/// fight; NPCs, triggers and summons are friendly anchors nobody fights), and its `lifetime`.
pub fn spawned_templates() -> HashMap<String, (SpawnDef, Option<std::time::Duration>)> {
    let c = crate::gamedata::get();
    c.npcs
        .values()
        .filter_map(|n| {
            let hash = n.spawn_hash.clone()?;
            let kind = match n.kind {
                NpcKind::Enemy | NpcKind::Boss | NpcKind::Champion => SpawnKind::Enemy,
                NpcKind::Npc | NpcKind::Trigger | NpcKind::Summon => SpawnKind::Npc,
            };
            let spawn = Spawn { kind, name: n.name.trim().to_string(), source: n.source, ..Default::default() };
            let t = serde_json::to_value(n).ok()?;
            let def = def_from(&spawn, &t, &|a| c.skills.get(a).and_then(|s| s.client.as_ref()).map(|c| c.spell), &|kind, loot| own_loot(c, kind, loot))?;
            Some((hash, (def, n.lifetime.map(|d| d.to_std()))))
        })
        .collect()
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
        // the sides in every world (tunables `npc.factions`), numbered apart from the world's own
        for d in defs.iter_mut().filter(|d| d.kind == Kind::Enemy && d.faction.is_none()) {
            d.faction = c.tunables.npc.factions.iter().position(|f| f.monster_data.contains(&d.monster_data)).map(|i| GLOBAL_FACTIONS + i as u32);
        }
        if !defs.is_empty() {
            table.insert(w.playfield.to_string(), defs);
        }
    }
    table
}

/// The invisible `invisible_monsterdata` look the cinematics' owners have.
const INVISIBLE_LOOK: u32 = 51057;

/// The NPCs that own cinematics, by playfield: a world's invisible trigger spawns with an NPC
/// template that a dialogue of that world names as its speaker ("Cinematic Start Oops" in Pirate
/// World). Live spawned them as characters (LMO_Captures.cap: monster data 51057, CARS group 5,
/// their NPC template) and opened those dialogues' windows with them as the speaker (knubot.rs);
/// they stay invisible and players don't click them (`SpawnDef::cinematic`). Left out where an
/// NPC of the name stands in the world (the jail's Swashbuckler: live's speaker was the one
/// players see).
pub fn cinematic_owners() -> HashMap<String, Vec<SpawnDef>> {
    cinematic_owners_of(crate::gamedata::get())
}

fn cinematic_owners_of(c: &Content) -> HashMap<String, Vec<SpawnDef>> {
    let mut table: HashMap<String, Vec<SpawnDef>> = HashMap::new();
    for (world_id, w) in &c.worlds {
        let spawns: Vec<&Spawn> = c.in_world(&c.spawns, world_id).filter(|s| !s.removed && s.wave.is_none() && s.appeared_at.is_none()).collect();
        let speaks = |name: &str| c.dialogues.values().any(|d| d.world.as_deref() == Some(world_id.as_str()) && d.npc.as_deref() == Some(name));
        let stands = |name: &str| spawns.iter().any(|s| s.kind != SpawnKind::Trigger && s.name == name && def_of(c, s).is_some());
        let defs: Vec<SpawnDef> = spawns
            .iter()
            .filter(|s| s.kind == SpawnKind::Trigger && speaks(&s.name) && !stands(&s.name))
            .filter_map(|s| def_of(c, &Spawn { kind: SpawnKind::Npc, ..(*s).clone() }))
            .filter(|d| d.monster_data == INVISIBLE_LOOK && d.npc_template.is_some())
            .map(|d| SpawnDef { cinematic: true, ..d })
            .collect();
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
    fn cinematic_owners_spawn() {
        let owners = cinematic_owners();
        let of = |pf: &str| owners.get(pf).into_iter().flatten().map(|d| (d.name.as_str(), d.npc_template)).collect::<Vec<_>>();
        // live's owners of 206 (Pirate World) and 822 (Isle of Yarr), with their NPC templates
        assert!(of("4600").contains(&("Cinematic Start Oops", Some(1014))));
        assert!(of("4200").contains(&("Cinematic Start Escaped From Jail", Some(2725))));
        // the jail's Swashbuckler speaks as the NPC players see; the invisible dummies own nothing
        assert!(of("4610").is_empty());
        assert!(owners.values().flatten().all(|d| !d.name.contains("Dummy")));
        assert!(owners.values().flatten().all(|d| d.cinematic && d.kind == Kind::Npc && d.monster_data == INVISIBLE_LOOK && d.cars_group == Some(5)));
        // (not in the table the world editor and the tutorial read)
        assert!(table().values().flatten().all(|d| !d.cinematic && d.name != "Cinematic Start Oops"));
    }

    #[test]
    fn speeds_match_the_live_capture() {
        // every NPC record of the live capture (walk, run, sprint, walk backwards, charge) against
        // what the server gives the spawns of its NPC template
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/live-capture/lmo-captures/playfield-objects.json");
        let live: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let stats = ["movement_walk_speed", "movement_run_speed", "movement_sprint_speed", "movement_walk_backwards_speed", "movement_charge_speed"];
        let mut ours: HashMap<u32, Vec<f32>> = HashMap::new();
        for d in table().into_values().chain(cinematic_owners().into_values()).flatten() {
            let speeds = crate::npc::speeds(&d);
            let v = [1396, 1397, 1398, 1399, 1400].map(|s| speeds.iter().find(|x| x.0 == s).unwrap().1).to_vec();
            if let Some(t) = d.npc_template {
                assert_eq!(ours.entry(t).or_insert_with(|| v.clone()), &v, "{} (template {t}): spawns differ", d.name);
            }
        }
        let (mut checked, mut wrong) = (0, Vec::new());
        for o in live["objects"].as_array().unwrap() {
            let core = &o["core"];
            let (Some(t), false) = (core["npc_stats"]["template_id"].as_u64(), core["firstname"].is_string()) else { continue };
            let Some(v) = ours.get(&(t as u32)) else { continue };
            let want: Vec<f32> = stats.iter().map(|s| core["stats"][*s].as_f64().unwrap() as f32).collect();
            checked += 1;
            if want.iter().zip(v).any(|(a, b)| (a - b).abs() > 0.001) {
                wrong.push((o["name"].clone(), t, want, v.clone()));
            }
        }
        assert!(wrong.is_empty(), "{wrong:?}");
        assert!(checked > 1000, "{checked} records checked");
        // the looks' own sets where the capture saw no such template
        let t = table();
        let of = |name: &str| t.values().flatten().find(|d| d.name == name).map(|d| crate::npc::speeds(d)[1].1);
        assert_eq!(of("Ogre"), Some(4.2));
        assert_eq!(of("Minotaur"), Some(2.8));
        // `!test speeds`
        for (name, run) in [("Musketeer", 4.2), ("Skeleton Axeman", 1.4), ("Forest Treeman", 2.2), ("Medieval Ogre Defender Melee War", 4.2), ("Harpy Raider", 4.2), ("Corrupted Chicken Suit Guy", 4.2)] {
            assert_eq!(of(name), Some(run), "{name}");
        }
    }

    #[test]
    fn conversation_portraits() {
        // the speaker's portrait is its icon (else the client's default, the DJ): live's Video
        // Game Guy and Swashbuckler had theirs, its Tavern Pirates none
        let t = table();
        let icon = |pf: &str, name: &str| t[pf].iter().find(|d| d.name == name).map(|d| d.icon);
        assert_eq!(icon("1300", "Video Game Guy"), Some(Some(443_659)));
        assert_eq!(icon("4200", "Swashbuckler"), Some(Some(443_660)));
        assert!(t.values().flatten().filter(|d| d.name == "Tavern Pirate").all(|d| d.icon.is_none()));
    }

    #[test]
    fn factions_everywhere() {
        let table = table();
        // skeletons and ghost pirates are rivals in the Grotto (4310) as on the Isle of Yarr
        let sides: std::collections::BTreeSet<u32> = table["4310"].iter().filter_map(|d| d.faction).collect();
        assert!(sides.len() >= 2, "{sides:?}");
        // the Isle of Yarr keeps its own sides
        assert!(table["4200"].iter().any(|d| d.faction == Some(0)));
    }

    #[test]
    fn own_loot_tables() {
        let c = crate::gamedata::get();
        // the usual tables aren't "own": bosses and champions still trade them by rank
        assert_eq!(own_loot(c, Kind::Enemy, Some("loot/enemy")), None);
        assert_eq!(own_loot(c, Kind::Smashable, Some("loot/smashable")), None);
        assert_eq!(own_loot(c, Kind::Enemy, Some("loot/boss")).as_deref(), Some("loot/boss"));
        // so far every template has its kind's usual table but the smashable chests and cages
        // (live: always an orb and 4-5 stars) and the Jail Warden (live: a boss bag, at a regular
        // enemy's rank)
        assert!(table().values().flatten().all(|d| d.loot.is_none()
            || d.loot.as_deref() == Some("loot/smashable-chest")
            || (d.name == "Jail Warden" && d.loot.as_deref() == Some("loot/boss"))));
        assert!(table().values().flatten().any(|d| d.loot.as_deref() == Some("loot/smashable-chest")));
    }

    /// The story bosses placed since (docs/TUNABLES.md "Story bosses"): (playfield, old or new name).
    const STORY_BOSSES: [(&str, &str); 9] = [
        ("2320", "Hades"),
        ("4300", "Captain Pete"),
        ("4310", "Captain Blackheart"),
        ("4310", "Captain Dreadleg"),
        ("4610", "Jail Warden"),
        ("5300", "The Jester"),
        ("5310", "Evil Sorcerer"),
        ("5310", "Pieromancer Elementius"),
        ("6300", "T-Rex"),
    ];

    /// Basic attacks changed since: (name, old spell, new spell). The Space Merc Engineer's
    /// 374158 (no client definition) is no longer cast; his shot is his basic attack.
    const ATTACKS_CHANGED: [(&str, u32, u32); 1] = [("Space Merc Engineer", 374_158, 374_159)];

    /// The Landing Beach's navy, friendly fighters before, are the ghosts' rival enemies since
    /// (2026-10-30: players couldn't attack them): (playfield, name).
    const NOW_ENEMIES: [(&str, &str); 3] = [("4200", "Commodore Moksey"), ("4200", "Dragoon"), ("4200", "Musketeer")];

    /// Spawns deleted since: (playfield, name).
    const REMOVED: [(&str, &str); 1] = [("6300", "Dino Raptor")];

    /// Pocket adventures given proposed spawns since (the Kingdom's entrances, wired up 2026-10-30).
    const ADDED: [&str; 4] = ["1652", "1673", "1682", "5610"];

    /// (the world editor's spawns weren't in the old files)
    #[test]
    fn same_as_the_old_files() {
        let mut new = table();
        new.retain(|pf, _| !ADDED.contains(&pf.as_str()));
        for defs in new.values_mut() {
            defs.retain(|d| d.source.as_deref() != Some("survey"));
            for d in defs.iter_mut() {
                // (the factions of every world came after)
                d.faction = d.faction.filter(|&f| f < GLOBAL_FACTIONS);
                if let Some((_, old, _)) = ATTACKS_CHANGED.iter().find(|(name, _, new)| d.name == *name && d.spell == *new) {
                    d.spell = *old;
                }
            }
        }
        let mut old = old_table();
        for (pf, defs) in old.iter_mut() {
            for d in defs.iter_mut().filter(|d| d.kind == Kind::Fighter && NOW_ENEMIES.contains(&(pf.as_str(), d.name.as_str()))) {
                d.kind = Kind::Enemy;
                d.faction = Some(1);
            }
        }
        for (pf, defs) in new.iter_mut().chain(old.iter_mut()) {
            defs.retain(|d| !STORY_BOSSES.contains(&(pf.as_str(), d.name.as_str())) && !REMOVED.contains(&(pf.as_str(), d.name.as_str())));
        }
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
