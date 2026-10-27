//! Upgrade procs: minifigure upgrades whose effect was server script ("After you smash an
//! enemy, ...", "Every fifth attack ..."). They are content: each active upgrade's buff
//! (content/buffs/upgrade-*.json) lists `proc` effects (when: `on`, `every`, `chance`, `below`,
//! `lockout`, `channelling`; what: `then`, actions) and stat effects (`orb_chance`). The buffs
//! the procs apply (content/buffs/proc-*.json) carry the client nano shown on the player
//! (`MagicBuffIIR_c`; the client applies its client-side Modify actions itself, so speed boosts
//! move the player faster), their time, `ends_on` (used up by the next hit or hit taken) and
//! the stats the server applies where it computes damage (npc.rs): `damage`
//! (FinalDamageMultiplier), `defense` (FinalDefenseMultiplier), `mitigation`. Values the client
//! data doesn't give (heal amounts, durations of buffs without TimeExist) are assumptions,
//! listed in docs/TUNABLES.md.
//!
//! Actions procs run: `apply_buff`, `heal`, `knockback` (this attack's hits), `drop` (at the
//! smashed enemy), and the commands `upgrade.damage_field`, `upgrade.charge_up_add`,
//! `upgrade.charge_up_release`. `not_hit_for` procs are conditions, not events: while the
//! holder hasn't been hit for `after`, their buffs' stats count (not shown).
//!
//! Not done: pulls and the special attacks' own knockbacks (Harpoon, Crowd Control, Equalizing
//! Burst, Vortex), cooldown changes (Specialist, Technician: the client keeps the recharge
//! times), Elemental Affinity (location elements).

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use lmoxide_content::{Action, BuffEffect, Proc, ProcOn, Stacking};
use lmoxide_net::types::Id32;
use serde_json::Value;
use tracing::debug;

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
    /// Events counted per proc (for `every`).
    counts: HashMap<String, u32>,
    /// Damage bonuses: (bonus, until; `None` = the next hit only).
    damage: Vec<(f32, Option<Instant>, u32)>,
    /// Defense: (FinalDefenseMultiplier bonus, flat mitigation, until; `None` = the next hit).
    defense: Vec<(f32, f32, Option<Instant>, u32)>,
    last_hit_taken: Option<Instant>,
    lockouts: HashMap<String, Instant>,
    /// Charge Up: charges and when they expire.
    charges: u32,
    charges_until: Option<Instant>,
    /// Knockback: this attack's hits push enemies away until then, this far.
    knockback_until: Option<(Instant, f32)>,
}

static PLAYERS: LazyLock<Mutex<HashMap<Id32, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// What a proc does after the players' lock is released.
enum Later {
    Heal(f32),
    Drop(String),
    /// upgrade.damage_field: around the player or the smashed enemy
    Field { at_target: bool, effect: Option<u32>, radius: f32, ability_mod: f32, pulses: u32, every: Duration },
    /// upgrade.charge_up_release
    Release { radius: f32, ability_mod: f32 },
}

/// Where a proc fires: the player's attack, its kill, ...
#[derive(Clone, Copy)]
struct Event {
    spell: u32,
    playfield: u32,
    at: [f32; 3],
    target: Id32,
}

/// The player's active minifigure changed (or its upgrades).
pub fn set_active(player: Id32, template: u32, upgrades: u32, level: u32) {
    let mut procs = Vec::new();
    let mut orb_bonus = 0.0;
    for b in crate::combat::active_upgrade_buffs(template, upgrades) {
        for (i, e) in b.effects.iter().enumerate() {
            match e {
                BuffEffect::Proc(p) => procs.push(Active { key: format!("{}#{i}", b.id), proc: p }),
                BuffEffect::Stat { stat, value, .. } if stat.as_str() == Some("orb_chance") => orb_bonus += share(value) * 100.0,
                _ => {}
            }
        }
    }
    let spells = crate::collection::spells(template);
    let mut players = PLAYERS.lock().unwrap();
    let s = players.entry(player).or_default();
    *s = State {
        template,
        upgrades,
        level,
        procs,
        orb_bonus: orb_bonus.round() as u64,
        normal: spells.first().copied().unwrap_or(0),
        special: spells.get(1).copied().unwrap_or(0),
        lockouts: std::mem::take(&mut s.lockouts),
        last_hit_taken: s.last_hit_taken,
        ..Default::default()
    };
}

