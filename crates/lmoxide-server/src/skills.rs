//! Skills from the content tree (content/skills, the projectile items in content/items), in the
//! shapes the spell code reads: each skill's client spell (stats by name, game action lists by
//! list number), projectile, dash and summon; projectile items' on-hit actions and hit radii;
//! the skills each enemy look casts (content/npcs).

use std::{collections::HashMap, sync::LazyLock};

use lmoxide_content::{GameAction, ItemKind, Skill};
use serde_json::{Value, json};

/// A game action as the spell code reads it (every field present).
fn action(a: &GameAction) -> Value {
    json!({ "type": a.kind, "client": a.client, "args": a.args, "criteria": a.criteria })
}

/// Server-made spells (content skills with no client spell) take ids from here up
/// (docs/CONTENT.md): the clients have no data for them, so they are never shown as buffs.
pub const SERVER_SPELLS_FROM: u32 = 2_000_000_000;

/// Whether `spell` is server-made (`SERVER_SPELLS_FROM`).
pub fn is_server_made(spell: u32) -> bool {
    spell >= SERVER_SPELLS_FROM
}

/// Skills with a definition in the client's data, by client spell.
static BY_SPELL: LazyLock<HashMap<u32, &'static Skill>> = LazyLock::new(|| {
    crate::gamedata::get()
        .skills
        .values()
        .filter(|s| !s.tags.iter().any(|t| t == "no-client-definition"))
        .filter_map(|s| Some((s.client.as_ref()?.spell, s)))
        .collect()
});

/// Every skill by client spell (with or without a client definition).
static ANY_BY_SPELL: LazyLock<HashMap<u32, &'static Skill>> =
    LazyLock::new(|| crate::gamedata::get().skills.values().filter_map(|s| Some((s.client.as_ref()?.spell, s))).collect());

/// The client spells: `{name, minifigure, stats, lists, effects}` by spell id (`effects`: what
/// their server Lua did, content skills `effects`).
pub fn spells() -> HashMap<u32, Value> {
    BY_SPELL
        .iter()
        .map(|(&id, s)| {
            let c = s.client.as_ref().expect("filtered");
            let lists: serde_json::Map<String, Value> = c.actions.iter().map(|(k, l)| (k.clone(), Value::Array(l.iter().map(action).collect()))).collect();
            let mut v = json!({ "name": s.name, "stats": c.stats, "lists": lists });
            if let Some(m) = &s.minifigure {
                v["minifigure"] = json!(m);
            }
            if !s.effects.is_empty() {
                v["effects"] = json!(s.effects);
            }
            (id, v)
        })
        .collect()
}

/// Projectile items: `{name, actions, effect_package}` by item template.
pub fn projectile_items() -> HashMap<u32, Value> {
    crate::gamedata::get()
        .items
        .values()
        .filter(|i| i.kind == ItemKind::Projectile && !i.actions.is_empty())
        .filter_map(|i| Some((i.template?, json!({ "name": i.name, "effect_package": i.effect_package, "actions": i.actions.iter().map(action).collect::<Vec<_>>() }))))
        .collect()
}

/// Hit radius (m) of projectile items.
pub fn projectile_radii() -> HashMap<u32, f32> {
    crate::gamedata::get().items.values().filter_map(|i| Some((i.template?, i.radius?))).collect()
}

/// A skill's range (m): its own `range`, or the client's AttackRange (cm).
pub fn range(spell: u32) -> Option<f32> {
    let s = ANY_BY_SPELL.get(&spell)?;
    s.range.or_else(|| s.client.as_ref()?.stats.get("AttackRange")?.as_f64().map(|cm| cm as f32 / 100.0))
}

