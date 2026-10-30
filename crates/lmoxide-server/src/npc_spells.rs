//! Enemy abilities: the spells each enemy casts at players and what they do, from the content
//! tree (skills.rs: the NPC templates' `skills` by look, first from tools/re/extract_npc_spells.py:
//! the recorded session's enemies' spells, others matched to their monster by the spells' names;
//! the skills' client spells; projectile items).
//!
//! A cast is shown as the original server showed the recorded enemies' casts: a MagicCommand
//! on the enemy (BehaviorControlCenterIIR_c, `npc::observed_cast`), with the player as target
//! (the enemy itself for self-centred spells, TargetingMode 8). After the spell's ItemDelay (the
//! animation's strike) the server runs the server-side game actions of the spell's lists, as
//! actions.rs does for the players' abilities but aimed at players:
//!
//! - WeaponHit: damage = the enemy's power (`npc::enemy_damage`) × AbilityMod ± variance;
//! - CastNano: a nano with TimeExist (or one on the enemy itself without server actions) is a
//!   buff (`MagicBuffIIR_c`; the client runs its snares and stuns), otherwise its own list 0
//!   runs on the target and its EffectPackage plays there;
//! - CoreFilteredAreaFormula / ColumnFormula: the players in the shape get the formula's nano
//!   (`AreaFormulaIIR_t` shows it), CoreRandomPointAreaFormula: nanos at random points;
//! - CorePersistentGroundEffect / PlaceTrap: a ground zone re-applying its nano;
//! - Projectile (a client action the server carries out, as for players): the server creates the
//!   projectile dynel (projectile.rs `launch_enemy`, recorded Scallywag and Musketeer shots) and,
//!   when it arrives, runs the projectile item's actions (usually CastNano of a hit nano) on whom
//!   it hit, with `CoreProjectileIIR` HitTarget;
//! - channelled spells: OnChanneling every ChannelingInterval for ChannelingTicks ticks, then
//!   OnChannelingEnd and the command's abort (BCC action 7, as the recorded Dreadleg spin ended).
//!
//! - CastNano with an Interval: a pulse every Interval while the spell lasts and the caster fights
//!   (the champions' element auras, npc.rs `champion_aura`).
//!
//! - SpawnMonster: the content template with that `spawn_hash` (npc.rs `spawn_monster`) where the
//!   action applies, then the action's NanoTemplate cast on it: adds that fight the player and
//!   drop nothing, or invisible anchors that orbiting shots circle (ProjectileType 3,
//!   projectile.rs `launch_orbit`: the Space Merc Engineer's drones). Templates nobody matched
//!   to a hash yet spawn nothing.
//!
//! Not run: server scripts, damage redirects, buff counters. Choosing which spell to cast, ranges and cooldowns: `npc.rs` `engage`; the guesses are
//! in docs/TUNABLES.md.

use crate::instance::Pf;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_net::types::Id32;
use serde::Deserialize;
use tracing::{debug, info};

use crate::{effects, world};

struct Data {
    monsters: HashMap<u32, Vec<u32>>,
    spells: HashMap<u32, Spell>,
    items: HashMap<u32, Item>,
}

#[derive(Deserialize)]
struct Spell {
    name: String,
    #[serde(default)]
    stats: Stats,
    #[serde(default)]
    lists: HashMap<String, Vec<Action>>,
}

/// Spell stats; times in centiseconds, distances in centimetres.
#[derive(Default, Deserialize)]
struct Stats {
    #[serde(rename = "AttackRange")]
    attack_range: Option<f32>,
    #[serde(rename = "RechargeDelay")]
    recharge_delay: Option<f32>,
    #[serde(rename = "ItemDelay")]
    item_delay: Option<f32>,
    #[serde(rename = "ChannelingInterval")]
    channeling_interval: Option<f32>,
    #[serde(rename = "ChannelingTicks")]
    channeling_ticks: Option<u32>,
    #[serde(rename = "TargetingMode")]
    targeting_mode: Option<u32>,
    #[serde(rename = "TimeExist")]
    time_exist: Option<f32>,
    #[serde(rename = "AIPowerrating")]
    power: Option<f32>,
    #[serde(rename = "EffectPackage")]
    effect_package: Option<u32>,
}

#[derive(Deserialize)]
struct Action {
    #[serde(rename = "type")]
    kind: String,
    client: bool,
    #[serde(default)]
    args: HashMap<String, serde_json::Value>,
    #[serde(default)]
    criteria: Vec<[u32; 3]>,
}

impl Action {
    fn f(&self, key: &str) -> Option<f32> {
        self.args.get(key).and_then(|v| v.as_f64()).map(|v| v as f32)
    }
    fn u(&self, key: &str) -> Option<u32> {
        self.args.get(key).and_then(|v| v.as_u64()).map(|v| v as u32)
    }
    /// Sets the caster's Health to 0 (ApplyOn 1, Stat 27, no Value): the shrooms' explosions.
    fn kills_caster(&self) -> bool {
        self.kind == "Set" && self.on_caster() && self.u("Stat") == Some(crate::game::STAT_HEALTH) && self.u("Value").unwrap_or(0) == 0
    }
    fn on_caster(&self) -> bool {
        self.u("ApplyOn") == Some(1)
    }
}

