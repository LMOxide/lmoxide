//! Upgrade procs: minifigure upgrades whose effect was server script ("After you smash an
//! enemy, ...", "Every fifth hit ..."). They are content: each active upgrade's buff
//! (content/buffs/upgrade-*.json) lists `proc` effects (when: `on`, `every` (+ `count_for`),
//! `chance`, `below`, `lockout`, `channelling`, `target_slowed`; what: `then`, actions) and stat
//! effects (`orb_chance`, `elemental_affinity`, `cc_immunity_channelling`). The rules follow the
//! server Lua shipped with the client (research/lmoxide-notes/client-survey/stats-and-minigames.md
//! §2). The buffs the procs apply (content/buffs/proc-*.json) carry the client nano shown on the
//! player (`MagicBuffIIR_c`; the client applies its client-side Modify actions itself, so speed
//! boosts move the player faster), their time, `ends_on` (used up by the next hit or hit taken)
//! and the stats the server applies where it computes damage (npc.rs): `damage`
//! (FinalDamageMultiplier), `defense` (FinalDefenseMultiplier), `mitigation`, `cc_immunity`,
//! `build_speed`. A buff applied to the target (an enemy: Puff of Smoke, Dangerous Waters) shows
//! its nano there; its `damage` makes the enemy hit softer, its `interval` procs' `damage` hurt
//! it over time.
//!
//! Actions procs run: `apply_buff`, `heal`, `effect`, `knockback` (the target hit, or everyone
//! within `radius`; `pull`), `drop` (at the smashed enemy), and the commands
//! `upgrade.damage_field`, `upgrade.charge_up_add`, `upgrade.charge_up_release`,
//! `upgrade.retaliate`, `upgrade.oil_slick`. `not_hit_for` and `in_combat` procs are conditions,
//! not events: while they hold, the stats of the buffs they apply count (nothing is shown).
//!
//! Also here: the zone's element (Wrong Element and Elemental Affinity, tunables `elements`), the
//! swap immunity, crowd control immunity and the build speed.
//!
//! Not done: cooldown changes (Specialist, Technician: the client keeps the recharge times).

use crate::instance::Pf;
use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use lmoxide_content::{Action, BuffEffect, Proc, ProcOn, Stacking, Target};
use lmoxide_net::types::Id32;
use serde_json::Value;
use tracing::{debug, info};

use crate::{effects, world};

/// A proc of an active upgrade: its key (for counters and lockouts) and definition.
struct Active {
    key: String,
    proc: &'static Proc,
}

#[derive(Default)]
struct State {
    template: u32,
    upgrades: u32,
    level: u32,
    procs: Vec<Active>,
    /// Extra health orb chance in percent (`orb_chance`).
    orb_bonus: u64,
    normal: u32,
    special: u32,
    /// Events counted per proc (for `every`), and when each was last counted (`count_for`).
    counts: HashMap<String, (u32, Instant)>,
    /// Damage bonuses: (bonus, until; `None` = the next hit only).
    damage: Vec<(f32, Option<Instant>, u32)>,
    /// Defense: (FinalDefenseMultiplier bonus, flat mitigation, until; `None` = the next hit).
    defense: Vec<(f32, f32, Option<Instant>, u32)>,
    last_hit_taken: Option<Instant>,
    lockouts: HashMap<String, Instant>,
    /// Charge Up: charges and when they expire.
    charges: u32,
    charges_until: Option<Instant>,
    /// No crowd control until then (True Grit).
    cc_until: Option<Instant>,
    /// No crowd control while channelling a special (Stalwart).
    cc_channelling: bool,
    /// Can't be hurt until then (the minifigure swap).
    immune_until: Option<Instant>,
    /// The zone's element (ElementalType, 0 none).
    zone_element: u32,
    /// The zone buffs on the player: their nanos and stats.
    zone_nanos: Vec<u32>,
    zone_damage: f32,
    zone_defense: f32,
    zone_build: f32,
}

static PLAYERS: LazyLock<Mutex<HashMap<Id32, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Enemies weakened by a player's upgrade: (damage share taken off, until).
static WEAKENED: LazyLock<Mutex<HashMap<Id32, Vec<(f32, Instant)>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Spells the procs' own hits use (damage fields, damage over time, retaliation): they aren't
/// attacks, so they don't set off hit procs or use up one-hit bonuses.
static PROC_SPELLS: LazyLock<Mutex<HashSet<u32>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn is_proc_spell(spell: u32) -> bool {
    PROC_SPELLS.lock().unwrap().contains(&spell)
}

/// What a proc does after the players' lock is released.
enum Later {
    Heal(f32),
    Effect(u32),
    Drop(String),
    /// Knockback: the target, or everyone within `radius` of the owner.
    Knock { distance: f32, speed: f32, pull: bool, radius: Option<f32> },
    /// A buff on the target (an enemy).
    EnemyBuff { buff: String, duration: Option<Duration> },
    /// upgrade.damage_field: around the player or the smashed enemy
    Field { at_target: bool, effect: Option<u32>, radius: f32, ability_mod: f32, pulses: u32, every: Duration, spell: Option<u32> },
    /// upgrade.charge_up_release
    Release { radius: f32, ability_mod: f32 },
    /// upgrade.retaliate: a shot back at whoever hit the player
    Retaliate { item: u32, spell: u32, ability_mod: f32, range: f32, speed: f32 },
    /// upgrade.oil_slick: puddles where the player goes that weaken the enemies in them
    OilSlick { radius: f32, drop_every: Duration, for_: Duration, every: Duration, pulses: u32, weaken: f32, effect: Option<u32> },
}