/// Projectile parameters (the extractor's shape: `type`, `speed`, `item` ...) by spell. Only full
/// ones (with a `kind`): a summon's pet attack keeps just the shot's look (`summons`).
pub fn projectiles() -> HashMap<u32, Value> {
    ANY_BY_SPELL
        .iter()
        .filter_map(|(&id, s)| {
            let p = s.projectile.as_ref().filter(|p| p.kind.is_some())?;
            let mut v = json!({
                "attractor": p.attractor, "item": p.item, "offset_z": p.offset_z, "range": p.range,
                "ricochets": p.ricochets, "speed": p.speed, "type": p.kind,
            });
            if !p.charge_items.is_empty() {
                v["charge_items"] = json!(p.charge_items);
            }
            if let Some(i) = p.channel_interval {
                v["channel_interval"] = json!(i);
            }
            if let Some(b) = p.counter_buff {
                v["counter_buff"] = json!(b);
            }
            for (k, x) in &p.more {
                v[k] = x.clone();
            }
            Some((id, v))
        })
        .collect()
}

/// Dashes by spell (the extractor's shape).
pub fn dashes() -> HashMap<u32, Value> {
    ANY_BY_SPELL.iter().filter_map(|(&id, s)| Some((id, serde_json::to_value(s.dash.as_ref()?).expect("dash")))).collect()
}

/// Summons by spell: `{name, monster_data, lifetime, spell?, projectile?}`.
pub fn summons() -> HashMap<u32, Value> {
    let c = crate::gamedata::get();
    ANY_BY_SPELL
        .iter()
        .filter_map(|(&id, s)| {
            let summon = s.summon.as_ref()?;
            let npc = c.npcs.get(&summon.npc)?;
            let mut v = json!({
                "name": npc.name,
                "monster_data": npc.render.monster_data,
                "lifetime": summon.lifetime.or(npc.lifetime).map_or(15, |d| d.secs().round() as u64),
            });
            if let Some(attack) = npc.attack.as_ref().and_then(|a| c.skills.get(a)) {
                v["spell"] = json!(attack.client.as_ref().map(|c| c.spell));
                if let Some(p) = &attack.projectile {
                    v["projectile"] = json!({ "item": p.item, "speed": p.speed });
                }
            }
            Some((id, v))
        })
        .collect()
}

/// The spells each enemy look (monster data) casts: the `skills` most of its NPC templates have
/// (ties: the first's), so one template's own (the Alpha Wolf's Call to Arms, [`template_spells`])
/// doesn't spread to the others.
pub fn monster_spells() -> HashMap<u32, Vec<u32>> {
    let c = crate::gamedata::get();
    let mut lists: HashMap<u32, Vec<(Vec<u32>, usize)>> = HashMap::new();
    for n in c.npcs.values() {
        let (Some(md), false) = (n.render.monster_data, n.skills.is_empty()) else { continue };
        let spells: Vec<u32> = n.skills.iter().filter_map(|u| c.skills.get(&u.skill)?.client.as_ref().map(|c| c.spell)).collect();
        let seen = lists.entry(md).or_default();
        match seen.iter_mut().find(|(s, _)| *s == spells) {
            Some((_, count)) => *count += 1,
            None => seen.push((spells, 1)),
        }
    }
    lists
        .into_iter()
        .map(|(md, seen)| {
            let most = seen.iter().map(|(_, n)| *n).max().unwrap_or(0);
            (md, seen.into_iter().find(|(_, n)| *n == most).map(|(s, _)| s).unwrap_or_default())
        })
        .collect()
}