#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    actions: Vec<Action>,
}

static DATA: LazyLock<Data> = LazyLock::new(|| Data {
    monsters: crate::skills::monster_spells(),
    spells: crate::skills::spells().into_iter().map(|(k, v)| (k, serde_json::from_value(v).expect("skill"))).collect(),
    items: crate::skills::projectile_items().into_iter().map(|(k, v)| (k, serde_json::from_value(v).expect("projectile item"))).collect(),
});

const ON_USE: &str = "0";
const ON_CHANNELING: &str = "33";
const CHANNEL_START: &str = "35";
const ON_CHANNELING_END: &str = "36";
const PRE_USE: &str = "39";
const TARGETING_GROUND: u32 = 7;
const TARGETING_SELF: u32 = 8;
const CRITERIA_HAS_ABILITY: u32 = 91;
const CRITERIA_BUFF_COUNTER: u32 = 166;
/// A buff without TimeExist on a character: as long as the original server showed Captain
/// Dreadleg's "spin tired" stun (MagicBuffIIR_c duration 300 cs, recorded).
const DEFAULT_BUFF: Duration = Duration::from_secs(3);
/// Ground zones (Arg1 the pulse interval in centiseconds, Arg2 the number of pulses, as
/// actions.rs): the interval when Arg1 is missing, the shortest visual.
const GROUND_DEFAULT_PULSE_CS: f32 = 100.0;
const GROUND_MIN_VISUAL: Duration = Duration::from_secs(1);
/// Range of a projectile without AttackRange.
const DEFAULT_PROJECTILE_RANGE: f32 = 15.0;
const MAX_DEPTH: u32 = 10;
/// Crowd control actions (what NanoResistance strain 800 resists).
const CROWD_CONTROL: &[&str] = &["AddSnare", "Stun", "Knockaway", "AreaKnockaway", "Root", "Fear"];
/// Traps (PlaceTrap) spring when a player comes this close (the islander scout's trap area
/// formula's Radius), checked this often.
const TRAP_RADIUS: f32 = 2.0;
const TRAP_POLL: Duration = Duration::from_millis(250);

fn spell(id: u32) -> Option<&'static Spell> {
    DATA.spells.get(&id)
}

/// The spells enemies of monsterdata `monster_data` cast, if known.
pub fn spells_of(monster_data: u32) -> Option<&'static [u32]> {
    DATA.monsters.get(&monster_data).map(Vec::as_slice)
}

/// Whether this module runs `spell` (otherwise it's a plain hit, npc.rs `attack`).
pub fn known(id: u32) -> bool {
    spell(id).is_some()
}

/// The spell's attack range in metres (0 when it has none).
pub fn range(id: u32) -> f32 {
    let Some(s) = spell(id) else { return 0.0 };
    let range = s.stats.attack_range.map_or(0.0, |r| r / 100.0);
    // a spell that blows its caster up (the shrooms' explosions) is cast where its blast
    // reaches (the space Boom Shroom's says 18 m, its blast 2 m)
    match s.lists.get(ON_USE).filter(|l| l.iter().any(Action::kills_caster)) {
        Some(list) => list.iter().filter(|a| a.kind == "CoreFilteredAreaFormula").filter_map(|a| a.f("Radius")).fold(range, f32::min),
        None => range,
    }
}

/// The spell's AI power rating (the AI prefers higher ones when they're ready).
pub fn power(id: u32) -> f32 {
    spell(id).and_then(|s| s.stats.power).unwrap_or(0.0)
}

/// How long after a cast the spell can be cast again: its RechargeDelay, or the recast time
/// its own SetSpellRecastTime sets, whichever is longer.
pub fn cooldown(id: u32) -> Duration {
    let Some(s) = spell(id) else { return Duration::ZERO };
    let recast = s
        .lists
        .values()
        .flatten()
        .filter(|a| a.kind == "SetSpellRecastTime" && a.u("NanoTemplate").is_none_or(|n| n == id))
        .filter_map(|a| a.f("Value"))
        .fold(0.0f32, f32::max);
    cs(s.stats.recharge_delay.unwrap_or(0.0).max(recast))
}

/// How long the caster stands still for a cast: until the strike (ItemDelay), and for
/// channels it doesn't move with (not self-centred, e.g. a blaster volley, a breath) the
/// channel's length.
pub fn busy(id: u32) -> Duration {
    let Some(s) = spell(id) else { return Duration::ZERO };
    let strike = cs(s.stats.item_delay.unwrap_or(0.0));
    match channel(s) {
        Some((interval, ticks)) if !self_centred(s) => strike + interval * ticks,
        _ => strike,
    }
}

/// When the spell strikes after its cast starts (its ItemDelay).
pub fn strike_delay(id: u32) -> Duration {
    spell(id).map_or(Duration::ZERO, |s| cs(s.stats.item_delay.unwrap_or(0.0)))
}