/// Where a proc fires: the player's attack, its kill, the enemy that hit it ...
#[derive(Clone, Copy)]
struct Event {
    spell: u32,
    playfield: Option<Pf>,
    at: Option<[f32; 3]>,
    target: Option<Id32>,
}

impl Event {
    fn none(spell: u32) -> Event {
        Event { spell, playfield: None, at: None, target: None }
    }
}

/// The zone's element for `player` (stat ElementalType of the playfield it is entering); the
/// zone buffs follow at the next `set_active`.
pub fn set_zone(player: Id32, element: u32) {
    PLAYERS.lock().unwrap().entry(player).or_default().zone_element = element;
}

/// The player's active minifigure changed (or its upgrades, level or zone): returns the records
/// that show its zone buffs (Wrong Element, Elemental Affinity).
pub fn set_active(player: Id32, template: u32, upgrades: u32, level: u32) -> Vec<Bytes> {
    let mut procs = Vec::new();
    let mut orb_bonus = 0.0;
    let (mut affinity, mut cc_channelling) = (false, false);
    for b in crate::combat::active_upgrade_buffs(template, upgrades) {
        for (i, e) in b.effects.iter().enumerate() {
            match e {
                BuffEffect::Proc(p) => procs.push(Active { key: format!("{}#{i}", b.id), proc: p }),
                BuffEffect::Stat { stat, value, .. } => match stat.as_str() {
                    Some("orb_chance") => orb_bonus += share(value) * 100.0,
                    Some("elemental_affinity") => affinity = true,
                    Some("cc_immunity_channelling") => cc_channelling = true,
                    _ => {}
                },
                _ => {}
            }
        }
    }
    let spells = crate::collection::spells(template);
    let mut players = PLAYERS.lock().unwrap();
    let s = players.entry(player).or_default();
    let old = std::mem::take(s);
    *s = State {
        template,
        upgrades,
        level,
        procs,
        orb_bonus: orb_bonus.round() as u64,
        normal: spells.first().copied().unwrap_or(0),
        special: spells.get(1).copied().unwrap_or(0),
        lockouts: old.lockouts,
        last_hit_taken: old.last_hit_taken,
        cc_until: old.cc_until,
        immune_until: old.immune_until,
        cc_channelling,
        zone_element: old.zone_element,
        ..Default::default()
    };
    let mut records: Vec<Bytes> = old.zone_nanos.iter().map(|&n| unbuff(player, n)).collect();
    for b in zone_buffs(template, s.zone_element, affinity) {
        let Some(nano) = b.nano else { continue };
        for e in &b.effects {
            if let BuffEffect::Stat { stat, value, .. } = e {
                let v = share(value) as f32;
                match stat.as_str() {
                    Some("damage") => s.zone_damage += v,
                    Some("defense") => s.zone_defense += v,
                    Some("build_speed") => s.zone_build += v,
                    _ => {}
                }
            }
        }
        s.zone_nanos.push(nano);
        records.extend(buff(player, nano, None));
    }
    if !s.zone_nanos.is_empty() {
        info!(?player, template, zone = s.zone_element, nanos = ?s.zone_nanos, "zone element buffs");
    }
    records
}

/// The zone buffs of minifigure `template` in a zone of `zone` (0 none): the playfield
/// controller's Wrong Element when the elements differ, else Elemental Affinity's buff for its
/// role (with the upgrade).
fn zone_buffs(template: u32, zone: u32, affinity: bool) -> Vec<&'static lmoxide_content::Buff> {
    let c = crate::gamedata::get();
    let t = &c.tunables.elements;
    let (element, role) = crate::combat::element_role(template);
    if zone == 0 || element == 0 {
        return Vec::new();
    }
    let id = if element != zone {
        t.wrong_element.as_ref()
    } else if affinity {
        let role = match role {
            1 => "striker",
            2 => "defender",
            3 => "builder",
            _ => return Vec::new(),
        };
        t.affinity.get(role)
    } else {
        None
    };
    id.and_then(|id| c.buffs.get(id)).into_iter().collect()
}

/// A number or a share ("15%") as a fraction.
fn share(v: &Value) -> f64 {
    serde_json::from_value::<lmoxide_content::Ratio>(v.clone()).map_or(0.0, |r| r.fraction())
}

fn duration(v: &Value) -> Option<Duration> {
    serde_json::from_value::<lmoxide_content::Duration>(v.clone()).ok().map(|d| d.to_std())
}

pub fn left(player: Id32) {
    PLAYERS.lock().unwrap().remove(&player);
    effects::forget_buffs(player);
    crate::actions::forget_counters(player);
}

fn show(player: Id32, records: Vec<Bytes>) {
    world::send_playfield(player, true, world::Out::Records(records));
}

/// Shows buff `nano` on the player (`effects::show_buff`: replaced, not stacked; a timed one
/// ends on time). Buffs without an end last until `unbuff`.
fn buff(player: Id32, nano: u32, for_: Option<Duration>) -> Vec<Bytes> {
    effects::show_buff(player, nano, player, for_)
}