/// A number or a share ("15%") as a fraction.
fn share(v: &Value) -> f64 {
    serde_json::from_value::<lmoxide_content::Ratio>(v.clone()).map_or(0.0, |r| r.fraction())
}

fn duration(v: &Value) -> Option<Duration> {
    serde_json::from_value::<lmoxide_content::Duration>(v.clone()).ok().map(|d| d.to_std())
}

fn this_attack() -> Duration {
    crate::gamedata::get().tunables.combat.this_attack.to_std()
}

pub fn left(player: Id32) {
    PLAYERS.lock().unwrap().remove(&player);
    effects::forget_buffs(player);
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

/// Runs the procs of `s` that go off on `on` (`channelling`: during a channelled special),
/// counting them for `every` and checking `chance` and `lockout`.
fn fire(s: &mut State, player: Id32, on: &[ProcOn], channelling: bool, records: &mut Vec<Bytes>, later: &mut Vec<Later>) {
    let now = Instant::now();
    let due: Vec<&'static Proc> = s
        .procs
        .iter()
        .filter(|a| on.contains(&a.proc.on) && (!a.proc.channelling || channelling))
        .map(|a| (a.key.clone(), a.proc))
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|(key, p)| {
            if let Some(n) = p.every {
                let c = s.counts.entry(key.clone()).or_default();
                *c += 1;
                if *c % n != 0 {
                    return false;
                }
            }
            if p.chance.is_some_and(|c| !crate::loot::chance((c.fraction() * 100.0).round() as u64)) {
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
        Action::ApplyBuff { buff: id, duration: d, .. } => {
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
        Action::Knockback { distance, .. } => s.knockback_until = Some((now + this_attack(), *distance)),
        Action::Drop { loot } => later.push(Later::Drop(loot.clone())),
        Action::Command { name, args } => match name.as_str() {
            "upgrade.damage_field" => {
                let f = |k: &str| args.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                let every = args.get("every").and_then(duration).unwrap_or(Duration::from_secs(1));
                let pulses = match args.get("for").and_then(duration) {
                    Some(total) => (total.as_secs_f32() / every.as_secs_f32()).ceil() as u32,
                    None => args.get("pulses").and_then(Value::as_u64).unwrap_or(1) as u32,
                };
                later.push(Later::Field {
                    at_target: args.get("at").and_then(Value::as_str) == Some("target"),
                    effect: args.get("effect_package").and_then(Value::as_u64).map(|e| e as u32),
                    radius: f("radius"),
                    ability_mod: f("ability_mod"),
                    pulses,
                    every,
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
        },
        other => debug!(?other, "proc: action not run by procs"),
    }
}

fn u32_list(v: Option<&Value>) -> Vec<u32> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as u32).collect()
}

/// Runs what `fire` left for after the lock: heals, drops, damage fields, Charge Up releases.
fn finish(player: Id32, event: Option<Event>, later: Vec<Later>) {
    for l in later {
        match l {
            Later::Heal(share) => heal_share(player, share),
            Later::Drop(table) => {
                let Some(e) = event else { continue };
                let mut items = Vec::new();
                crate::loot::roll_table_into(&table, orb_bonus(player), e.playfield, &mut items);
                crate::loot::drop_items(player, e.playfield, e.at, e.target, items);
            }
            Later::Field { at_target, effect, radius, ability_mod, pulses, every } => {
                let (Some(e), Some((template, upgrades, level))) = (event, active_of(player)) else { continue };
                let damage = crate::combat::damage_with_mod(template, upgrades, e.spell, ability_mod, level);
                let (playfield, spell) = (e.playfield, e.spell);
                let here = if at_target { e.at } else { world::position_of(player).unwrap_or(e.at) };
                if let Some(effect) = effect {
                    show(player, vec![effects::effect_package_at(player, effect, here)]);
                }
                tokio::spawn(async move {
                    for _ in 0..pulses {
                        let center = if at_target { here } else { world::position_of(player).unwrap_or(here) };
                        crate::npc::hit_area(player, playfield, center, radius, spell, damage);
                        tokio::time::sleep(every).await;
                    }
                });
            }
            Later::Release { radius, ability_mod } => {
                let (Some(e), Some((template, upgrades, level))) = (event, active_of(player)) else { continue };
                if let Some((playfield, here)) = world::whereabouts(player) {
                    // Charge Up: the charges go off around the player
                    let damage = crate::combat::damage_with_mod(template, upgrades, e.spell, ability_mod, level);
                    crate::npc::hit_area(player, playfield, here, radius, e.spell, damage);
                }
            }
        }
    }
}

fn active_of(player: Id32) -> Option<(u32, u32, u32)> {
    PLAYERS.lock().unwrap().get(&player).map(|s| (s.template, s.upgrades, s.level))
}

/// `player` casts `spell` (any MagicCommand): counts attacks for "every n-th attack" procs and
/// runs the attack procs.
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
        fire(s, player, &[ProcOn::Attack, which], false, &mut records, &mut later);
    }
    if !records.is_empty() {
        debug!(?player, spell, "attack procs");
        show(player, records);
    }
    finish(player, Some(Event { spell, playfield: 0, at: [0.0; 3], target: player }), later);
}

/// Whether `attacker`'s current attack knocks the enemies it hits away (Knockback), and how far.
pub fn knockback(attacker: Id32) -> Option<f32> {
    let players = PLAYERS.lock().unwrap();
    players.get(&attacker)?.knockback_until.filter(|(t, _)| *t > Instant::now()).map(|(_, d)| d)
}

/// The stats of the `not_hit_for` procs' buffs that count now (`stat`: "damage" or "defense").
fn while_not_hit(s: &State, stat: &str) -> f32 {
    let buffs = &crate::gamedata::get().buffs;
    s.procs
        .iter()
        .filter(|a| a.proc.on == ProcOn::NotHitFor)
        .filter(|a| s.last_hit_taken.is_none_or(|t| a.proc.after.is_none_or(|after| t.elapsed() >= after.to_std())))
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

/// `attacker` hits with `spell` for `damage`: returns the damage with the attacker's proc
/// bonuses (one-hit bonuses are used up), and runs the per-hit procs.
pub fn outgoing(attacker: Id32, spell: u32, damage: u32) -> u32 {
    let mut records = Vec::new();
    let mut later = Vec::new();
    let factor = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&attacker) else { return damage };
        let now = Instant::now();
        let which = if spell == s.normal { ProcOn::NormalHit } else { ProcOn::SpecialHit };
        fire(s, attacker, &[ProcOn::Hit, which], false, &mut records, &mut later);
        s.damage.retain(|&(_, until, _)| until.is_none_or(|u| u > now));
        let bonus: f32 = s.damage.iter().map(|&(b, _, _)| b).sum::<f32>() + while_not_hit(s, "damage");
        for (_, _, nano) in s.damage.iter().filter(|(_, until, _)| until.is_none()) {
            records.push(unbuff(attacker, *nano));
        }
        s.damage.retain(|&(_, until, _)| until.is_some());
        1.0 + bonus
    };
    if !records.is_empty() {
        show(attacker, records);
    }
    finish(attacker, None, later);
    ((damage as f32) * factor).round() as u32
}

/// Extra health orb chance (percent) for `player`'s loot (`orb_chance`: Resourceful).
pub fn orb_bonus(player: Id32) -> u64 {
    PLAYERS.lock().unwrap().get(&player).map_or(0, |s| s.orb_bonus)
}

/// `attacker` defeated enemy `target` with `spell` at `at` on `playfield`.
pub fn killed(attacker: Id32, target: Id32, spell: u32, playfield: u32, at: [f32; 3]) {
    let mut records = Vec::new();
    let mut later = Vec::new();
    {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&attacker) else { return };
        let on: &[ProcOn] = if spell != s.normal { &[ProcOn::Kill, ProcOn::SpecialKill] } else { &[ProcOn::Kill] };
        fire(s, attacker, on, false, &mut records, &mut later);
    }
    if !records.is_empty() {
        show(attacker, records);
    }
    finish(attacker, Some(Event { spell, playfield, at, target }), later);
}