/// The projectile the spell shoots (its first Projectile action on use): the item and its
/// speed in m/s. NPC-against-NPC fights (npc.rs `strike`) show it flying.
pub fn projectile_of(id: u32) -> Option<(u32, f32)> {
    let s = spell(id)?;
    let a = [PRE_USE, ON_USE].iter().filter_map(|e| s.lists.get(*e)).flatten().find(|a| a.kind == "Projectile")?;
    Some((a.u("ItemInstance")?, a.f("Speed").unwrap_or(1000.0) / 100.0))
}

fn channel(s: &Spell) -> Option<(Duration, u32)> {
    let ticks = s.stats.channeling_ticks.filter(|t| *t > 0)?;
    Some((cs(s.stats.channeling_interval.unwrap_or(50.0)), ticks))
}

fn cs(v: f32) -> Duration {
    Duration::from_millis((v.max(0.0) * 10.0) as u64)
}

/// A cast in progress.
struct Run {
    caster: Id32,
    playfield: Pf,
    /// The caster's power (`npc::enemy_damage`).
    power: f32,
    spell: u32,
    player: Id32,
    /// The player's position when the cast started (ground-targeted spells aim there).
    aimed: [f32; 3],
    /// Where the caster faced when it cast (sectors, columns, straight shots).
    facing: [f32; 2],
}

#[derive(Clone, Copy, Debug)]
struct Target {
    id: Option<Id32>,
    pos: [f32; 3],
}

/// Enemy `caster` (on `playfield`, with `power`, `npc::enemy_damage`) casts `id` at `player` (at `at`).
pub fn cast(caster: Id32, playfield: Pf, power: f32, id: u32, player: Id32, at: [f32; 3]) {
    let Some(s) = spell(id) else { return };
    let here = crate::npc::position(caster).unwrap_or(at);
    let self_cast = self_centred(s);
    let commanded = if self_cast { caster } else { player };
    let (command, records) = crate::npc::observed_cast(caster, commanded, id);
    world::send_playfield(caster, false, world::Out::Records(records.to_vec()));
    let (dx, dz) = (at[0] - here[0], at[2] - here[2]);
    let d = dx.hypot(dz);
    let facing = if d > 0.01 { [dx / d, dz / d] } else { [0.0, 1.0] };
    let run = Arc::new(Run { caster, playfield, power, spell: id, player, aimed: at, facing });
    info!(?caster, spell = id, name = %s.name, ?player, "enemy cast");
    run_list(&run, id, PRE_USE, root_target(&run), 0);
    let delay = cs(s.stats.item_delay.unwrap_or(0.0));
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        if !alive(&run) {
            return;
        }
        run_list(&run, id, ON_USE, root_target(&run), 0);
        let Some((interval, ticks)) = channel(s) else { return };
        run_list(&run, id, CHANNEL_START, root_target(&run), 0);
        for _ in 0..ticks {
            tokio::time::sleep(interval).await;
            if !alive(&run) {
                return;
            }
            run_list(&run, id, ON_CHANNELING, root_target(&run), 0);
        }
        run_list(&run, id, ON_CHANNELING_END, root_target(&run), 0);
        show(&run, vec![crate::npc::command_record(caster, commanded, id, crate::npc::BCC_ABORT, command)]);
    });
}

fn alive(run: &Run) -> bool {
    crate::npc::health_of(run.caster).is_some()
}

fn here(run: &Run) -> [f32; 3] {
    crate::npc::position(run.caster).unwrap_or(run.aimed)
}

/// The spell's own target: the caster for self-centred spells, the player's position at the
/// cast for ground-targeted ones, else the player.
fn root_target(run: &Run) -> Target {
    let Some(s) = spell(run.spell) else { return Target { id: Some(run.caster), pos: here(run) } };
    if self_centred(s) {
        Target { id: Some(run.caster), pos: here(run) }
    } else if s.stats.targeting_mode == Some(TARGETING_GROUND) {
        Target { id: None, pos: run.aimed }
    } else {
        Target { id: Some(run.player), pos: world::position_of(run.player).unwrap_or(run.aimed) }
    }
}

/// Spells cast on the caster itself: TargetingMode 8, and channels without a TargetingMode
/// (the original server cast Captain Dreadleg's spin, which has none, on himself).
fn self_centred(s: &Spell) -> bool {
    match s.stats.targeting_mode {
        Some(mode) => mode == TARGETING_SELF,
        None => channel(s).is_some(),
    }
}

/// Sends `records` to the caster's playfield (through the player once the caster is gone).
fn show(run: &Run, records: Vec<Bytes>) {
    if records.is_empty() {
        return;
    }
    let from = if crate::npc::position(run.caster).is_some() { run.caster } else { run.player };
    world::send_playfield(from, true, world::Out::Records(records));
}