fn unbuff(player: Id32, nano: u32) -> Bytes {
    effects::forget_buff(player, nano);
    effects::buff_remove(player, nano, player)
}

/// Runs the procs of `s` that go off on `on` (`channelling`: during a channelled special;
/// `slowed`: the target is slowed or stunned), counting them for `every` and checking `chance`
/// and `lockout`.
fn fire(s: &mut State, player: Id32, on: &[ProcOn], channelling: bool, slowed: bool, records: &mut Vec<Bytes>, later: &mut Vec<Later>) {
    let now = Instant::now();
    let due: Vec<&'static Proc> = s
        .procs
        .iter()
        .filter(|a| on.contains(&a.proc.on) && (!a.proc.channelling || channelling) && (!a.proc.target_slowed || slowed))
        .map(|a| (a.key.clone(), a.proc))
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|(key, p)| {
            if let Some(n) = p.every {
                let (c, at) = s.counts.entry(key.clone()).or_insert((0, now));
                // a buff counter with a TimeExist: the count is gone after that long
                if p.count_for.is_some_and(|d| now.duration_since(*at) >= d.to_std()) {
                    *c = 0;
                }
                *c += 1;
                *at = now;
                if *c < n {
                    return false;
                }
                *c = 0;
            }
            if p.chance.is_some_and(|c| !crate::loot::chance_fraction(c.fraction())) {
                return false;
            }
            if let Some(lockout) = p.lockout {
                if s.lockouts.get(key).is_some_and(|t| *t > now) {
                    return false;
                }
                s.lockouts.insert(key.clone(), now + lockout.to_std());
            }
            true
        })
        .map(|(_, p)| p)
        .collect();
    for p in due {
        for action in &p.then {
            run(s, player, action, now, records, later);
        }
    }
}

/// One proc action.
fn run(s: &mut State, player: Id32, action: &Action, now: Instant, records: &mut Vec<Bytes>, later: &mut Vec<Later>) {
    match action {
        Action::ApplyBuff { buff: id, duration: d, target, .. } => {
            if *target == Target::Target {
                // a buff on the enemy the event is about
                later.push(Later::EnemyBuff { buff: id.clone(), duration: d.map(|d| d.to_std()) });
                return;
            }
            let Some(b) = crate::gamedata::get().buffs.get(id) else { return debug!(buff = id, "proc: no such buff") };
            let Some(nano) = b.nano else { return debug!(buff = id, "proc: buff without a nano") };
            let until_used = b.ends_on.is_some();
            let d = d.or(b.duration).map(|d| d.to_std());
            let until = if until_used { None } else { d.map(|d| now + d) };
            let (mut damage, mut defense, mut mitigation) = (0.0, 0.0, 0.0);
            for e in &b.effects {
                if let BuffEffect::Stat { stat, value, .. } = e {
                    match stat.as_str() {
                        Some("damage") => damage += share(value) as f32,
                        Some("defense") => defense += share(value) as f32,
                        Some("mitigation") => mitigation += share(value) as f32,
                        Some("cc_immunity") => s.cc_until = until.or(Some(now + crate::gamedata::get().tunables.combat.this_attack.to_std())),
                        _ => {}
                    }
                }
            }
            let stacks = s.defense.iter().filter(|d| d.3 == nano).count() + s.damage.iter().filter(|d| d.2 == nano).count();
            let room = b.stacking != Stacking::Stack || b.max_stacks.is_none_or(|m| stacks < m as usize);
            if room {
                if damage != 0.0 {
                    s.damage.push((damage, until, nano));
                }
                if defense != 0.0 || mitigation != 0.0 {
                    s.defense.push((defense, mitigation, until, nano));
                }
            }
            records.extend(buff(player, nano, if until_used { None } else { d }));
        }
        Action::Heal { amount, .. } => later.push(Later::Heal(amount.fraction() as f32)),
        Action::Effect { effect_package, .. } => later.push(Later::Effect(*effect_package)),
        Action::Knockback { distance, speed, pull, radius, .. } => {
            later.push(Later::Knock { distance: *distance, speed: speed.unwrap_or(25.0), pull: *pull, radius: *radius })
        }
        Action::Drop { loot } => later.push(Later::Drop(loot.clone())),
        Action::Command { name, args } => {
            let f = |k: &str| args.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let u = |k: &str| args.get(k).and_then(Value::as_u64).map(|v| v as u32);
            match name.as_str() {
                "upgrade.damage_field" => {
                    let every = args.get("every").and_then(duration).unwrap_or(Duration::from_secs(1));
                    let pulses = match args.get("for").and_then(duration) {
                        Some(total) => (total.as_secs_f32() / every.as_secs_f32()).ceil() as u32,
                        None => u("pulses").unwrap_or(1),
                    };
                    later.push(Later::Field {
                        at_target: args.get("at").and_then(Value::as_str) == Some("target"),
                        effect: u("effect_package"),
                        radius: f("radius"),
                        ability_mod: f("ability_mod"),
                        pulses,
                        every,
                        spell: u("spell"),
                    });
                }
                "upgrade.retaliate" => {
                    if let (Some(item), Some(spell)) = (u("item"), u("spell")) {
                        later.push(Later::Retaliate { item, spell, ability_mod: f("ability_mod"), range: f("range"), speed: f("speed").max(1.0) });
                    }
                }
                "upgrade.oil_slick" => {
                    let d = |k: &str, or: u64| args.get(k).and_then(duration).unwrap_or(Duration::from_millis(or));
                    later.push(Later::OilSlick {
                        radius: f("radius"),
                        drop_every: d("drop_every", 250),
                        for_: d("for", 1000),
                        every: d("every", 500),
                        pulses: u("pulses").unwrap_or(1),
                        weaken: args.get("weaken").map_or(0.0, share) as f32,
                        effect: u("effect_package"),
                    });
                }
                "upgrade.charge_up_add" => {
                    let stacks = u32_list(args.get("stacks"));
                    let for_ = args.get("for").and_then(duration).unwrap_or(Duration::from_secs(5));
                    if stacks.is_empty() {
                        return;
                    }
                    if s.charges > 0 {
                        records.push(unbuff(player, stacks[s.charges as usize - 1]));
                    }
                    s.charges = (s.charges + 1).min(stacks.len() as u32);
                    s.charges_until = Some(now + for_);
                    records.extend(buff(player, stacks[s.charges as usize - 1], Some(for_)));
                }
                "upgrade.charge_up_release" => {
                    let stacks = u32_list(args.get("stacks"));
                    let mods: Vec<f32> = args.get("ability_mods").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).map(|m| m as f32).collect();
                    if s.charges == 0 || stacks.is_empty() {
                        return;
                    }
                    let n = (s.charges as usize).min(stacks.len());
                    s.charges = 0;
                    records.push(unbuff(player, stacks[n - 1]));
                    let full = n == stacks.len();
                    let effect = args.get(if full { "effect_package_full" } else { "effect_package" }).and_then(Value::as_u64);
                    if let Some(effect) = effect {
                        records.push(effects::effect_package(player, effect as u32));
                    }
                    let radius = args.get("radius").and_then(Value::as_f64).unwrap_or(3.0) as f32;
                    later.push(Later::Release { radius, ability_mod: mods.get(n - 1).copied().unwrap_or(1.0) });
                }
                other => debug!(command = other, "proc: unknown command"),
            }
        }
        other => debug!(?other, "proc: action not run by procs"),
    }
}

