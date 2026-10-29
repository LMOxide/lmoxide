//! Definitions: texts, skills, items, upgrade buffs (with the procs procs.rs ran), minifigures,
//! loot tables and tunables (loot.rs, combat.rs, npc.rs), dialogues, achievements.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use serde_json::{Value, json};

use super::*;
use crate::action::{Action, Target};
use crate::record::GameAction;
use crate::tunables::*;
use crate::units::{Duration, Range, Ratio};

/// A client name without its "XX_" developer prefix.
fn clean(name: &str) -> String {
    let n = name.trim();
    n.strip_prefix("XX_")
        .or_else(|| n.strip_prefix("xx_"))
        .or_else(|| n.strip_prefix("XX "))
        .unwrap_or(n)
        .trim()
        .to_string()
}

/// The skill of a client spell, made (with only its id) if the data has no definition.
pub fn skill_ref(tree: &mut Tree, ctx: &mut Ctx, spell: u32, name: Option<&str>) -> String {
    if let Some(id) = ctx.skill.get(&spell) {
        return id.clone();
    }
    let name = name.map(clean).unwrap_or_else(|| format!("spell {spell}"));
    let id = ctx.ids.new_id_or_numbered("skill", &name, spell);
    ctx.skill.insert(spell, id.clone());
    tree.skills.insert(
        spell,
        Skill {
            id: id.clone(),
            name,
            client: Some(ClientSpell {
                spell,
                ..Default::default()
            }),
            source: Some(Source::Client),
            tags: vec!["no-client-definition".into()],
            ..Default::default()
        },
    );
    id
}

pub fn texts(tree: &mut Tree, data: &dyn Fn(&str) -> Result<Value>) -> Result<()> {
    let d = data("dialogues.json")?;
    let map = |v: &Value| -> BTreeMap<String, Value> {
        entries(v).map(|(k, v)| (k.clone(), v.clone())).collect()
    };
    tree.texts = Texts {
        texts: map(&d["texts"])
            .into_iter()
            .filter_map(|(k, v)| Some((k, v.as_str()?.to_string())))
            .collect(),
        answers: map(&d["answers"])
            .into_iter()
            .filter_map(|(k, v)| Some((k, u32_of(&v)?)))
            .collect(),
        keys: map(&d["keys"])
            .into_iter()
            .filter_map(|(k, v)| Some((k, v.as_str()?.to_string())))
            .collect(),
        names: map(&d["names"])
            .into_iter()
            .filter_map(|(k, v)| Some((k, v.as_str()?.to_string())))
            .collect(),
    };
    Ok(())
}

fn client_spell(spell: u32, v: &Value) -> ClientSpell {
    ClientSpell {
        spell,
        stats: entries(&v["stats"])
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        actions: entries(&v["lists"])
            .map(|(k, l)| {
                (
                    k.clone(),
                    l.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(strict)
                        .collect(),
                )
            })
            .collect(),
    }
}

pub fn skills(tree: &mut Tree, ctx: &mut Ctx, data: &dyn Fn(&str) -> Result<Value>) -> Result<()> {
    let sa = data("spell_actions.json")?;
    let ns = data("npc_spells.json")?;
    let projectiles = data("projectiles.json")?;
    let sizes = data("projectile_sizes.json")?;
    let dashes = data("dashes.json")?;
    let summons = data("summons.json")?;

    let mut defs: BTreeMap<u32, &Value> = BTreeMap::new();
    for (k, v) in entries(&sa).chain(entries(&ns["spells"])) {
        if let Ok(spell) = k.parse::<u32>() {
            defs.entry(spell).or_insert(v);
        }
    }
    for (&spell, v) in &defs {
        let name = clean(v["name"].as_str().unwrap_or(""));
        let id = ctx.ids.new_id_or_numbered("skill", &name, spell);
        ctx.skill.insert(spell, id.clone());
        tree.skills.insert(
            spell,
            Skill {
                id,
                name,
                client: Some(client_spell(spell, v)),
                minifigure: str_of(&v["minifigure"]),
                source: Some(Source::Client),
                ..Default::default()
            },
        );
    }
    for (k, p) in entries(&projectiles) {
        let Ok(spell) = k.parse::<u32>() else {
            continue;
        };
        let id = skill_ref(tree, ctx, spell, None);
        let mut more: BTreeMap<String, Value> =
            entries(p).map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut take = |k: &str| more.remove(k).unwrap_or(Value::Null);
        let projectile = Projectile {
            attractor: u32_of(&take("attractor")),
            item: u32_of(&take("item")),
            offset_z: f32_of(&take("offset_z")),
            range: f32_of(&take("range")),
            ricochets: u32_of(&take("ricochets")),
            speed: f32_of(&take("speed")),
            kind: u32_of(&take("type")),
            channel_interval: f32_of(&take("channel_interval")),
            charge_items: take("charge_items")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(u32_of)
                .collect(),
            counter_buff: u32_of(&take("counter_buff")),
            more,
            ..Default::default()
        };
        if let Some(s) = tree.skills.get_mut(&spell) {
            debug_assert_eq!(s.id, id);
            s.projectile = Some(projectile);
        }
    }
    for (k, r) in entries(&sizes["spells"]) {
        let Ok(spell) = k.parse::<u32>() else {
            continue;
        };
        let Some(s) = tree.skills.get_mut(&spell) else {
            continue;
        };
        // the client's AttackRange (cm) says the same; keep a range only where it doesn't
        let client = s
            .client
            .as_ref()
            .and_then(|c| c.stats.get("AttackRange"))
            .and_then(Value::as_f64)
            .map(|cm| cm / 100.0);
        if client.is_none_or(|c| (c - r.as_f64().unwrap_or(0.0)).abs() > 0.01) {
            s.range = f32_of(r);
        }
    }
    for (k, d) in entries(&dashes) {
        let Ok(spell) = k.parse::<u32>() else {
            continue;
        };
        let name = d["name"].as_str().map(str::to_string);
        skill_ref(tree, ctx, spell, name.as_deref());
        let mut d = d.clone();
        d.as_object_mut().map(|o| o.remove("name"));
        match serde_json::from_value::<Dash>(d) {
            Ok(dash) => tree.skills.get_mut(&spell).expect("made").dash = Some(dash),
            Err(e) => tree.notes.push(format!("dash {spell}: {e}")),
        }
    }
    for (k, s) in entries(&summons) {
        let Ok(spell) = k.parse::<u32>() else {
            continue;
        };
        let name = str_of(&s["name"]).unwrap_or_default();
        let skill = skill_ref(tree, ctx, spell, Some(&name));
        let npc_id = ctx.ids.new_id("npc", &name);
        let lifetime = s["lifetime"].as_f64().map(Duration);
        let mut npc = Npc {
            id: npc_id.clone(),
            name: name.clone(),
            kind: NpcKind::Summon,
            render: Render { monster_data: u32_of(&s["monster_data"]), ..Default::default() },
            lifetime,
            source: Some(Source::Server),
            comment: "The summoned monster's template isn't in the client data: its look was chosen by name (summons.json).".into(),
            ..Default::default()
        };
        if let Some(pet_spell) = u32_of(&s["spell"]) {
            let attack = skill_ref(tree, ctx, pet_spell, None);
            npc.attack = Some(attack);
            let skill = tree.skills.get_mut(&pet_spell).expect("made");
            if skill.projectile.is_none() && s["projectile"].is_object() {
                skill.projectile = Some(Projectile {
                    item: u32_of(&s["projectile"]["item"]),
                    speed: f32_of(&s["projectile"]["speed"]),
                    ..Default::default()
                });
            }
        }
        tree.npcs.push(npc);
        tree.skills.get_mut(&spell).expect("made").summon = Some(Summon {
            npc: npc_id,
            count: 1,
            lifetime,
            ..Default::default()
        });
        let _ = skill;
    }
    Ok(())
}