/// The players on the caster's playfield where they are now (`world::live_position`) `inside`.
fn players_where(run: &Run, inside: impl Fn([f32; 3]) -> bool) -> Vec<(Id32, [f32; 3])> {
    world::live_positions(run.playfield).into_iter().filter(|(id, p)| inside(*p) && crate::npc::is_player(*id)).collect()
}

fn criteria_met(a: &Action) -> bool {
    a.criteria.iter().all(|&[_, y, kind]| match kind {
        // enemies have no upgrades and run no buff counters
        CRITERIA_HAS_ABILITY => false,
        CRITERIA_BUFF_COUNTER => y == 0,
        _ => true,
    })
}

fn run_list(run: &Arc<Run>, id: u32, event: &str, target: Target, depth: u32) {
    let Some(list) = spell(id).and_then(|s| s.lists.get(event)) else { return };
    // (one blast: its formulas hit each player once with a nano, actions::blast)
    crate::actions::blast(|| {
        for a in list.iter().filter(|a| criteria_met(a)) {
            act(run, id, a, target, depth);
        }
    })
}

fn act(run: &Arc<Run>, id: u32, a: &Action, target: Target, depth: u32) {
    let caster = run.caster;
    let me = Target { id: Some(caster), pos: here(run) };
    let who = if a.on_caster() { me } else { target };
    match a.kind.as_str() {
        // the client-side actions the server carries out for enemies
        "Projectile" => projectile(run, id, a, target, depth),
        // (after its blast, earlier in the list) it dies: no loot, only a kill drops some
        _ if a.kills_caster() => crate::npc::self_destruct(caster),
        _ if a.client => {}
        "WeaponHit" => {
            if let Some(player) = who.id.filter(|p| *p != caster && crate::npc::is_player(*p)) {
                let damage = crate::npc::enemy_hit(run.power, a.f("AbilityMod").unwrap_or(1.0));
                crate::npc::hurt_player(caster, player, run.spell, damage);
            }
        }
        "CastNano" => {
            if let Some(nano) = a.u("Value") {
                match a.u("Interval").filter(|i| *i > 0) {
                    // a pulse every Interval cs while the spell lasts (its TimeExist) and the
                    // caster fights: the champions' auras
                    Some(interval) => pulse(run, id, nano, who, cs(interval as f32), depth),
                    None => cast_nano(run, nano, who, depth + 1, true),
                }
            }
        }
        "RemoveBuff" => {
            if let (Some(buff), Some(on)) = (a.u("TemplateID"), who.id) {
                effects::forget_buff(on, buff);
                show(run, vec![effects::buff_remove(on, buff, caster)]);
            }
        }
        "CoreFilteredAreaFormula" => {
            let center = who.pos;
            let radius = a.f("Radius").unwrap_or(2.0);
            let min = a.f("Min").unwrap_or(0.0);
            let angle = a.f("Angle").unwrap_or(360.0);
            let from = a.f("AngleStart").unwrap_or(-angle / 2.0);
            let mut hits = players_where(run, |p| {
                let d = distance(p, center);
                d <= radius && d >= min && (angle >= 360.0 || in_sector(center, run.facing, p, from, angle))
            });
            hits.sort_by(|(_, x), (_, y)| distance(*x, center).total_cmp(&distance(*y, center)));
            if let Some(nano) = a.u("NanoTemplate") {
                crate::actions::first_hits(&mut hits, nano);
            }
            if let Some(max) = a.u("Max1").filter(|m| *m > 0) {
                hits.truncate(max as usize);
            }
            if let Some(nano) = a.u("NanoTemplate") {
                let ids: Vec<Id32> = hits.iter().map(|(id, _)| *id).collect();
                show(run, vec![crate::actions::area_formula_record(caster, &ids, nano, center)]);
                for (p, pos) in hits {
                    cast_nano(run, nano, Target { id: Some(p), pos }, depth + 1, false);
                }
            }
        }
        "ColumnFormula" => {
            // from the caster, or from a ground point (where Captain Dreadleg's bomb went off);
            // Value1 turns it by quarters (the bomb's four arms), as actions.rs
            let from = if target.id.is_none() && !a.on_caster() { target.pos } else { me.pos };
            let dir = rotate(run.facing, 90.0 * a.u("Value1").unwrap_or(0) as f32);
            let length = a.f("Distance").unwrap_or(5.0);
            let width = a.f("FloatValue1").unwrap_or(1.0);
            let mut hits = players_where(run, |p| on_line(from, dir, p, length, width).is_some());
            if let Some(nano) = a.u("NanoTemplate") {
                crate::actions::first_hits(&mut hits, nano);
                let ids: Vec<Id32> = hits.iter().map(|(id, _)| *id).collect();
                let mid = [from[0] + dir[0] * length / 2.0, from[1], from[2] + dir[1] * length / 2.0];
                show(run, vec![crate::actions::area_formula_record(caster, &ids, nano, mid)]);
                for (p, pos) in hits {
                    cast_nano(run, nano, Target { id: Some(p), pos }, depth + 1, false);
                }
            }
        }
        "CoreRandomPointAreaFormula" => {
            let radius = a.f("FloatRadius").unwrap_or(3.0);
            if let Some(nano) = a.u("NanoTemplate") {
                for _ in 0..a.u("Max1").unwrap_or(1) {
                    let (angle, r) = (random_unit() * std::f32::consts::TAU, radius * random_unit().sqrt());
                    let pos = [who.pos[0] + r * angle.sin(), who.pos[1], who.pos[2] + r * angle.cos()];
                    cast_nano(run, nano, Target { id: None, pos }, depth + 1, true);
                }
            }
        }
        "CorePersistentGroundEffect" => {
            let pulse = cs(a.f("Arg1").unwrap_or(GROUND_DEFAULT_PULSE_CS).max(1.0));
            let pulses = a.u("Arg2").unwrap_or(1).max(1);
            let delay = cs(a.f("DelayTime").unwrap_or(0.0));
            let lifetime = (delay + pulse * pulses).max(GROUND_MIN_VISUAL);
            if let Some(package) = a.u("EffectPackage") {
                let instance = effects::new_instance();
                show(run, vec![effects::ground_effect(caster, package, who.pos, id, instance)]);
                let run = run.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(lifetime).await;
                    show(&run, vec![effects::stop_effect_package(run.caster, package, id, instance)]);
                });
            }
            // three nanos (the action is server-only, so read from the data): TemplateID once when
            // the zone starts (the "impact aoe" ones: Sheriff, Tribal Woman, Clumsy Guy; the
            // Banana), NanoTemplate every pulse (often the same nano: then only the pulses), and
            // TemplateID2 once when it ends (Captain Dreadleg's bomb aoe 238199 after its 0.5 s
            // fuse; the champions' brambles after their 3 s rock marker)
            let pulsed = a.u("NanoTemplate");
            let first = a.u("TemplateID").filter(|n| Some(*n) != pulsed);
            let last = a.u("TemplateID2").map(|n| (n, delay + pulse * pulses));
            if first.is_some() || last.is_some() {
                ground_once(run, first.map(|n| (n, delay)).into_iter().chain(last).collect(), who.pos, depth);
            }
            if let Some(nano) = pulsed {
                ground_zone(run, nano, who.pos, delay, pulse, pulse * pulses, depth);
            }
        }
        "PlaceTrap" => {
            if let Some(nano) = a.u("NanoTemplate") {
                let lifetime = Duration::from_secs(a.u("TimedLength").unwrap_or(3) as u64);
                trap(run, nano, who.pos, lifetime, depth);
            }
        }
        "SpawnMonster" => {
            // the original server's monster (StringHash: content npcs `spawn_hash`) where the
            // spell aims, then its NanoTemplate cast on it: an add that joins the fight against
            // the player (dropping nothing), or an anchor shots circle (the Space Merc
            // Engineer's drones, which stay where he cast them)
            let Some(hash) = a.args.get("StringHash").and_then(|v| v.as_str()) else { return };
            match crate::npc::spawn_monster(caster, run.playfield, hash, who.pos, Some(run.player)) {
                Some(spawned) => {
                    if let Some(nano) = a.u("NanoTemplate") {
                        cast_nano(run, nano, Target { id: Some(spawned), pos: who.pos }, depth + 1, false);
                    }
                }
                None => debug!(spell = id, hash, "enemy spawn monster: no template with this spawn_hash"),
            }
        }
        other => debug!(spell = id, action = other, "enemy spell action not run"),
    }
}