/// The spells each NPC template (`npc_template`, TDB 51000) casts when its own `skills` differ
/// from its look's ([`monster_spells`]): the Alpha Wolf's Call to Arms, which the Summoned
/// Wolves of the same look don't cast.
pub fn template_spells() -> HashMap<u32, Vec<u32>> {
    let c = crate::gamedata::get();
    let looks = monster_spells();
    c.npcs
        .values()
        .filter(|n| !n.skills.is_empty())
        .filter_map(|n| {
            let spells: Vec<u32> = n.skills.iter().filter_map(|u| c.skills.get(&u.skill)?.client.as_ref().map(|c| c.spell)).collect();
            (looks.get(&n.render.monster_data?) != Some(&spells)).then_some((n.render.npc_template?, spells))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str) -> Value {
        let path = format!("{}/../../data/archive/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Equal, numbers compared as f32 (content keeps f32 where the server reads f32).
    fn same_f32(a: &Value, b: &Value) -> bool {
        match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x as f32 == y as f32,
            _ => a == b,
        }
    }

    /// Old action entries with the fields the code reads filled in.
    fn full(a: &Value) -> Value {
        json!({ "type": a["type"], "client": a["client"], "args": a.get("args").cloned().unwrap_or(json!({})), "criteria": a.get("criteria").cloned().unwrap_or(json!([])) })
    }

    #[test]
    fn same_as_the_old_files() {
        let spells = spells();
        let mut checked = 0;
        for file in ["spell_actions.json", "npc_spells.json"] {
            let d = data(file);
            let list = if file == "npc_spells.json" { &d["spells"] } else { &d };
            for (k, v) in list.as_object().unwrap().iter().filter(|(k, _)| k.parse::<u32>().is_ok()) {
                let got = &spells[&k.parse::<u32>().unwrap()];
                assert_eq!(got["stats"], v["stats"], "{k} stats");
                assert_eq!(got.get("minifigure"), v.get("minifigure"), "{k} minifigure");
                let lists: serde_json::Map<String, Value> =
                    v["lists"].as_object().unwrap().iter().map(|(n, l)| (n.clone(), Value::Array(l.as_array().unwrap().iter().map(full).collect()))).collect();
                assert_eq!(got["lists"], Value::Object(lists), "{k} lists");
                checked += 1;
            }
            let items = projectile_items();
            let src = if file == "npc_spells.json" { &d["items"] } else { &d["items"] };
            for (k, v) in src.as_object().unwrap() {
                let got = &items[&k.parse::<u32>().unwrap()];
                let actions: Vec<Value> = v["actions"].as_array().unwrap().iter().map(full).collect();
                assert_eq!(got["actions"], json!(actions), "item {k}");
                assert_eq!(got["effect_package"], v["effect_package"], "item {k}");
            }
        }
        assert!(checked > 1100);
        let projectiles = projectiles();
        for (k, v) in data("projectiles.json").as_object().unwrap() {
            let got = &projectiles[&k.parse::<u32>().unwrap()];
            for (f, x) in v.as_object().unwrap() {
                assert!(same_f32(&got[f], x), "projectile {k} {f}: {} vs {x}", got[f]);
            }
        }
        let sizes = data("projectile_sizes.json");
        let radii = projectile_radii();
        for (k, r) in sizes["items"].as_object().unwrap() {
            assert_eq!(radii[&k.parse::<u32>().unwrap()], r.as_f64().unwrap() as f32, "radius {k}");
        }
        for (k, r) in sizes["spells"].as_object().unwrap() {
            assert!((range(k.parse().unwrap()).unwrap() as f64 - r.as_f64().unwrap()).abs() < 0.001, "range {k}");
        }
        let dashes = dashes();
        for (k, v) in data("dashes.json").as_object().unwrap() {
            let got = &dashes[&k.parse::<u32>().unwrap()];
            for (f, x) in v.as_object().unwrap().iter().filter(|(f, _)| *f != "name") {
                let same = got.get(f.as_str()).is_some_and(|g| same_f32(g, x)) || (x.is_null() && got.get(f.as_str()).is_none()) || (x == &json!([]) && got.get(f.as_str()).is_none()) || (x == &json!(false) && got.get(f.as_str()).is_none());
                assert!(same, "dash {k} {f}: {x} vs {:?}", got.get(f.as_str()));
            }
        }
        let summons = summons();
        // From Bad to Worse spawns the invisible anchor its clouds circle (SpawnMonster FXCX, as
        // in the live capture), not the pet summons.json guessed
        const NO_LONGER_SUMMONS: [&str; 1] = ["196059"];
        // pets that attack with their Lua pet spell (the owner casts the damage: actions.rs
        // `pet_attack`) where summons.json had none: Team Zombie, Gigantuan, Queen's Guard
        const LUA_PET_ATTACKS: [(&str, u32); 3] = [("641633", 642_562), ("641620", 654_694), ("663959", 667_155)];
        for (k, v) in data("summons.json").as_object().unwrap().iter().filter(|(k, _)| k.parse::<u32>().is_ok() && !NO_LONGER_SUMMONS.contains(&k.as_str())) {
            let got = &summons[&k.parse::<u32>().unwrap()];
            for f in ["name", "monster_data", "lifetime", "spell"] {
                // Queen's Guard brings the live capture's Queen's Guard for 10 s (U1XT,
                // npc/queen-s-guard-summon), not the guessed Royal Guard for 15 s
                if k == "663959" && (f == "name" || f == "lifetime") {
                    continue;
                }
                if let Some((_, spell)) = LUA_PET_ATTACKS.iter().find(|(s, _)| *s == k).filter(|_| f == "spell") {
                    assert_eq!(got.get(f), Some(&json!(spell)), "summon {k} lua pet attack");
                    continue;
                }
                assert_eq!(got.get(f), v.get(f), "summon {k} {f}");
            }
            if let Some(p) = v.get("projectile") {
                assert!(same_f32(&got["projectile"]["item"], &p["item"]) && same_f32(&got["projectile"]["speed"], &p["speed"]), "summon {k} projectile");
            }
        }
        let monsters = monster_spells();
        // spells added since: the Space Merc Engineer's drones (skill/merc-engineer-drones-castable: no
        // monster spell list names it), the Alien Mindslicer's abductions (the user, 2026-10-30)
        const ADDED: [(u32, u32); 2] = [(388_444, 374_162), (386_089, 378_128)];
        // the Witch Doctor's knife (XX_Stab) instead of the guessed Magic Potion (the user); the
        // Hunter's XX_Bolas instead of the guessed XX_Dart (the live capture); the Grenadier's
        // XX_Bomb (the pirate_grenadier_bomb) instead of XX_Grenade Throw's bottle (the user)
        // the Bot Battle Merc's stomp and missile instead of the generic Shot, the Grenadier's
        // grenade instead of an XX_Slash his look can't animate, the Space Merc Defender's
        // reflect (the user, 2026-10-30)
        const REPLACED: [(u32, &[u32]); 15] = [
            (201_479, &[194_198]),
            (201_477, &[194_199, 194_198]),
            (192_821, &[190_796, 190_612]),
            (359_212, &[361_429, 361_424]),
            (379_392, &[373_106, 373_290]),
            // spells the looks can animate (tools/content/anim_audit.py, 2026-10-30): Gorgon,
            // Swashbuckler, Treant Brown, Troll Hornblower, Troll Ranged; the Lizard Man the T-Rex's hit and rawr
            (98_178, &[100_144, 188_671]),
            (192_828, &[188_671]),
            (337_606, &[331_516, 16_101]),
            (331_167, &[191_516, 319_342]),
            (331_170, &[319_299]),
            (521_311, &[521_897, 521_607]),
            // the Minotaur's quick smash and Bull Charge (the user: he charges, has melee attacks)
            (636_593, &[319_298, 319_891, 3_202]),
            // the troll heavy hitter's own ground smash instead of a Rampage its look can't
            // animate; the Crooked Tooth Brute's charge, ground smash and swings (the user)
            (327_846, &[319_338, 319_946]),
            (157_082, &[100_331, 100_312, 100_799]),
            // the spiders' live kit (Skew, Web, Silk Thread, Taunt) for the generic Venom + Stab
            (231_991, &[197_776, 197_778, 198_074, 198_187]),
        ];
        for (k, v) in data("npc_spells.json")["monsters"].as_object().unwrap() {
            let mut want: Vec<u32> = serde_json::from_value(v["spells"].clone()).unwrap();
            if let Some((_, spells)) = REPLACED.iter().find(|(md, _)| k == &md.to_string()) {
                want = spells.to_vec();
            }
            want.extend(ADDED.iter().filter(|(md, _)| k == &md.to_string()).map(|(_, s)| *s));
            // Medusa's and the Lizard Man's kits on their NPC looks since (spawns.rs LOOKS_CHANGED)
            let md = match k.parse::<u32>().unwrap() {
                23_373 => 487_260,
                521_311 => 522_158,
                md => md,
            };
            assert_eq!(monsters.get(&md), Some(&want), "monster {k}");
        }
    }
}