/// A loot bag's game action (`{"type": .., "GameAction_Level": 1, ...}`) as a [`GameAction`].
fn bag_action(a: &Value) -> GameAction {
    GameAction {
        kind: str_of(&a["type"]).unwrap_or_default(),
        client: false,
        args: entries(a)
            .filter(|(k, _)| *k != "type")
            .map(|(k, v)| {
                (
                    k.strip_prefix("GameAction_").unwrap_or(k).to_string(),
                    v.clone(),
                )
            })
            .collect(),
        criteria: vec![],
    }
}

/// A minifigure's name from its head part's ("DJ Head" -> "DJ").
fn head_name(parts_entry: &Value) -> Option<String> {
    let head = parts_entry["parts"].as_array()?.iter().find(|p| p["location"] == "Head")?;
    Some(head["part_name"].as_str()?.strip_suffix(" Head")?.to_string())
}

fn minifigure_names(data: &dyn Fn(&str) -> Result<Value>) -> Result<BTreeMap<u32, String>> {
    let stats = data("minifigure_stats.json")?;
    let parts = data("minifig_parts.json")?;
    let drops = data("minifigure_drops.json")?;
    let mut names = BTreeMap::new();
    for m in parts["minifigures"].as_array().into_iter().flatten() {
        if let (Some(t), Some(n)) = (u32_of(&m["minifigure_template_id"]), m["name"].as_str()) {
            names.insert(t, n.to_string());
        }
    }
    // the client data has no name for some (the DJ, 1922): their head part's ("DJ Head")
    for m in parts["minifigures"].as_array().into_iter().flatten() {
        if let (Some(t), Some(n)) = (u32_of(&m["minifigure_template_id"]), head_name(m)) {
            names.entry(t).or_insert(n);
        }
    }
    for m in drops["minifigures"].as_array().into_iter().flatten() {
        if let (Some(t), Some(n)) = (u32_of(&m["template"]), m["name"].as_str()) {
            names.entry(t).or_insert(n.to_string());
        }
    }
    for (k, m) in entries(&stats["minifigures"]) {
        if let Ok(t) = k.parse::<u32>() {
            let n = m["name"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("minifigure {t}"));
            names.entry(t).or_insert(n);
        }
    }
    Ok(names)
}