/// Casts nano `nano` on `target`: a buff is shown as one, a hit nano plays its effect there
/// (`own_effect`, not for an area formula's: its record shows it); then its list 0 runs there.
fn cast_nano(run: &Arc<Run>, nano: u32, target: Target, depth: u32, own_effect: bool) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(n) = spell(nano) else { return };
    let caster = run.caster;
    let on_caster = target.id == Some(caster);
    let server_actions = n.lists.get(ON_USE).is_some_and(|l| l.iter().any(|a| !a.client));
    // (a server-made nano's TimeExist is only how long what it sends lasts: the clients don't know it)
    let is_buff = target.id.is_some() && !crate::skills::is_server_made(nano) && (n.stats.time_exist.is_some() || (on_caster && !server_actions));
    // a player immune to crowd control (True Grit, Stalwart) doesn't get slows, stuns and
    // knockbacks (the client would run them)
    let crowd_control = n.lists.values().flatten().any(|a| CROWD_CONTROL.contains(&a.kind.as_str()));
    if is_buff && crowd_control && target.id.is_some_and(|p| p != caster && crate::npc::is_player(p) && crate::procs::cc_immune(p)) {
        debug!(nano, "crowd control resisted");
        return;
    }
    match target.id {
        Some(on) if is_buff => {
            let duration = n.stats.time_exist.map_or(DEFAULT_BUFF, cs);
            show(run, effects::show_buff(on, nano, caster, Some(duration)));
            // a stun on the caster (e.g. tired after a spin) keeps it from moving
            if on_caster && n.lists.values().flatten().any(|a| a.kind == "Stun") {
                crate::npc::set_busy(caster, duration);
            }
        }
        Some(on) if own_effect => {
            if let Some(package) = n.stats.effect_package {
                show(run, vec![effects::effect_package(on, package)]);
            }
        }
        // a nano at a bare ground point plays its effect there (Captain Dreadleg's bomb aoe:
        // the explosion 239180)
        None if own_effect => {
            if let Some(package) = n.stats.effect_package {
                show(run, vec![effects::effect_package_at(caster, package, target.pos)]);
            }
        }
        _ => {}
    }
    run_list(run, nano, ON_USE, target, depth);
}