fn u32_list(v: Option<&Value>) -> Vec<u32> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as u32).collect()
}

/// Runs `f` outside the caller's locks (npc.rs calls the procs with its NPCS lock held): on the
/// runtime when there is one, else now (unit tests).
fn defer(f: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn(async move { f() });
        }
        Err(_) => f(),
    }
}

/// Runs what `fire` left for after the lock: heals, drops, knockbacks, enemy buffs, damage
/// fields, Charge Up releases ...
fn finish(player: Id32, event: Event, later: Vec<Later>) {
    if later.is_empty() {
        return;
    }
    defer(move || {
        for l in later {
            finish_one(player, event, l);
        }
    });
}

fn finish_one(player: Id32, e: Event, l: Later) {
    let whereabouts = || world::whereabouts(player);
    let playfield = || e.playfield.or_else(|| whereabouts().map(|w| w.0));
    match l {
        Later::Heal(share) => heal_share(player, share),
        Later::Effect(package) => show(player, vec![effects::effect_package(player, package)]),
        Later::Drop(table) => {
            let (Some(pf), Some(at), Some(target)) = (e.playfield, e.at, e.target) else { return };
            let mut items = Vec::new();
            // (the power-up scripts roll 10281 and 211 at power 0)
            crate::loot::roll_table_into(&table, orb_bonus(player), pf, 0, &mut items);
            crate::loot::drop_items(player, pf, at, target, items);
        }
        Later::Knock { distance, speed, pull, radius } => {
            let Some((pf, here)) = whereabouts() else { return };
            match radius {
                Some(r) => {
                    for id in crate::npc::enemies_near(pf, here, r) {
                        crate::npc::knock(id, here, distance, pull, speed);
                    }
                }
                None => {
                    if let Some(target) = e.target.filter(|t| crate::npc::is_enemy(*t)) {
                        crate::npc::knock(target, here, distance, pull, speed);
                    }
                }
            }
        }
        Later::EnemyBuff { buff, duration } => {
            if let (Some(target), Some(pf)) = (e.target.filter(|t| crate::npc::is_enemy(*t)), playfield()) {
                enemy_buff(player, pf, target, &buff, duration);
            }
        }
        Later::Field { at_target, effect, radius, ability_mod, pulses, every, spell } => {
            let (Some(pf), Some((template, upgrades, level))) = (playfield(), active_of(player)) else { return };
            let damage = crate::combat::damage_with_mod(template, upgrades, e.spell, ability_mod, level);
            let spell = spell.unwrap_or(e.spell);
            if spell != e.spell {
                PROC_SPELLS.lock().unwrap().insert(spell);
            }
            let me = world::position_of(player);
            let here = if at_target { e.at.or(me) } else { me.or(e.at) };
            let Some(here) = here else { return };
            if let Some(effect) = effect {
                show(player, vec![effects::effect_package_at(player, effect, here)]);
            }
            tokio::spawn(async move {
                for _ in 0..pulses {
                    let center = if at_target { here } else { world::position_of(player).unwrap_or(here) };
                    crate::npc::hit_area(player, pf, center, radius, spell, damage);
                    tokio::time::sleep(every).await;
                }
            });
        }
        Later::Release { radius, ability_mod } => {
            let Some((template, upgrades, level)) = active_of(player) else { return };
            if let Some((pf, here)) = whereabouts() {
                // Charge Up: the charges go off around the player
                let damage = crate::combat::damage_with_mod(template, upgrades, e.spell, ability_mod, level);
                crate::npc::hit_area(player, pf, here, radius, e.spell, damage);
            }
        }
        Later::Retaliate { item, spell, ability_mod, range, speed } => {
            let (Some(attacker), Some((pf, here)), Some((template, upgrades, level))) = (e.target, whereabouts(), active_of(player)) else { return };
            let Some(at) = crate::npc::position(attacker) else { return };
            let d = ((at[0] - here[0]).powi(2) + (at[2] - here[2]).powi(2)).sqrt();
            if d > range || !crate::npc::is_enemy(attacker) {
                return;
            }
            PROC_SPELLS.lock().unwrap().insert(spell);
            let damage = crate::combat::damage_with_mod(template, upgrades, spell, ability_mod, level);
            crate::projectile::launch_visual(player, pf, item, speed, here, at);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs_f32(d / speed)).await;
                crate::npc::hit(player, attacker, spell, damage);
            });
        }
        Later::OilSlick { radius, drop_every, for_, every, pulses, weaken, effect } => {
            let Some(pf) = playfield() else { return };
            tokio::spawn(async move {
                let start = Instant::now();
                while start.elapsed() < for_ {
                    let Some(here) = world::position_of(player) else { return };
                    if let Some(effect) = effect {
                        show(player, vec![effects::effect_package_at(player, effect, here)]);
                    }
                    tokio::spawn(async move {
                        for _ in 0..pulses {
                            for id in crate::npc::enemies_near(pf, here, radius) {
                                weaken_enemy(id, weaken, every);
                            }
                            tokio::time::sleep(every).await;
                        }
                    });
                    tokio::time::sleep(drop_every).await;
                }
            });
        }
    }
}