pub fn items(tree: &mut Tree, ctx: &mut Ctx, data: &dyn Fn(&str) -> Result<Value>) -> Result<()> {
    for (t, name) in minifigure_names(data)? {
        let id = ctx.ids.new_id_or_numbered("minifig", &name, t);
        ctx.minifig.insert(t, id);
    }
    let loot = data("loot_items.json")?;
    let sa = data("spell_actions.json")?;
    let ns = data("npc_spells.json")?;
    let sizes = data("projectile_sizes.json")?;

    let add = |tree: &mut Tree,
               ctx: &mut Ctx,
               template: u32,
               name: &str,
               make: &dyn Fn(String) -> Item| {
        if tree.items.contains_key(&template) {
            return;
        }
        let id = ctx.ids.new_id_or_numbered("item", &clean(name), template);
        ctx.item.insert(template, id.clone());
        tree.items.insert(template, make(id));
    };
    for t in loot["token_items"].as_array().into_iter().flatten() {
        let Some(template) = u32_of(&t["template"]) else {
            continue;
        };
        let name = str_of(&t["name"]).unwrap_or_default();
        add(tree, ctx, template, &name, &|id| Item {
            id,
            name: name.clone(),
            kind: ItemKind::Token,
            template: Some(template),
            dynel_type: u32_of(&t["dynel_type"]),
            source: Some(Source::Client),
            ..Default::default()
        });
        if let Some(token) = u32_of(&t["token"]) {
            let item = tree.items.get_mut(&template).expect("added");
            let give = TokenAmount {
                token,
                amount: t["amount"].as_i64(),
            };
            if !item.tokens.contains(&give) {
                item.tokens.push(give);
            }
        }
    }
    for p in loot["part_items"].as_array().into_iter().flatten() {
        let Some(template) = u32_of(&p["template"]) else {
            continue;
        };
        let name = str_of(&p["name"]).unwrap_or_default();
        add(tree, ctx, template, &name, &|id| Item {
            id,
            name: name.clone(),
            template: Some(template),
            source: Some(Source::Client),
            ..Default::default()
        });
        let item = tree.items.get_mut(&template).expect("added");
        item.kind = ItemKind::Part;
        if let Some(token) =
            u32_of(&p["token"]).filter(|t| !item.tokens.iter().any(|g| g.token == *t))
        {
            item.tokens.push(TokenAmount {
                token,
                amount: Some(1),
            });
        }
        item.location = str_of(&p["location"]);
        item.minifigure =
            u32_of(&p["minifigure_template"]).and_then(|m| ctx.minifig.get(&m).cloned());
    }
    for b in loot["minifigure_bags"].as_array().into_iter().flatten() {
        let Some(template) = u32_of(&b["bag_template"]) else {
            continue;
        };
        let name = format!(
            "{} {}",
            b["name"].as_str().unwrap_or("Minifigure Bag"),
            b["minifigure_name"].as_str().unwrap_or("")
        );
        let minifigure =
            u32_of(&b["minifigure_template"]).and_then(|m| ctx.minifig.get(&m).cloned());
        add(tree, ctx, template, &name, &|id| Item {
            id,
            name: str_of(&b["name"]).unwrap_or_default(),
            kind: ItemKind::MinifigBag,
            template: Some(template),
            minifigure: minifigure.clone(),
            controlled_loot_type: u32_of(&b["controlled_loot_type"]),
            actions: b["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(bag_action)
                .collect(),
            source: Some(Source::Client),
            ..Default::default()
        });
        if let (None, Some(m)) = (&minifigure, u32_of(&b["minifigure_template"])) {
            let item = tree.items.get_mut(&template).expect("added");
            item.comment = format!(
                "For minifigure template {m} ({}), which isn't a playable minifigure in the client's data.",
                b["minifigure_name"].as_str().unwrap_or("?")
            );
            tree.notes
                .push(format!("bag {template}: minifigure {m} isn't playable"));
        }
    }
    for c in loot["chests"].as_array().into_iter().flatten() {
        let Some(template) = u32_of(&c["template"]) else {
            continue;
        };
        let name = str_of(&c["name"]).unwrap_or_default();
        let loot_type = u32_of(&c["loot_type"]);
        add(tree, ctx, template, &name, &|id| Item {
            id,
            name: name.clone(),
            kind: ItemKind::Chest,
            template: Some(template),
            monster_data: u32_of(&c["monster_data"]),
            effect_package: u32_of(&c["effect_package"]),
            chest: Some(ChestInfo {
                loot_type,
                client_loot_table: u32_of(&c["loot_table"]),
                spawn_effect_package: u32_of(&c["spawn_effect_package"]),
                lua_script: u32_of(&c["lua_script"]),
                offset: Some(c["offset"].clone()).filter(|v| !v.is_null()),
                loot_power_level: None,
                loot: match loot_type {
                    Some(1) => Some("loot/chest".into()),
                    Some(2) => Some("loot/member-chest".into()),
                    _ => None,
                },
                unused: name.contains("Endless"),
            }),
            source: Some(Source::Client),
            ..Default::default()
        });
    }
    for (k, p) in entries(&sa["items"]).chain(entries(&ns["items"])) {
        let Ok(template) = k.parse::<u32>() else {
            continue;
        };
        let name = str_of(&p["name"]).unwrap_or_default();
        add(tree, ctx, template, &name, &|id| Item {
            id,
            name: clean(&name),
            kind: ItemKind::Projectile,
            template: Some(template),
            effect_package: u32_of(&p["effect_package"]),
            actions: p["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(strict)
                .collect(),
            source: Some(Source::Client),
            ..Default::default()
        });
    }
    for (k, r) in entries(&sizes["items"]) {
        let Ok(template) = k.parse::<u32>() else {
            continue;
        };
        let name = format!("projectile {template}");
        add(tree, ctx, template, &name, &|id| Item {
            id,
            kind: ItemKind::Projectile,
            template: Some(template),
            source: Some(Source::Client),
            ..Default::default()
        });
        tree.items.get_mut(&template).expect("added").radius = f32_of(r);
    }
    add(tree, ctx, HEALTH_ORB_ITEM, "Health Orb", &|id| Item {
        id,
        name: "Health Orb".into(),
        kind: ItemKind::HealthOrb,
        template: Some(HEALTH_ORB_ITEM),
        source: Some(Source::Server),
        ..Default::default()
    });
    tree.items.get_mut(&HEALTH_ORB_ITEM).expect("added").kind = ItemKind::HealthOrb;
    Ok(())
}

// loot.rs's items
const STAR_ITEM: u32 = 20634;
const HEALTH_ORB_ITEM: u32 = 20908;
const STARS_25_ITEM: u32 = 386269;
const STARS_100_ITEM: u32 = 386270;
const STARS_250_ITEM: u32 = 386271;
const STARS_1000_ITEM: u32 = 386272;
const HEALTH_POTION_ITEM: u32 = 399661;

/// A stat change effect.
fn stat(name: &str, op: StatOp, value: Value) -> BuffEffect {
    BuffEffect::Stat {
        stat: json!(name),
        op,
        value,
    }
}

fn apply(buff: &str, duration: Option<f64>) -> Action {
    Action::ApplyBuff {
        buff: buff.into(),
        amount: None,
        duration: duration.map(Duration),
        stacks: None,
        target: Target::Owner,
    }
}

fn heal(share: f64) -> Action {
    Action::Heal {
        amount: Ratio(share),
        target: Target::Owner,
    }
}

fn proc_(on: ProcOn, then: Vec<Action>) -> Proc {
    Proc {
        on,
        every: None,
        chance: None,
        below: None,
        after: None,
        every_time: None,
        lockout: None,
        channelling: false,
        count_for: None,
        target_slowed: false,
        then,
    }
}

pub fn buffs(tree: &mut Tree, ctx: &mut Ctx, data: &dyn Fn(&str) -> Result<Value>) -> Result<()> {
    let stats = data("minifigure_stats.json")?;

    // the buffs the procs apply (procs.rs), by nano
    struct ProcBuff {
        nano: u32,
        name: &'static str,
        effects: Vec<BuffEffect>,
        duration: Option<f64>,
        ends_on: Option<ProcOn>,
        max_stacks: Option<u32>,
    }
    let pb = |nano, name, effects, duration, ends_on| ProcBuff {
        nano,
        name,
        effects,
        duration,
        ends_on,
        max_stacks: None,
    };
    let mut proc_buffs = vec![
        pb(
            429_771,
            "Warrior Discipline",
            vec![stat("damage", StatOp::Add, json!(0.1))],
            Some(3.0),
            None,
        ),
        pb(
            429_698,
            "Block",
            vec![stat("mitigation", StatOp::Add, json!(0.1))],
            Some(3.0),
            None,
        ),
        pb(
            428_731,
            "Sprint",
            vec![stat("MovementMultiplier", StatOp::Add, json!(50))],
            Some(2.0),
            None,
        ),
        pb(
            423_293,
            "Speed on Kill",
            vec![stat("MovementMultiplier", StatOp::Add, json!(50))],
            Some(2.0),
            None,
        ),
        pb(
            423_317,
            "Berserker Strike",
            vec![stat("damage", StatOp::Add, json!(1.0))],
            Some(1.0),
            None,
        ),
        pb(
            423_295,
            "Brute Force",
            vec![stat("damage", StatOp::Add, json!(0.33))],
            None,
            Some(ProcOn::Hit),
        ),
        pb(
            531_641,
            "Boost Shield",
            vec![stat("defense", StatOp::Add, json!(0.33))],
            None,
            Some(ProcOn::Damaged),
        ),
        pb(
            447_637,
            "Harden Up",
            vec![stat("defense", StatOp::Add, json!(0.1))],
            Some(3.0),
            None,
        ),
        pb(
            531_119,
            "Powered Barrier",
            vec![stat("defense", StatOp::Add, json!(1.0))],
            None,
            Some(ProcOn::Damaged),
        ),
        pb(
            526_233,
            "Beat Up",
            vec![stat("damage", StatOp::Add, json!(0.1))],
            Some(3.0),
            None,
        ),
        pb(
            531_630,
            "Slippery Power",
            vec![stat("damage", StatOp::Add, json!(0.2))],
            None,
            Some(ProcOn::Damaged),
        ),
        pb(
            531_633,
            "Slippery Defense",
            vec![stat("defense", StatOp::Add, json!(0.2))],
            None,
            Some(ProcOn::Damaged),
        ),
        pb(
            380_600,
            "True Grit",
            vec![stat("mitigation", StatOp::Add, json!(0.9))],
            Some(5.0),
            None,
        ),
        pb(
            465_318,
            "Escape Artist",
            vec![stat("MovementMultiplier", StatOp::Add, json!(50))],
            Some(3.0),
            None,
        ),
        pb(
            481_807,
            "Skirmisher",
            vec![stat("MovementMultiplier", StatOp::Add, json!(50))],
            Some(2.0),
            None,
        ),
        pb(531_687, "Personal Space", vec![], Some(3.0), None),
        pb(
            446_818,
            "Fortify",
            vec![stat("defense", StatOp::Add, json!(0.02))],
            Some(1.5),
            None,
        ),
    ];
    // Fortify (Shields Up): at most 5 stacks
    proc_buffs.last_mut().expect("listed").max_stacks = Some(5);
    let mut by_nano = HashMap::new();
    for p in proc_buffs {
        let id = ctx.ids.new_id("buff", &format!("proc {}", p.name));
        by_nano.insert(p.nano, id.clone());
        tree.buffs.push(Buff {
            id,
            name: p.name.into(),
            nano: Some(p.nano),
            duration: p.duration.map(Duration),
            ends_on: p.ends_on,
            stacking: if p.max_stacks.is_some() { Stacking::Stack } else { Stacking::Refresh },
            max_stacks: p.max_stacks,
            effects: p.effects,
            source: Some(Source::Server),
            comment: "Applied by an upgrade's proc (procs.rs); values from the client's \"XX_passive generic\" nanos or assumed (docs/TUNABLES.md).".into(),
            ..Default::default()
        });
    }
    let b = |nano: u32| by_nano[&nano].clone();
    let every = |on, n, then| Proc {
        every: Some(n),
        ..proc_(on, then)
    };
    let low = |lockout: f64, then| Proc {
        below: Some(Ratio(0.2)),
        lockout: Some(Duration(lockout)),
        ..proc_(ProcOn::HealthBelow, then)
    };
    // upgrade ability -> its procs (procs.rs)
    let procs: HashMap<u32, Vec<BuffEffect>> = [
        (
            421_224,
            vec![BuffEffect::Proc(proc_(
                ProcOn::SpecialAttack,
                vec![apply(&b(429_771), None)],
            ))],
        ), // Lionhearted
        (
            421_250,
            vec![BuffEffect::Proc(proc_(
                ProcOn::SpecialAttack,
                vec![apply(&b(429_698), None)],
            ))],
        ), // Block
        (
            421_249,
            vec![BuffEffect::Proc(proc_(
                ProcOn::SpecialAttack,
                vec![apply(&b(428_731), None)],
            ))],
        ), // Smash and Dash
        (
            421_238,
            vec![BuffEffect::Proc(proc_(
                ProcOn::Kill,
                vec![apply(&b(423_293), None)],
            ))],
        ), // Hunter
        (
            421_231,
            vec![BuffEffect::Proc(every(
                ProcOn::Attack,
                5,
                vec![apply(&b(423_317), None)],
            ))],
        ), // Berserker
        (
            421_217,
            vec![BuffEffect::Proc(proc_(
                ProcOn::Kill,
                vec![apply(&b(423_295), None)],
            ))],
        ), // Brute Force
        (
            531_640,
            vec![BuffEffect::Proc(proc_(
                ProcOn::Kill,
                vec![apply(&b(531_641), None)],
            ))],
        ), // Boost Shield
        (
            447_636,
            vec![BuffEffect::Proc(every(
                ProcOn::NormalAttack,
                3,
                vec![apply(&b(447_637), None)],
            ))],
        ), // Harden Up
        (
            531_121,
            vec![BuffEffect::Proc(every(
                ProcOn::Attack,
                5,
                vec![apply(&b(531_119), None)],
            ))],
        ), // Powered Barrier
        (
            421_171,
            vec![BuffEffect::Proc(every(
                ProcOn::NormalAttack,
                3,
                vec![apply(&b(526_233), None)],
            ))],
        ), // Left Hook
        (
            531_629,
            vec![BuffEffect::Proc(Proc {
                after: Some(Duration(5.0)),
                ..proc_(ProcOn::NotHitFor, vec![apply(&b(531_630), None)])
            })],
        ), // Slippery
        (
            531_635,
            vec![BuffEffect::Proc(Proc {
                after: Some(Duration(5.0)),
                ..proc_(ProcOn::NotHitFor, vec![apply(&b(531_633), None)])
            })],
        ),
        (
            421_178, // Meltdown: an impact, then a pulse a second
            vec![BuffEffect::Proc(proc_(
                ProcOn::SpecialKill,
                vec![Action::Command {
                    name: "upgrade.damage_field".into(),
                    args: [
                        ("at", json!("self")),
                        ("effect_package", json!(442_041)),
                        ("radius", json!(3.0)),
                        ("ability_mod", json!(0.05)),
                        ("pulses", json!(6)),
                        ("every", json!("1s")),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                }],
            ))],
        ),
        (
            531_682, // Personal Space: where the enemy was smashed
            vec![BuffEffect::Proc(proc_(
                ProcOn::SpecialKill,
                vec![
                    apply(&b(531_687), None),
                    Action::Command {
                        name: "upgrade.damage_field".into(),
                        args: [
                            ("at", json!("target")),
                            ("radius", json!(3.0)),
                            ("ability_mod", json!(0.05)),
                            ("for", json!("3s")),
                            ("every", json!("250ms")),
                        ]
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                    },
                ],
            ))],
        ),
        (
            421_222,
            vec![BuffEffect::Proc(proc_(ProcOn::Kill, vec![heal(0.05)]))],
        ), // Reconstruct
        (
            421_243,
            vec![BuffEffect::Proc(proc_(ProcOn::Hit, vec![heal(0.01)]))],
        ), // Sponge
        (464_965, vec![BuffEffect::Proc(low(600.0, vec![heal(0.5)]))]), // Second Wind
        (
            380_599,
            vec![BuffEffect::Proc(low(300.0, vec![apply(&b(380_600), None)]))],
        ), // True Grit
        (
            465_317,
            vec![BuffEffect::Proc(low(300.0, vec![apply(&b(465_318), None)]))],
        ), // Escape Artist
        (
            481_806,
            vec![BuffEffect::Proc(proc_(
                ProcOn::NormalHit,
                vec![apply(&b(481_807), None)],
            ))],
        ), // Skirmisher
        (
            421_240,
            vec![BuffEffect::Proc(Proc {
                channelling: true,
                ..proc_(ProcOn::Damaged, vec![heal(0.02)])
            })],
        ), // Regenerate
        (
            443_935, // Shields Up: +2% defense a hit while channelling, at most 10%
            vec![BuffEffect::Proc(Proc {
                channelling: true,
                ..proc_(
                    ProcOn::Damaged,
                    vec![Action::ApplyBuff {
                        buff: b(446_818),
                        amount: None,
                        duration: None,
                        stacks: None,
                        target: Target::Owner,
                    }],
                )
            })],
        ),
        (
            443_056, // Charge Up: normals charge (5 at most, 5 s), the special releases them around the player
            vec![
                BuffEffect::Proc(proc_(
                    ProcOn::NormalAttack,
                    vec![Action::Command {
                        name: "upgrade.charge_up_add".into(),
                        args: [("stacks", json!([663_890, 663_891, 663_892, 663_893, 663_894])), ("for", json!("5s"))]
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), v))
                            .collect(),
                    }],
                )),
                BuffEffect::Proc(proc_(
                    ProcOn::SpecialAttack,
                    vec![Action::Command {
                        name: "upgrade.charge_up_release".into(),
                        args: [
                            (
                                "stacks",
                                json!([663_890, 663_891, 663_892, 663_893, 663_894]),
                            ),
                            ("ability_mods", json!([0.5, 0.75, 1.0, 1.25, 1.5])),
                            ("radius", json!(3.0)),
                            ("effect_package", json!(663_901)),
                            ("effect_package_full", json!(663_889)),
                        ]
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                    }],
                )),
            ],
        ),
        // Empowered Normals: the client raises the normal's AbilityMod by 1.2 (ModifyGameAction)
        (
            531_727,
            vec![BuffEffect::SkillMod { skill: "normal".into(), projectiles: None, cooldown: None, ability_mod: Some(Ratio(1.2)) }],
        ),
        (421_226, vec![stat("orb_chance", StatOp::Add, json!("15%"))]), // Resourceful
        (461_237, vec![stat("orb_chance", StatOp::Add, json!("15%"))]),
        (
            421_221, // Professional
            vec![BuffEffect::Proc(Proc {
                chance: Some(Ratio(0.25)),
                ..proc_(
                    ProcOn::SpecialKill,
                    vec![Action::Drop {
                        loot: "loot/health-orb".into(),
                    }],
                )
            })],
        ),
        (
            421_161,
            vec![BuffEffect::Proc(every(
                ProcOn::NormalAttack,
                3,
                vec![Action::Knockback {
                    distance: 4.0,
                    speed: None,
                    pull: false,
                    radius: None,
                    target: Target::Target,
                }],
            ))],
        ), // Knockback
    ]
    .into_iter()
    .collect();

    for (_, m) in entries(&stats["minifigures"]) {
        for u in m["upgrades"].as_array().into_iter().flatten() {
            let Some(ability) = u32_of(&u["ability"]) else {
                continue;
            };
            if ctx.buff.contains_key(&ability) {
                continue;
            }
            let full = str_of(&u["name"]).unwrap_or_default();
            let name = full.strip_prefix("Upgrade: ").unwrap_or(&full).to_string();
            let id = ctx
                .ids
                .new_id_or_numbered("buff", &format!("upgrade {name}"), ability);
            ctx.buff.insert(ability, id.clone());
            let mut buff = Buff {
                id,
                name: full.clone(),
                nano: Some(ability),
                source: Some(Source::Client),
                ..Default::default()
            };
            for e in u["effects"].as_array().into_iter().flatten() {
                let value = if e["GameAction_FloatValue1"].is_null() {
                    e["GameAction_Value"].clone()
                } else {
                    e["GameAction_FloatValue1"].clone()
                };
                let name = e["stat_name"]
                    .as_str()
                    .map(|n| json!(n))
                    .unwrap_or_else(|| e["stat"].clone());
                match e["type"].as_str() {
                    Some("Modify") => buff.effects.push(BuffEffect::Stat {
                        stat: name,
                        op: StatOp::Add,
                        value,
                    }),
                    Some("StatMultiplier") => buff.effects.push(BuffEffect::Stat {
                        stat: name,
                        op: StatOp::Mul,
                        value,
                    }),
                    _ => buff.effects.push(BuffEffect::Client(bag_action(e))),
                }
            }
            if let Some(p) = procs.get(&ability) {
                buff.effects.extend(p.iter().cloned());
                buff.source = Some(Source::Server);
            } else if u["effects"].as_array().is_none_or(|a| a.is_empty())
                && !full.starts_with("No upgrade")
            {
                buff.tags.push("todo".into());
                buff.comment =
                    "No client-side game actions: the effect was server script, not known yet."
                        .into();
            }
            tree.buffs.push(buff);
        }
    }
    Ok(())
}