/// Casts `nano` on `who` every `every` while spell `id` lasts (its TimeExist) and the caster
/// fights (`npc::fighting`); `who` is followed if it is a character.
fn pulse(run: &Arc<Run>, id: u32, nano: u32, who: Target, every: Duration, depth: u32) {
    let lasts = spell(id).and_then(|s| s.stats.time_exist).map_or(DEFAULT_BUFF, cs);
    let run = run.clone();
    let follow = who.id;
    tokio::spawn(async move {
        let until = std::time::Instant::now() + lasts;
        let mut at = who.pos;
        loop {
            tokio::time::sleep(every).await;
            if std::time::Instant::now() >= until || !alive(&run) || !crate::npc::fighting(run.caster) {
                return;
            }
            if let Some(p) = follow.and_then(|f| if f == run.caster { crate::npc::position(f) } else { world::position_of(f) }) {
                at = p;
            }
            cast_nano(&run, nano, Target { id: follow, pos: at }, depth + 1, true);
        }
    });
}

/// A ground zone at `at`: after `delay`, re-applies `nano` there every `pulse` for `lifetime`.
fn ground_zone(run: &Arc<Run>, nano: u32, at: [f32; 3], delay: Duration, pulse: Duration, lifetime: Duration, depth: u32) {
    let run = run.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let until = std::time::Instant::now() + lifetime;
        while std::time::Instant::now() < until {
            cast_nano(&run, nano, Target { id: None, pos: at }, depth + 1, true);
            tokio::time::sleep(pulse).await;
        }
    });
}

/// A ground zone's one-off nanos at `at`: each (nano, after) cast once that long from now.
fn ground_once(run: &Arc<Run>, nanos: Vec<(u32, Duration)>, at: [f32; 3], depth: u32) {
    for (nano, after) in nanos {
        let run = run.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            cast_nano(&run, nano, Target { id: None, pos: at }, depth + 1, true);
        });
    }
}

/// A trap at `at` (PlaceTrap): springs once, when a player comes within TRAP_RADIUS during
/// `lifetime`, casting `nano` there.
fn trap(run: &Arc<Run>, nano: u32, at: [f32; 3], lifetime: Duration, depth: u32) {
    let run = run.clone();
    tokio::spawn(async move {
        let until = std::time::Instant::now() + lifetime;
        while std::time::Instant::now() < until {
            if !players_where(&run, |p| distance(p, at) <= TRAP_RADIUS).is_empty() {
                cast_nano(&run, nano, Target { id: None, pos: at }, depth + 1, true);
                return;
            }
            tokio::time::sleep(TRAP_POLL).await;
        }
    });
}