/// Buff `id` from `player`'s upgrade on enemy `target`: its nano shown there, its `damage`
/// weakening the enemy's hits, its `interval` procs' `damage` hitting it while it lasts.
fn enemy_buff(player: Id32, pf: Pf, target: Id32, id: &str, d: Option<Duration>) {
    let Some(b) = crate::gamedata::get().buffs.get(id) else { return debug!(buff = id, "proc: no such buff") };
    let lasts = d.or(b.duration.map(|d| d.to_std())).unwrap_or(Duration::from_secs(2));
    let nano = b.nano.unwrap_or(0);
    if nano != 0 {
        world::send_playfield(target, false, world::Out::Records(effects::show_buff(target, nano, player, Some(lasts))));
    }
    for e in &b.effects {
        match e {
            BuffEffect::Stat { stat, value, .. } if stat.as_str() == Some("damage") => weaken_enemy(target, -(share(value) as f32), lasts),
            BuffEffect::Proc(p) if p.on == ProcOn::Interval => {
                let every = p.every_time.map_or(Duration::from_secs(1), |d| d.to_std());
                let mods: Vec<f32> = p
                    .then
                    .iter()
                    .filter_map(|a| match a {
                        Action::Damage { ability_mod, .. } => *ability_mod,
                        _ => None,
                    })
                    .collect();
                let Some((template, upgrades, level)) = active_of(player) else { continue };
                if mods.is_empty() {
                    continue;
                }
                let spell = if nano != 0 { nano } else { crate::collection::spells(template).first().copied().unwrap_or(0) };
                PROC_SPELLS.lock().unwrap().insert(spell);
                let ticks = (lasts.as_secs_f32() / every.as_secs_f32()).floor().max(1.0) as u32;
                tokio::spawn(async move {
                    for _ in 0..ticks {
                        tokio::time::sleep(every).await;
                        if crate::npc::health_of(target).is_none() || world::whereabouts(player).is_none_or(|w| w.0 != pf) {
                            return;
                        }
                        for m in &mods {
                            let damage = crate::combat::damage_with_mod(template, upgrades, spell, *m, level);
                            crate::npc::hit(player, target, spell, damage);
                        }
                    }
                });
            }
            _ => {}
        }
    }
}

/// Enemy `id` does `share` less damage for `lasts` (the strongest counts).
fn weaken_enemy(id: Id32, share: f32, lasts: Duration) {
    if share <= 0.0 {
        return;
    }
    let now = Instant::now();
    let mut w = WEAKENED.lock().unwrap();
    let list = w.entry(id).or_default();
    list.retain(|(_, until)| *until > now);
    list.push((share.min(1.0), now + lasts));
}

/// What enemy `id`'s hits are multiplied by now (1 unless an upgrade weakened it).
pub fn enemy_damage_factor(id: Id32) -> f32 {
    let now = Instant::now();
    let mut w = WEAKENED.lock().unwrap();
    let Some(list) = w.get_mut(&id) else { return 1.0 };
    list.retain(|(_, until)| *until > now);
    let strongest = list.iter().map(|(s, _)| *s).fold(0.0, f32::max);
    if list.is_empty() {
        w.remove(&id);
    }
    1.0 - strongest
}