pub fn minifigures(
    tree: &mut Tree,
    ctx: &mut Ctx,
    data: &dyn Fn(&str) -> Result<Value>,
) -> Result<()> {
    let stats = data("minifigure_stats.json")?;
    let parts = data("minifig_parts.json")?;
    let drops = data("minifigure_drops.json")?;
    let rarity = data("minifigure_rarity.json")?;
    let auras = data("minifigure_auras.json")?;
    let looks = data("minifigure_monster_data.json")?;
    let abilities = data("minifigure_abilities.json")?;
    let spells = data("minifigure_spells.json")?;
    let loot = data("loot_items.json")?;

    let parts_of: HashMap<u32, &Value> = parts["minifigures"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some((u32_of(&m["minifigure_template_id"])?, m)))
        .collect();
    let drop_of: HashMap<u32, &Value> = drops["minifigures"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some((u32_of(&m["template"])?, m)))
        .collect();
    let bag_of: HashMap<u32, u32> = loot["minifigure_bags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| u32_of(&b["controlled_loot_type"]) == Some(3))
        .filter_map(|b| {
            Some((
                u32_of(&b["minifigure_template"])?,
                u32_of(&b["bag_template"])?,
            ))
        })
        .collect();

    let attack = |tree: &mut Tree, ctx: &mut Ctx, a: &Value| -> Attack {
        let skill = skill_ref(
            tree,
            ctx,
            u32_of(&a["spell"]).unwrap_or(0),
            a["name"].as_str(),
        );
        let hits = a["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| Hit {
                skill: skill_ref(tree, ctx, u32_of(&h["spell"]).unwrap_or(0), None),
                kind: str_of(&h["type"]).unwrap_or_default(),
                ability_mod: f32_of(&h["ability_mod"]),
                damage_type: u32_of(&h["damage_type"]),
                via: h["via"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(u32_of)
                    .collect(),
            })
            .collect();
        Attack { name: a["name"].as_str().map(str::to_string).unwrap_or_default(), skill, hits }
    };

    let mut templates: Vec<u32> = entries(&stats["minifigures"])
        .filter_map(|(k, _)| k.parse().ok())
        .collect();
    templates.sort();
    for t in templates {
        let m = &stats["minifigures"][t.to_string()];
        let id = ctx.minifig[&t].clone();
        let p = parts_of.get(&t);
        let d = drop_of.get(&t);
        let name = id_name(&id, m, p, d);
        let normal = attack(tree, ctx, &m["normal"]);
        let special = attack(tree, ctx, &m["special"]);
        let mut mf = Minifigure {
            id,
            name: name.clone(),
            template: t,
            series: u32_of(&m["series"]).unwrap_or(0),
            series_index: u32_of(&m["series_index"]).unwrap_or(0),
            render: Render {
                monster_data: u32_of(&looks[t.to_string()]),
                ..Default::default()
            },
            stats: MinifigureStats {
                life: u32_of(&m["life"]).unwrap_or(0),
                power: u32_of(&m["power"]).unwrap_or(0),
                defense: u32_of(&m["defense"]).unwrap_or(0),
                creativity: u32_of(&m["creativity"]).unwrap_or(0),
                role: u32_of(&m["role"]).unwrap_or(0),
                element: u32_of(&m["element"]).unwrap_or(0),
            },
            normal,
            special,
            source: Some(Source::Client),
            ..Default::default()
        };
        if let Some(p) = p {
            mf.click_spells = [&p["left_click"], &p["right_click"]]
                .into_iter()
                .filter_map(u32_of)
                .collect();
            for part in p["parts"].as_array().into_iter().flatten() {
                if let Some(item) = u32_of(&part["part_template_id"]).and_then(|i| ctx.item.get(&i))
                {
                    mf.parts.push(MinifigurePart {
                        location: str_of(&part["location"]).unwrap_or_default(),
                        item: item.clone(),
                    });
                }
            }
        }
        let listed: Vec<u32> = spells[t.to_string()]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(u32_of)
            .collect();
        let attacks: Vec<u32> = [&m["normal"]["spell"], &m["special"]["spell"]]
            .into_iter()
            .filter_map(u32_of)
            .collect();
        if !listed.is_empty() && listed != attacks {
            tree.notes.push(format!(
                "minifigure {t}: minifigure_spells {listed:?} differ from its attacks {attacks:?}"
            ));
        }
        for u in m["upgrades"].as_array().into_iter().flatten() {
            let Some(buff) = u32_of(&u["ability"]).and_then(|a| ctx.buff.get(&a)) else {
                continue;
            };
            mf.upgrades.push(Upgrade {
                bit: u32_of(&u["bit"]).unwrap_or(0),
                slot: u32_of(&u["slot"]),
                buff: buff.clone(),
                ..Default::default()
            });
        }
        for a in abilities[t.to_string()].as_array().into_iter().flatten() {
            let (bit, ability) = (u32_of(&a[0]), u32_of(&a[1]));
            let known = mf.upgrades.iter().any(|u| {
                Some(u.bit) == bit && ability.and_then(|x| ctx.buff.get(&x)) == Some(&u.buff)
            });
            if !known {
                tree.notes.push(format!(
                    "minifigure {t}: ability {a} isn't among its upgrades"
                ));
            }
        }
        mf.aura_tags = auras[t.to_string()]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(u32_of)
            .collect();
        mf.bag = bag_of.get(&t).and_then(|b| ctx.item.get(b)).cloned();
        if let Some(d) = d {
            mf.obtain = Some(MinifigureObtain {
                source: str_of(&d["source"]).unwrap_or_default(),
                regions: d["worlds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| ctx.region.get(w.as_str()?).cloned())
                    .collect(),
                achievement: u32_of(&d["achievement"]),
                text: str_of(&d["text"]).unwrap_or_default(),
                rarity: rarity["minifigures"][&name].as_str().map(str::to_string),
                location_text: p.and_then(|p| u32_of(&p["drop_location_tdb"])),
                location_picture: p.and_then(|p| u32_of(&p["drop_location_picture"])),
            });
        }
        tree.minifigures.push(mf);
    }
    Ok(())
}

fn id_name(id: &str, m: &Value, p: Option<&&Value>, d: Option<&&Value>) -> String {
    m["name"]
        .as_str()
        .or_else(|| p.and_then(|p| p["name"].as_str()))
        .map(str::to_string)
        .or_else(|| p.and_then(|p| head_name(p)))
        .or_else(|| d.and_then(|d| d["name"].as_str()).map(str::to_string))
        .unwrap_or_else(|| id.to_string())
}

pub fn loot_and_tunables(
    tree: &mut Tree,
    ctx: &mut Ctx,
    data: &dyn Fn(&str) -> Result<Value>,
) -> Result<()> {
    let item = |t: u32| {
        ctx.item
            .get(&t)
            .cloned()
            .unwrap_or_else(|| format!("item/missing-{t}"))
    };
    let stars = |n: (u32, u32)| LootBundle {
        stars: Some(Range { min: n.0, max: n.1 }),
        ..Default::default()
    };
    let items = |t: u32| LootBundle {
        items: vec![item(t)],
        ..Default::default()
    };
    let always = |drop| LootEntry {
        drop,
        ..Default::default()
    };
    let chance = |p: f64, drop| LootEntry {
        chance: Some(Ratio(p)),
        drop,
        ..Default::default()
    };
    let world = |n| MinifigureDrop {
        from: MinifigureSource::World("world".into()),
        count: Range::one(n),
    };

    let chests: Vec<String> = tree
        .items
        .values()
        .filter(|i| {
            i.chest
                .as_ref()
                .is_some_and(|c| c.loot_type == Some(1) && !c.unused)
        })
        .map(|i| i.id.clone())
        .collect();
    let member_chests: Vec<String> = tree
        .items
        .values()
        .filter(|i| {
            i.chest
                .as_ref()
                .is_some_and(|c| c.loot_type == Some(2) && !c.unused)
        })
        .map(|i| i.id.clone())
        .collect();

    // loot.rs roll(): (stars, guaranteed bundle, orb %, potion %, part %, parts, bag %, chests) and bundles per mille
    struct Rule {
        key: &'static str,
        name: &'static str,
        stars: (u32, u32),
        bundle: Option<u32>,
        orb: f64,
        potion: f64,
        part: f64,
        parts: u32,
        bag: f64,
        chests: bool,
        bundles: &'static [(u32, f64)],
    }
    let rules = [
        Rule {
            key: "smashable",
            name: "Smashable",
            stars: (1, 2),
            bundle: None,
            orb: 0.0,
            potion: 0.03,
            part: 0.04,
            parts: 1,
            bag: 0.0,
            chests: false,
            bundles: &[(STARS_25_ITEM, 0.02)],
        },
        Rule {
            key: "enemy",
            name: "Enemy",
            stars: (3, 3),
            bundle: None,
            orb: 0.35,
            potion: 0.05,
            part: 0.06,
            parts: 1,
            bag: 0.01,
            chests: false,
            bundles: &[(STARS_25_ITEM, 0.03), (STARS_100_ITEM, 0.005)],
        },
        Rule {
            key: "champion",
            name: "Champion",
            stars: (6, 6),
            bundle: Some(STARS_25_ITEM),
            orb: 1.0,
            potion: 0.5,
            part: 1.0,
            parts: 1,
            bag: 0.15,
            chests: false,
            bundles: &[(STARS_100_ITEM, 0.2)],
        },
        Rule {
            key: "boss",
            name: "Boss",
            stars: (10, 10),
            bundle: Some(STARS_100_ITEM),
            orb: 1.0,
            potion: 1.0,
            part: 1.0,
            parts: 2,
            bag: 0.4,
            chests: true,
            bundles: &[(STARS_250_ITEM, 0.25), (STARS_1000_ITEM, 0.05)],
        },
        Rule {
            key: "chest",
            name: "Chest",
            stars: (5, 5),
            bundle: Some(STARS_25_ITEM),
            orb: 0.0,
            potion: 1.0,
            part: 1.0,
            parts: 2,
            bag: 0.25,
            chests: false,
            bundles: &[(STARS_100_ITEM, 0.2)],
        },
        Rule {
            key: "member-chest",
            name: "Member Chest",
            stars: (5, 5),
            bundle: Some(STARS_100_ITEM),
            orb: 0.0,
            potion: 1.0,
            part: 1.0,
            parts: 3,
            bag: 0.5,
            chests: false,
            bundles: &[(STARS_250_ITEM, 0.3), (STARS_1000_ITEM, 0.05)],
        },
    ];
    let mut tables = BTreeMap::new();
    for r in &rules {
        let mut e = vec![always(stars(r.stars))];
        e.extend(r.bundle.map(|b| always(items(b))));
        e.extend(r.bundles.iter().map(|&(b, p)| chance(p, items(b))));
        if r.orb > 0.0 {
            let mut orb = chance(
                r.orb,
                LootBundle {
                    hearts: Some(Range::one(1)),
                    ..Default::default()
                },
            );
            orb.when = Some(LootWhen {
                bonus_stat: Some("orb_chance".into()),
                ..Default::default()
            });
            e.push(orb);
        }
        if r.potion > 0.0 {
            e.push(chance(
                r.potion,
                LootBundle {
                    potions: Some(Range::one(1)),
                    ..Default::default()
                },
            ));
        }
        if r.part > 0.0 {
            e.push(chance(
                r.part,
                LootBundle {
                    minifig_parts: Some(world(r.parts)),
                    ..Default::default()
                },
            ));
        }
        if r.bag > 0.0 {
            e.push(chance(
                r.bag,
                LootBundle {
                    minifig_bags: Some(world(1)),
                    ..Default::default()
                },
            ));
        }
        if r.chests {
            let mut free = always(LootBundle {
                chest: chests.clone(),
                ..Default::default()
            });
            free.comment = "a free and a member chest side by side, as live placed them".into();
            e.push(free);
            let mut member = always(LootBundle {
                chest: member_chests.clone(),
                ..Default::default()
            });
            member.when = Some(LootWhen {
                members_only: true,
                ..Default::default()
            });
            e.push(member);
        }
        let id = format!("loot/{}", r.key);
        tables.insert(r.key.replace('-', "_"), id.clone());
        tree.loot.push(LootTable {
            id,
            name: r.name.into(),
            rolls: 1,
            entries: e,
            source: Some(Source::Server),
            comment: "loot.rs roll(): stars as recorded (smashables 1-2, enemies 3), the rest proposed (docs/TUNABLES.md).".into(),
            ..Default::default()
        });
    }
    tree.loot.push(LootTable {
        id: "loot/health-orb".into(),
        name: "Health Orb".into(),
        rolls: 1,
        entries: vec![always(LootBundle {
            hearts: Some(Range::one(1)),
            ..Default::default()
        })],
        source: Some(Source::Server),
        ..Default::default()
    });

    let stats = data("minifigure_stats.json")?;
    let progression = data("progression.json")?;
    let npc_life = data("npc_life.json")?;
    let rarity = data("minifigure_rarity.json")?;
    let map_u32 = |v: &Value| -> BTreeMap<String, u32> {
        entries(v)
            .filter_map(|(k, v)| Some((k.clone(), u32_of(v)?)))
            .collect()
    };
    tree.tunables = Tunables {
        combat: Combat {
            level: 30,
            health_per_level: 100.0,
            star_factor: 3.0,
            power_rating_all: 50.0,
            variance: Ratio(0.1),
            this_attack: Duration(1.0),
        },
        loot: Loot {
            share_range: 60.0,
            bag_lifetime: Duration(240.0),
            bag_public_after: Duration(90.0),
            bag_template: 1209,
            bag_dynel_type: 51322,
            bag_interaction_radius: 50,
            tables,
            items: [
                ("star", STAR_ITEM),
                ("health_orb", HEALTH_ORB_ITEM),
                ("potion", HEALTH_POTION_ITEM),
                ("stars_25", STARS_25_ITEM),
                ("stars_100", STARS_100_ITEM),
                ("stars_250", STARS_250_ITEM),
                ("stars_1000", STARS_1000_ITEM),
            ]
            .into_iter()
            .map(|(k, t)| (k.to_string(), item(t)))
            .collect(),
            ..Default::default()
        },
        npc: NpcTunables {
            regen_delay: Duration(10.0),
            regen_tick: Duration(1.0),
            regen_share: Ratio(0.05),
            regen_share_in_combat: Ratio(0.005),
        },
        progression: Progression {
            minifigure_xp: map_u32(&progression["minifigure_xp"]),
            rank_player_level: progression["rank_player_level"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(u32_of)
                .collect(),
            ..Default::default()
        },
        npc_life: NpcLife {
            interpolation: str_of(&npc_life["interpolation"]).unwrap_or_default(),
            base: map_u32(&npc_life["base"]),
        },
        rarity: Rarity {
            tiers: map_u32(&rarity["tiers"]),
            default: str_of(&rarity["default"]).unwrap_or_default(),
            part_locations: map_u32(&rarity["part_locations"]),
        },
        minifigure_model: stats["model"].clone(),
        // (written by hand, not imported)
        lost_creations: std::mem::take(&mut tree.tunables.lost_creations),
        health: std::mem::take(&mut tree.tunables.health),
        smashed: std::mem::take(&mut tree.tunables.smashed),
        elements: std::mem::take(&mut tree.tunables.elements),
        champions: std::mem::take(&mut tree.tunables.champions),
        story: std::mem::take(&mut tree.tunables.story),
        more: std::mem::take(&mut tree.tunables.more),
    };
    Ok(())
}

pub fn dialogues(
    tree: &mut Tree,
    ctx: &mut Ctx,
    data: &dyn Fn(&str) -> Result<Value>,
) -> Result<()> {
    let d = data("dialogues.json")?;
    for (order, x) in d["dialogues"].as_array().into_iter().flatten().enumerate() {
        let texts: Vec<u32> = x["texts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(u32_of)
            .collect();
        let npc = str_of(&x["npc"]);
        let id = ctx.ids.new_id(
            "dialogue",
            &format!(
                "{} {}",
                npc.as_deref().unwrap_or("dialogue"),
                texts.first().copied().unwrap_or(0)
            ),
        );
        tree.dialogues.push(Dialogue {
            id,
            name: npc.clone().unwrap_or_default(),
            texts,
            world: u32_of(&x["playfield"]).and_then(|p| ctx.world.get(&p).cloned()),
            npc_name: u32_of(&x["npc_name"]),
            npc,
            start: Some(x["start"].clone()).filter(|v| !v.is_null()),
            pos: vec3(&x["pos"]),
            follows: Some(x["follows"].clone()).filter(|v| !v.is_null()),
            seen_live: x["seen_live"].as_bool().unwrap_or(false),
            order: order as u32,
            source: source_of(&x["source"]),
            ..Default::default()
        });
    }
    Ok(())
}

pub fn achievements(
    tree: &mut Tree,
    ctx: &mut Ctx,
    data: &dyn Fn(&str) -> Result<Value>,
) -> Result<()> {
    let a = data("achievements.json")?;
    let list: Vec<&Value> = a["achievements"].as_array().into_iter().flatten().collect();
    for x in &list {
        let Some(lore) = u32_of(&x["id"]) else {
            continue;
        };
        let name = x["title"]
            .as_str()
            .or(x["name"].as_str())
            .unwrap_or("achievement");
        let id = ctx.ids.new_id_or_numbered("achievement", name, lore);
        ctx.achievement.insert(lore, id);
    }
    let ach = |v: &Value| u32_of(v).and_then(|l| ctx.achievement.get(&l).cloned());
    // the lore tree's names of areas that aren't playfields (1000 LEGO World, 2000 Mythology ...)
    let names: serde_json::Map<String, Value> =
        entries(&a["playfields"]).filter(|(k, _)| k.parse::<u32>().is_ok_and(|pf| !ctx.world.contains_key(&pf))).map(|(k, v)| (k.clone(), v.clone())).collect();
    if !names.is_empty() {
        tree.tunables.more.insert("achievements.area_names".into(), Value::Object(names));
    }
    for x in list {
        let Some(lore) = u32_of(&x["id"]) else {
            continue;
        };
        let id = ctx.achievement[&lore].clone();
        tree.achievements.push(Achievement {
            id: id.clone(),
            lore: Some(lore),
            parent: ach(&x["parent"]).filter(|p| *p != id),
            parent_lore: u32_of(&x["parent"]).filter(|p| !ctx.achievement.contains_key(p)),
            client_name: str_of(&x["name"]),
            title: str_of(&x["title"]),
            text: str_of(&x["text"]),
            points: u32_of(&x["points"]),
            kind: str_of(&x["kind"]).unwrap_or_default(),
            world: u32_of(&x["playfield"]).and_then(|p| ctx.world.get(&p).cloned()),
            find: str_of(&x["find"]),
            place: str_of(&x["place"]),
            stars: u32_of(&x["stars"]),
            target: Some(x["target"].clone()).filter(|v| !v.is_null()),
            children: x["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(&ach)
                .collect(),
            counter: x["counter"]
                .as_array()
                .and_then(|c| Some([u32_of(c.first()?)?, u32_of(c.get(1)?)?])),
            minifigure: u32_of(&x["minifigure"]).and_then(|m| ctx.minifig.get(&m).cloned()),
            n: u32_of(&x["n"]),
            counts: Some(x["counts"].clone()).filter(|v| !v.is_null()),
            series: u32_of(&x["series"]),
            invisible: x["invisible"].as_bool().unwrap_or(false),
            source: Some(Source::Client),
            ..Default::default()
        });
    }
    Ok(())
}