/// `player` is hit for `damage` (before mitigation): returns it with the defense procs (and
/// uses up one-hit shields); `channelling` = during a channelled special.
pub fn incoming(player: Id32, damage: u32, channelling: bool) -> u32 {
    let mut records = Vec::new();
    let mut later = Vec::new();
    let damage = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&player) else { return damage };
        let now = Instant::now();
        let not_hit = while_not_hit(s, "defense");
        s.last_hit_taken = Some(now);
        fire(s, player, &[ProcOn::Damaged], channelling, &mut records, &mut later);
        s.defense.retain(|&(_, _, until, _)| until.is_none_or(|u| u > now));
        let defense: f32 = s.defense.iter().map(|d| d.0).sum::<f32>() + not_hit;
        let mitigation: f32 = s.defense.iter().map(|d| d.1).sum();
        for (_, _, _, nano) in s.defense.iter().filter(|d| d.2.is_none()) {
            records.push(unbuff(player, *nano));
        }
        s.defense.retain(|d| d.2.is_some());
        ((damage as f32) * (1.0 - mitigation.min(0.9)) / (1.0 + defense)).round().max(1.0) as u32
    };
    if !records.is_empty() {
        show(player, records);
    }
    finish(player, None, later);
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
        fire(s, player, &[ProcOn::HealthBelow], false, &mut records, &mut later);
        s.procs.extend(rest);
    }
    if !records.is_empty() {
        show(player, records);
    }
    finish(player, None, later);
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

    #[test]
    fn berserker_and_brute_force() {
        let p = Id32::new(50_000, 1);
        // the Alien Avenger has Berserker among its upgrades; activate every bit
        let alien_avenger = 41_754;
        set_active(p, alien_avenger, 0xfff, 30);
        let has = PLAYERS.lock().unwrap()[&p].procs.iter().any(|a| a.key.starts_with("buff/upgrade-berserker"));
        if has {
            let normal = PLAYERS.lock().unwrap()[&p].normal;
            let hits: Vec<u32> = (0..5)
                .map(|_| {
                    cast(p, normal);
                    outgoing(p, normal, 100)
                })
                .collect();
            assert!(hits[4] >= 200, "{hits:?}");
            assert!(hits[0] < 200, "{hits:?}");
        }
        left(p);
    }

    #[test]
    fn shields_up_stacks_five_times() {
        let c = crate::gamedata::get();
        let (template, bit) = c
            .minifigures
            .values()
            .find_map(|m| m.upgrades.iter().find(|u| u.buff.starts_with("buff/upgrade-shields-up")).map(|u| (m.template, u.bit)))
            .expect("a minifigure with Shields Up");
        let p = Id32::new(50_000, 2);
        set_active(p, template, 1 << bit, 30);
        let hits: Vec<u32> = (0..8).map(|_| incoming(p, 100, true)).collect();
        // +2% defense a hit while channelling, at most 5 (10%): 100 / 1.1
        assert_eq!(hits[7], 91, "{hits:?}");
        assert_eq!(incoming(p, 100, false), 91);
        left(p);
    }
}