/// `player`'s active minifigure: (template, upgrades, level).
pub fn active_of(player: Id32) -> Option<(u32, u32, u32)> {
    PLAYERS.lock().unwrap().get(&player).map(|s| (s.template, s.upgrades, s.level))
}

/// `player` casts `spell` (any MagicCommand): runs the attack procs.
pub fn cast(player: Id32, spell: u32) {
    let mut records = Vec::new();
    let mut later = Vec::new();
    {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&player) else { return };
        let normal = spell == s.normal;
        if !normal && spell != s.special {
            return;
        }
        if s.charges_until.is_some_and(|t| t <= Instant::now()) {
            s.charges = 0;
        }
        let which = if normal { ProcOn::NormalAttack } else { ProcOn::SpecialAttack };
        fire(s, player, &[ProcOn::Attack, which], false, false, &mut records, &mut later);
    }
    if !records.is_empty() {
        debug!(?player, spell, "attack procs");
        show(player, records);
    }
    finish(player, Event::none(spell), later);
}

/// `player` picked up a power-up (a health orb): the pickup procs (Pickup Artist).
pub fn picked_up(player: Id32) {
    let mut records = Vec::new();
    let mut later = Vec::new();
    {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&player) else { return };
        fire(s, player, &[ProcOn::Pickup], false, false, &mut records, &mut later);
    }
    if !records.is_empty() {
        show(player, records);
    }
    finish(player, Event::none(0), later);
}

/// The stats of the condition procs' buffs that count now (`not_hit_for`, `in_combat`; `stat`:
/// "damage", "defense", "build_speed").
fn conditional(s: &State, stat: &str, in_combat: bool) -> f32 {
    let buffs = &crate::gamedata::get().buffs;
    s.procs
        .iter()
        .filter(|a| match a.proc.on {
            ProcOn::NotHitFor => s.last_hit_taken.is_none_or(|t| a.proc.after.is_none_or(|after| t.elapsed() >= after.to_std())),
            ProcOn::InCombat => in_combat,
            _ => false,
        })
        .flat_map(|a| &a.proc.then)
        .filter_map(|action| match action {
            Action::ApplyBuff { buff, .. } => buffs.get(buff),
            _ => None,
        })
        .flat_map(|b| &b.effects)
        .filter_map(|e| match e {
            BuffEffect::Stat { stat: name, value, .. } if name.as_str() == Some(stat) => Some(share(value) as f32),
            _ => None,
        })
        .sum()
}

/// `attacker` hits `target` with `spell` for `damage` (`slowed`: the target is slowed or
/// stunned): returns the damage with the attacker's proc and zone bonuses (one-hit bonuses are
/// used up), and runs the per-hit procs. The procs' own hits (damage fields, damage over time)
/// get the bonuses but set off nothing.
pub fn outgoing(attacker: Id32, target: Id32, spell: u32, damage: u32, slowed: bool) -> u32 {
    let mut records = Vec::new();
    let mut later = Vec::new();
    let own = is_proc_spell(spell);
    let factor = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&attacker) else { return damage };
        let now = Instant::now();
        if !own {
            let which = if spell == s.normal { ProcOn::NormalHit } else { ProcOn::SpecialHit };
            fire(s, attacker, &[ProcOn::Hit, which], false, slowed, &mut records, &mut later);
        }
        s.damage.retain(|&(_, until, _)| until.is_none_or(|u| u > now));
        let bonus: f32 = s.damage.iter().map(|&(b, _, _)| b).sum::<f32>() + conditional(s, "damage", false) + s.zone_damage;
        if !own {
            for (_, _, nano) in s.damage.iter().filter(|(_, until, _)| until.is_none()) {
                records.push(unbuff(attacker, *nano));
            }
            s.damage.retain(|&(_, until, _)| until.is_some());
        }
        (1.0 + bonus).max(0.0)
    };
    if !records.is_empty() {
        show(attacker, records);
    }
    finish(attacker, Event { spell, playfield: None, at: None, target: Some(target) }, later);
    ((damage as f32) * factor).round() as u32
}

/// Extra health orb chance (percent) for `player`'s loot (`orb_chance`).
pub fn orb_bonus(player: Id32) -> u64 {
    PLAYERS.lock().unwrap().get(&player).map_or(0, |s| s.orb_bonus)
}

/// `attacker` defeated enemy `target` with `spell` at `at` on `playfield`.
pub fn killed(attacker: Id32, target: Id32, spell: u32, playfield: Pf, at: [f32; 3]) {
    let mut records = Vec::new();
    let mut later = Vec::new();
    let own = is_proc_spell(spell);
    {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&attacker) else { return };
        let on: &[ProcOn] = if spell != s.normal && !own { &[ProcOn::Kill, ProcOn::SpecialKill] } else { &[ProcOn::Kill] };
        fire(s, attacker, on, false, false, &mut records, &mut later);
    }
    if !records.is_empty() {
        show(attacker, records);
    }
    finish(attacker, Event { spell, playfield: Some(playfield), at: Some(at), target: Some(target) }, later);
}