/// A Projectile action: the server shoots the projectile item. Straight shots fly at where the
/// player is when they leave (or along a line: shots from the caster's facing or turned by RotY,
/// e.g. a fan of shots) for the spell's range, and hit the first player they touch
/// (`projectile::launch_at_players`): a player who moves out of their line escapes them. Lobbed
/// or ground-targeted shots act where they land.
fn projectile(run: &Arc<Run>, id: u32, a: &Action, target: Target, depth: u32) {
    let Some(item) = a.u("ItemInstance") else { return };
    let caster = run.caster;
    let from = here(run);
    let speed = a.f("Speed").unwrap_or(1000.0) / 100.0;
    let move_type = a.u("ProjectileType").unwrap_or(1);
    let attractor = a.u("AttractorID").unwrap_or(0);
    let range = a.f("AttackRange").or(spell(id).and_then(|s| s.stats.attack_range)).map_or(DEFAULT_PROJECTILE_RANGE, |r| r / 100.0);
    let turn = a.f("RotY").unwrap_or(0.0);
    let aimed_at = target.id.filter(|t| *t != caster && crate::npc::is_player(*t));
    let on_hit = || -> crate::projectile::OnHit {
        let run = run.clone();
        Arc::new(move |hit, pos| impact(&run, item, Target { id: hit, pos }, depth))
    };
    if move_type == crate::projectile::ORBITING {
        // it circles the caster or whom it is cast on (a spawned anchor), and bursts on the
        // first player it touches
        let centre = if a.on_caster() { caster } else { target.id.unwrap_or(caster) };
        let lasts = spell(id).and_then(|s| s.stats.time_exist).map_or(crate::projectile::ORBIT_LIFETIME, cs);
        let circle = crate::projectile::Circle {
            item,
            speed,
            centre,
            angle: turn,
            offset: ["DisplaceMentX", "DisplaceMentY", "DisplaceMentZ"].map(|k| a.f(k).unwrap_or(0.0) / 100.0),
            lifetime: crate::npc::spawned_time_left(centre).map_or(lasts, |left| lasts.min(left)),
            hits_players: true,
        };
        crate::projectile::launch_orbit(caster, run.playfield, id, &circle, on_hit());
        return;
    }
    if a.on_caster() || turn != 0.0 || (aimed_at.is_none() && target.id.is_some()) {
        let base = if a.on_caster() || target.id == Some(caster) {
            run.facing
        } else {
            let (dx, dz) = (target.pos[0] - from[0], target.pos[2] - from[2]);
            let d = dx.hypot(dz);
            if d > 0.01 { [dx / d, dz / d] } else { run.facing }
        };
        let (s, c) = turn.to_radians().sin_cos();
        let dir = [base[0] * c + base[1] * s, -base[0] * s + base[1] * c];
        let to = [from[0] + dir[0] * range, from[1], from[2] + dir[1] * range];
        crate::projectile::launch_at_players(caster, run.playfield, id, item, speed, attractor, from, to, range, None, on_hit());
    } else if let Some(player) = aimed_at.filter(|_| move_type == 1) {
        let at = world::live_position(player).unwrap_or(target.pos);
        crate::projectile::launch_at_players(caster, run.playfield, id, item, speed, attractor, from, at, range, Some(player), on_hit());
    } else {
        let to = target.pos;
        let (shot, flight) = crate::projectile::launch_enemy(caster, run.playfield, id, item, move_type, speed, attractor, from, to, None);
        let run = run.clone();
        tokio::spawn(async move {
            tokio::time::sleep(flight).await;
            crate::projectile::enemy_impact(run.caster, shot, None, Some(to));
            impact(&run, item, Target { id: None, pos: to }, depth);
        });
    }
}