/// `player` is hit by `attacker` for `damage` (before mitigation): returns it with the defense
/// procs (and uses up one-hit shields); `channelling` = during a channelled special.
pub fn incoming(player: Id32, attacker: Id32, damage: u32, channelling: bool) -> u32 {
    let mut records = Vec::new();
    let mut later = Vec::new();
    let damage = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&player) else { return damage };
        let now = Instant::now();
        let not_hit = conditional(s, "defense", false);
        s.last_hit_taken = Some(now);
        fire(s, player, &[ProcOn::Damaged], channelling, false, &mut records, &mut later);
        s.defense.retain(|&(_, _, until, _)| until.is_none_or(|u| u > now));
        let defense: f32 = s.defense.iter().map(|d| d.0).sum::<f32>() + not_hit;
        let mitigation: f32 = s.defense.iter().map(|d| d.1).sum();
        for (_, _, _, nano) in s.defense.iter().filter(|d| d.2.is_none()) {
            records.push(unbuff(player, *nano));
        }
        s.defense.retain(|d| d.2.is_some());
        ((damage as f32) * (1.0 - mitigation.min(0.9)) / (1.0 + defense).max(0.01)).round().max(1.0) as u32
    };
    if !records.is_empty() {
        show(player, records);
    }
    finish(player, Event { spell: 0, playfield: None, at: None, target: Some(attacker) }, later);
    damage
}

/// After `player` took damage: the low-health procs (`health_below`: Second Wind, True Grit,
/// Escape Artist).
pub fn after_damage(player: Id32, health: u32, max: u32) {
    if health == 0 || max == 0 {
        return;
    }
    let mut records = Vec::new();
    let mut later = Vec::new();
    {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&player) else { return };
        let share = health as f64 / max as f64;
        // only the procs whose threshold the health is under
        let procs = std::mem::take(&mut s.procs);
        let (low, rest): (Vec<Active>, Vec<Active>) =
            procs.into_iter().partition(|a| a.proc.on == ProcOn::HealthBelow && a.proc.below.is_some_and(|b| share < b.fraction()));
        s.procs = low;
        fire(s, player, &[ProcOn::HealthBelow], false, false, &mut records, &mut later);
        s.procs.extend(rest);
    }
    if !records.is_empty() {
        show(player, records);
    }
    finish(player, Event::none(0), later);
}

/// Whether `player` can't be crowd controlled now (True Grit; Stalwart while channelling).
pub fn cc_immune(player: Id32) -> bool {
    let (until, channelling) = PLAYERS.lock().unwrap().get(&player).map_or((None, false), |s| (s.cc_until, s.cc_channelling));
    until.is_some_and(|t| t > Instant::now()) || (channelling && crate::actions::is_channelling(player))
}

/// `player` can't be hurt for `d` (after a minifigure swap).
pub fn set_immune(player: Id32, d: Duration) {
    PLAYERS.lock().unwrap().entry(player).or_default().immune_until = Some(Instant::now() + d);
}

pub fn immune(player: Id32) -> bool {
    PLAYERS.lock().unwrap().get(&player).and_then(|s| s.immune_until).is_some_and(|t| t > Instant::now())
}

/// FinalDefenseMultiplier of the zone buffs, as the factor on the minifigure's defence.
pub fn defense_factor(player: Id32) -> f32 {
    PLAYERS.lock().unwrap().get(&player).map_or(1.0, |s| 1.0 + s.zone_defense)
}

/// How fast `player` builds (1 = the normal rate): zone buffs (Wrong Element, Builder
/// affinity) and Jury Rigging in combat.
pub fn build_factor(player: Id32) -> f32 {
    let in_combat = crate::npc::in_combat(player);
    PLAYERS.lock().unwrap().get(&player).map_or(1.0, |s| (1.0 + s.zone_build + conditional(s, "build_speed", in_combat)).max(0.05))
}

/// `!stats`: the player's upgrade procs, zone buffs and factors.
pub fn describe(player: Id32) -> Vec<String> {
    let players = PLAYERS.lock().unwrap();
    let Some(s) = players.get(&player) else { return vec!["no active minifigure".into()] };
    let buffs = &crate::gamedata::get().buffs;
    let zone: Vec<String> = s.zone_nanos.iter().filter_map(|n| buffs.values().find(|b| b.nano == Some(*n) && !b.id.starts_with("buff/upgrade")).map(|b| b.id.clone())).collect();
    let mut lines = vec![format!(
        "minifigure {} upgrades {:#x}: {} procs ({})",
        s.template,
        s.upgrades,
        s.procs.len(),
        s.procs.iter().map(|a| format!("{:?}", a.proc.on)).collect::<Vec<_>>().join(", ")
    )];
    lines.push(format!(
        "zone element {}: {}; damage x{:.2}, defence x{:.2}, build x{:.2}",
        s.zone_element,
        if zone.is_empty() { "no zone buff".into() } else { zone.join(", ") },
        1.0 + s.zone_damage,
        1.0 + s.zone_defense,
        1.0 + s.zone_build
    ));
    let now = Instant::now();
    let lockouts: Vec<String> = s.lockouts.iter().filter(|(_, t)| **t > now).map(|(k, t)| format!("{k} {}s", (*t - now).as_secs())).collect();
    if !lockouts.is_empty() {
        lines.push(format!("lockouts: {}", lockouts.join(", ")));
    }
    if s.cc_until.is_some_and(|t| t > now) || s.cc_channelling {
        lines.push(format!("crowd control immunity: {}", if s.cc_channelling { "while channelling" } else { "now" }));
    }
    lines
}