/// A projectile item's actions where it arrived.
fn impact(run: &Arc<Run>, item: u32, target: Target, depth: u32) {
    let Some(i) = DATA.items.get(&item) else { return };
    for a in i.actions.iter().filter(|a| !a.client) {
        act(run, run.spell, a, target, depth + 1);
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// How far along the line from `from` in direction `dir` the point `p` is, if it lies within
/// `width` of it and `length` of `from`.
fn on_line(from: [f32; 3], dir: [f32; 2], p: [f32; 3], length: f32, width: f32) -> Option<f32> {
    let (dx, dz) = (p[0] - from[0], p[2] - from[2]);
    let along = dx * dir[0] + dz * dir[1];
    let across = (dx * dir[1] - dz * dir[0]).abs();
    ((0.0..=length).contains(&along) && across <= width).then_some(along)
}

/// `dir` turned by `degrees` (positive: clockwise seen from above, as RotY).
fn rotate(dir: [f32; 2], degrees: f32) -> [f32; 2] {
    let (s, c) = degrees.to_radians().sin_cos();
    [dir[0] * c + dir[1] * s, -dir[0] * s + dir[1] * c]
}

/// Whether `p` lies in the sector from `start` to `start + angle` degrees around `facing`.
fn in_sector(center: [f32; 3], facing: [f32; 2], p: [f32; 3], start: f32, angle: f32) -> bool {
    let (dx, dz) = (p[0] - center[0], p[2] - center[2]);
    if dx.hypot(dz) < 0.01 {
        return true;
    }
    let rel = (dx.atan2(dz) - facing[0].atan2(facing[1])).to_degrees();
    let rel = (rel + 540.0).rem_euclid(360.0) - 180.0;
    rel >= start && rel <= start + angle
}

fn random_unit() -> f32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let x = nanos.wrapping_add(SEQ.fetch_add(0x9e37_79b9, Ordering::Relaxed));
    (x.wrapping_mul(2_654_435_761) % 10_000) as f32 / 10_000.0
}

/// A random pick weight in [0, 1) (spell choice ties).
pub fn jitter() -> f32 {
    random_unit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_enemies_have_their_spells() {
        // Scallywags (skeleton_pirate_1): XX_Shot 197370 and XX_Slash 197365, as recorded
        assert_eq!(spells_of(207_001), Some(&[197_370, 197_365][..]));
        // Captain Dreadleg: slash and spin
        assert_eq!(spells_of(634_824), Some(&[232_292, 238_132][..]));
        assert!(range(197_370) > 5.0 && range(197_365) < 1.0);
        // the shot's own recast time (SetSpellRecastTime 500) is longer than its RechargeDelay
        assert_eq!(cooldown(197_370), Duration::from_secs(5));
        // the spin: channelled, self-centred (the boss keeps moving), 20 s recast
        let spin = spell(238_132).unwrap();
        assert_eq!(channel(spin), Some((Duration::from_millis(500), 20)));
        assert_eq!(busy(238_132), Duration::from_millis(10));
        assert_eq!(cooldown(238_132), Duration::from_secs(20));
        // every projectile item the spells shoot is there
        for s in DATA.spells.values() {
            for a in s.lists.values().flatten().filter(|a| a.kind == "Projectile") {
                assert!(DATA.items.contains_key(&a.u("ItemInstance").unwrap()), "{}", s.name);
            }
        }
    }

    #[test]
    fn ability_test_enemies_have_spells() {
        // `!test abilities` / `!test blackheart` spawn copies of world content enemies by name
        let scenarios: serde_json::Value = serde_json::from_str(include_str!("../../../data/test_scenarios.json")).unwrap();
        let table = crate::spawns::table();
        let monster = |name: &str| {
            table.values().flatten().filter(|d| d.kind == crate::npc::Kind::Enemy && d.name == name).max_by_key(|d| d.health).map(|d| u64::from(d.monster_data))
        };
        for s in scenarios["scenarios"].as_array().unwrap().iter().filter(|s| ["abilities", "blackheart", "bomb"].contains(&s["label"].as_str().unwrap())) {
            for spawn in s["spawn"].as_array().unwrap() {
                let name = spawn["name"].as_str().unwrap();
                let md = monster(name).unwrap_or_else(|| panic!("no enemy named {name}"));
                let spells = spells_of(md as u32).unwrap_or_default();
                assert!(!spells.is_empty() && spells.iter().all(|s| known(*s)), "{name}");
                // `spells`: the only ones the test spawn casts are its own
                for only in spawn.get("spells").and_then(|v| v.as_array()).into_iter().flatten() {
                    assert!(spells.contains(&(only.as_u64().unwrap() as u32)), "{name} {only}");
                }
            }
        }
    }

    /// Captain Dreadleg's bomb: the lobbed Bomb casts the ground effect, whose TemplateID2 (the
    /// explosion, with its effect package) and the explosion's damage nano are there.
    #[test]
    fn dreadleg_bomb_explodes() {
        let item = projectile_of(238_202).map(|(item, _)| item);
        assert_eq!(item, Some(238_201));
        let lands = DATA.items[&238_201].actions.iter().find(|a| a.kind == "CastNano").and_then(|a| a.u("Value"));
        assert_eq!(lands, Some(238_200));
        let pge = spell(238_200).unwrap().lists[ON_USE].iter().find(|a| a.kind == "CorePersistentGroundEffect").unwrap();
        assert_eq!(pge.u("TemplateID2"), Some(238_199));
        let aoe = spell(238_199).expect("bomb aoe extracted");
        assert_eq!(aoe.stats.effect_package, Some(239_180));
        assert!(aoe.lists[ON_USE].iter().all(|a| a.u("NanoTemplate") == Some(238_198)));
        assert!(spell(238_198).is_some_and(|s| s.lists[ON_USE].iter().any(|a| a.kind == "WeaponHit")));
    }

    /// The Pirate Grenadier's XX_Bomb lobs the pirate_grenadier_bomb (item 190832), whose landing
    /// leaves its fuse (190829) that explodes in a cross (191106) of damage 191107.
    #[test]
    fn grenadier_bomb_explodes() {
        assert!(spells_of(192_821).is_some_and(|s| s.contains(&190_796) && !s.contains(&238_116)));
        assert_eq!(projectile_of(190_796).map(|(item, _)| item), Some(190_832));
        let lands = DATA.items[&190_832].actions.iter().find(|a| a.kind == "CastNano").and_then(|a| a.u("Value"));
        assert_eq!(lands, Some(190_829));
        let pge = spell(190_829).unwrap().lists[ON_USE].iter().find(|a| a.kind == "CorePersistentGroundEffect").unwrap();
        assert_eq!(pge.u("NanoTemplate"), Some(191_106));
        assert!(spell(191_106).unwrap().lists[ON_USE].iter().all(|a| a.u("NanoTemplate") == Some(191_107)));
        assert!(spell(191_107).is_some_and(|s| s.lists[ON_USE].iter().any(|a| a.kind == "WeaponHit")));
    }

    #[test]
    fn lines_and_sectors() {
        let o = [0.0, 0.0, 0.0];
        assert_eq!(on_line(o, [0.0, 1.0], [0.5, 0.0, 4.0], 10.0, 1.0), Some(4.0));
        assert_eq!(on_line(o, [0.0, 1.0], [2.0, 0.0, 4.0], 10.0, 1.0), None);
        assert_eq!(on_line(o, [0.0, 1.0], [0.0, 0.0, -1.0], 10.0, 1.0), None);
        assert!(in_sector(o, [0.0, 1.0], [0.0, 0.0, 2.0], -30.0, 60.0));
        assert!(!in_sector(o, [0.0, 1.0], [0.0, 0.0, -2.0], -30.0, 60.0));
        // the bomb's cross: Value1 quarter turns of the facing
        let east = rotate([0.0, 1.0], 90.0);
        assert_eq!(on_line(o, east, [4.0, 0.0, 0.5], 10.0, 1.0).map(f32::round), Some(4.0));
    }
}