/// Pushes the enemies around `player` away (a level-up, a minifigure swap).
pub fn knock_around(player: Id32, k: &lmoxide_content::tunables::Knockaway) {
    let k = k.clone();
    defer(move || {
        let Some((pf, here)) = world::whereabouts(player) else { return };
        for id in crate::npc::enemies_near(pf, here, k.radius) {
            crate::npc::knock(id, here, k.distance, false, k.speed);
        }
    });
}

fn heal_share(player: Id32, share: f32) {
    let max = crate::npc::player_max_health(player);
    if max == 0 {
        return;
    }
    let health = crate::npc::heal_player(player, ((max as f32) * share).ceil() as u32);
    world::send_to(player, world::Out::Records(vec![crate::game::int32_stat(player, crate::game::STAT_HEALTH, health)]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_upgrade(prefix: &str) -> Option<(u32, u32)> {
        crate::gamedata::get().minifigures.values().find_map(|m| m.upgrades.iter().find(|u| u.buff.starts_with(prefix)).map(|u| (m.template, u.bit)))
    }

    #[test]
    fn berserker_doubles_every_fifth_hit() {
        let p = Id32::new(50_000, 1);
        let enemy = Id32::new(50_000, 9_999);
        let (template, bit) = with_upgrade("buff/upgrade-berserker").expect("a minifigure with Berserker");
        set_active(p, template, 1 << bit, 30);
        let normal = PLAYERS.lock().unwrap()[&p].normal;
        let hits: Vec<u32> = (0..10).map(|_| outgoing(p, enemy, normal, 100, false)).collect();
        assert_eq!(hits.iter().filter(|h| **h >= 200).count(), 2, "{hits:?}");
        assert!(hits[4] >= 200 && hits[9] >= 200, "{hits:?}");
        left(p);
    }

    #[test]
    fn counters_with_a_time_restart() {
        // Left Hook: three normal hits within 1 s of each other
        let c = crate::gamedata::get();
        let p = c.buffs["buff/upgrade-left-hook"].effects.iter().find_map(|e| match e {
            BuffEffect::Proc(p) => Some(p),
            _ => None,
        });
        let p = p.expect("Left Hook's proc");
        assert_eq!(p.on, ProcOn::NormalHit);
        assert_eq!((p.every, p.count_for.map(|d| d.to_std())), (Some(3), Some(Duration::from_secs(1))));
    }

    #[test]
    fn shields_up_stacks_five_times() {
        let (template, bit) = with_upgrade("buff/upgrade-shields-up").expect("a minifigure with Shields Up");
        let p = Id32::new(50_000, 2);
        let enemy = Id32::new(50_000, 9_998);
        set_active(p, template, 1 << bit, 30);
        let hits: Vec<u32> = (0..8).map(|_| incoming(p, enemy, 100, true)).collect();
        // +2% defense a hit while channelling, at most 5 (10%): 100 / 1.1
        assert_eq!(hits[7], 91, "{hits:?}");
        assert_eq!(incoming(p, enemy, 100, false), 91);
        left(p);
    }

    #[test]
    fn true_grit_below_21_percent_gives_cc_immunity() {
        let (template, bit) = with_upgrade("buff/upgrade-true-grit").expect("a minifigure with True Grit");
        let p = Id32::new(50_000, 3);
        set_active(p, template, 1 << bit, 30);
        after_damage(p, 22, 100);
        assert!(!cc_immune(p));
        after_damage(p, 20, 100);
        assert!(cc_immune(p));
        left(p);
    }

    #[test]
    fn wrong_element_and_affinity() {
        let c = crate::gamedata::get();
        // a minifigure with an element and the Elemental Affinity upgrade
        let (m, bit) = c
            .minifigures
            .values()
            .filter(|m| m.stats.element != 0)
            .find_map(|m| m.upgrades.iter().find(|u| u.buff.starts_with("buff/upgrade-elemental-affinity")).map(|u| (m, u.bit)))
            .expect("a minifigure with Elemental Affinity");
        let p = Id32::new(50_000, 4);
        let enemy = Id32::new(50_000, 9_997);
        let other = m.stats.element % 5 + 1;
        set_zone(p, other);
        assert!(!set_active(p, m.template, 0, 30).is_empty());
        // Wrong Element (the soft controller): -25% damage, the defence almost gone
        assert_eq!(outgoing(p, enemy, 1, 100, false), 75);
        assert!(defense_factor(p) < 0.05);
        set_zone(p, m.stats.element);
        set_active(p, m.template, 1 << bit, 30);
        let expected = match m.stats.role {
            1 => (110, 1.0),
            2 => (100, 1.2),
            _ => (100, 1.0),
        };
        assert_eq!((outgoing(p, enemy, 1, 100, false), (defense_factor(p) * 10.0).round() / 10.0), expected);
        set_zone(p, 0);
        assert!(set_active(p, m.template, 0, 30).len() <= 1);
        assert_eq!(outgoing(p, enemy, 1, 100, false), 100);
        left(p);
    }
}
