//! Channelled abilities (held specials: the Barbarian's Spin Slash, the Cyclops' Laser Eye, the
//! Samurai Warrior's stance, ...). The server runs the server-only actions of the spell's game
//! action lists (data/spell_actions.json, tools/re/extract_spell_actions.py) while the client runs
//! the client-side ones of its own spell itself:
//!
//! - press: PreUse (list 39), OnUse (0) and the channel start (35);
//! - every ChannelingInterval, ChannelingTicks times: OnChanneling (33); the client counts the
//!   same ticks (`MagicCommand_c::OnRun` casts list 0x21 locally);
//! - RechargeDelay after the press: OnRechargeDelay (1);
//! - release (BCC action 20 from the client, `MagicCommand_c::FinishChannel`) or the last tick:
//!   OnChannelingEnd (36). After the last tick the server ends the client's channel with the
//!   same record (action 20), as for charges.
//!
//! Actions: CastNano (a nano with TimeExist, or one cast on the caster with no server actions,
//! is a buff shown with `MagicBuffIIR_c`; the nano's own list 0 then runs on whom it was cast),
//! area/column formulas (each enemy in the shape gets the formula's nano), WeaponHit (damage:
//! the caster's power × AbilityMod, combat.rs), AddBuffCounter/RemoveBuff, CorePersistentGroundEffect
//! and PlaceTrap (a ground zone re-applying its nano), CoreRandomPointAreaFormula and ColumnDash.
//! Crowd control (snares, knockbacks, stuns), diminishing returns and resistances are only shown
//! as the buffs' visuals. Timings the client data doesn't give are in docs/TUNABLES.md.

use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use lmoxide_net::types::Id32;
use serde::Deserialize;
use tracing::{debug, info};

use crate::{effects, world};

#[derive(Debug, Deserialize)]
struct Spell {
    name: String,
    /// Set on the minifigure abilities themselves (the rest are nanos they cast).
    #[serde(default)]
    minifigure: Option<String>,
    #[serde(default)]
    stats: Stats,
    #[serde(default)]
    lists: HashMap<String, Vec<Action>>,
}

/// Spell stats; times in centiseconds.
#[derive(Debug, Default, Deserialize)]
struct Stats {
    #[serde(rename = "TimeExist")]
    time_exist: Option<u32>,
    #[serde(rename = "ChannelingInterval")]
    channeling_interval: Option<u32>,
    #[serde(rename = "ChannelingTicks")]
    channeling_ticks: Option<u32>,
    #[serde(rename = "RechargeDelay")]
    recharge_delay: Option<u32>,
    #[serde(rename = "TargetingMode")]
    targeting_mode: Option<u32>,
    /// Time from the press to the hit (the client's attack animation).
    #[serde(rename = "ItemDelay")]
    item_delay: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Action {
    #[serde(rename = "type")]
    kind: String,
    client: bool,
    #[serde(default)]
    args: HashMap<String, serde_json::Value>,
    /// (a, b, kind): 91 = has upgrade ability b, 166 = buff a's counter is b; 19 = always.
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
    /// ApplyOn 1 = the caster, otherwise the current target.
    fn on_caster(&self) -> bool {
        self.u("ApplyOn") == Some(APPLY_ON_CASTER)
    }
}

const APPLY_ON_CASTER: u32 = 1;
const CRITERIA_HAS_ABILITY: u32 = 91;
const CRITERIA_BUFF_COUNTER: u32 = 166;
/// TargetingMode of spells cast on oneself (their "target" is the caster).
const TARGETING_SELF: u32 = 8;
/// Event lists (GameData::GameActionList_e).
const ON_USE: &str = "0";
const ON_RECHARGE_DELAY: &str = "1";
const ON_CHANNELING: &str = "33";
const CHANNEL_START: &str = "35";
const ON_CHANNELING_END: &str = "36";
const PRE_USE: &str = "39";
/// MitigationOverrideFlat: a buff's damage mitigation.
const STAT_MITIGATION_FLAT: u32 = 3_000_012;
/// The Samurai Warrior's stance script (server Lua, LuaScriptID 3825): "When you are hit or you
/// release, you will perform an area attack" (the spell's description), so being hit ends the
/// channel.
const ENDS_WHEN_HIT: &[u32] = &[597_608];
/// Ground zones (CorePersistentGroundEffect): lifetime when Arg1 (assumed tenths of a second)
/// is missing, and how often they re-apply their nano (assumed).
const GROUND_DEFAULT_LIFETIME: Duration = Duration::from_secs(3);
const GROUND_PULSE: Duration = Duration::from_secs(1);
/// Nested nanos deeper than this aren't followed (the data has at most 3 levels).
const MAX_DEPTH: u32 = 5;
/// How long a combo buff without TimeExist stays (the original server removed them ~1 s after
/// the press: recorded Gladius Slash).
const COMBO_WINDOW: Duration = Duration::from_millis(1000);
const POLL: Duration = Duration::from_millis(50);

static SPELLS: LazyLock<HashMap<u32, Spell>> = LazyLock::new(|| {
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/spell_actions.json")).expect("spell_actions.json");
    raw.into_iter()
        .filter_map(|(k, v)| Some((k.parse().ok()?, serde_json::from_value(v).expect("spell_actions.json entry"))))
        .collect()
});

/// Whether `spell` is a minifigure ability (or combo step) whose actions this module runs on a
/// plain press (`cast`): it has server-side PreUse or OnUse actions.
/// Only those whose actions reach a WeaponHit: the others' damage came from actions the server
/// doesn't run, so they keep the plain hit in game.rs.
pub fn is_ability(spell: u32) -> bool {
    SPELLS.get(&spell).is_some_and(|s| {
        s.minifigure.is_some()
            && [PRE_USE, ON_USE].iter().any(|l| s.lists.get(*l).is_some_and(|l| l.iter().any(|a| !a.client)))
            && reaches_weapon_hit(spell, &mut Vec::new())
    })
}

/// Whether `spell`'s server actions (and the nanos they cast) include a WeaponHit.
fn reaches_weapon_hit(spell: u32, seen: &mut Vec<u32>) -> bool {
    if seen.contains(&spell) {
        return false;
    }
    seen.push(spell);
    let Some(s) = SPELLS.get(&spell) else { return false };
    s.lists.values().flatten().filter(|a| !a.client).any(|a| {
        a.kind == "WeaponHit" || ["Value", "NanoTemplate"].iter().filter_map(|k| a.u(k)).any(|n| reaches_weapon_hit(n, seen))
    })
}

/// The time from pressing `spell` to its hit (ItemDelay), when this module runs it.
pub fn hit_delay(spell: u32) -> Option<Duration> {
    SPELLS.get(&spell).and_then(|s| s.stats.item_delay).map(|cs| Duration::from_millis(10 * cs as u64))
}

/// A plain (not channelled) press of ability `spell` (see `is_ability`), as the original server
/// ran it: PreUse (list 39: e.g. the combo buff that makes the client's next attack the next
/// combo step) right away, OnUse (list 0: the area formulas and hits) after the spell's
/// ItemDelay, when the attack animation strikes.
pub fn cast(caster: Caster, spell: u32) {
    let Some(s) = SPELLS.get(&spell) else { return };
    let delay = hit_delay(spell).unwrap_or(Duration::ZERO);
    let run = Arc::new(Run {
        caster,
        spell,
        started: Instant::now(),
        length: Duration::ZERO,
        released: AtomicBool::new(true),
        counters: Mutex::new(HashMap::new()),
        mitigation_buffs: Mutex::new(Vec::new()),
        ends_when_hit: false,
        aim: Mutex::new(None),
        end_record: Bytes::new(),
    });
    debug!(caster = ?run.caster.id, spell, name = %s.name, ?delay, "ability");
    run_list(&run, spell, PRE_USE, root_target(&run), 0);
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        run_list(&run, spell, ON_USE, root_target(&run), 0);
    });
}

/// Whether `spell` is a channelled ability this module runs.
pub fn is_channel(spell: u32) -> bool {
    SPELLS.get(&spell).is_some_and(|s| s.minifigure.is_some() && s.stats.channeling_interval.is_some())
}

/// Who casts, and at what.
pub struct Caster {
    pub id: Id32,
    pub playfield: u32,
    pub pos: world::Position,
    pub facing: Arc<Mutex<[f32; 2]>>,
    /// The active minifigure and its upgrades (damage, upgrade criteria).
    pub template: u32,
    pub upgrades: u32,
    /// Player level (damage).
    pub level: u32,
    /// The command's target (an enemy) or clicked ground point.
    pub target: Option<Id32>,
    pub ground: Option<[f32; 3]>,
}

/// A running channel.
struct Run {
    caster: Caster,
    spell: u32,
    started: Instant,
    /// Full channel length (all ticks).
    length: Duration,
    released: AtomicBool,
    counters: Mutex<HashMap<u32, u32>>,
    /// Buffs on the caster that mitigate damage, cleared at the end at the latest.
    mitigation_buffs: Mutex<Vec<u32>>,
    ends_when_hit: bool,
    /// Where the player aims now (`UpdateSpellTargetPosition` while channelling).
    aim: Mutex<Option<[f32; 3]>>,
    /// The client's command with action 20: ends its channel.
    end_record: Bytes,
}

static ACTIVE: LazyLock<Mutex<HashMap<Id32, Arc<Run>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Where actions apply: a character (enemy or the caster) or a ground point.
#[derive(Clone, Copy, Debug)]
struct Target {
    id: Option<Id32>,
    pos: [f32; 3],
}

/// Starts channel `spell` (see `is_channel`) for `caster`. `end_record` is the client's command
/// record with BCC action 20. A channel the caster still has running ends first.
pub fn start(caster: Caster, spell: u32, end_record: Bytes) {
    let Some(s) = SPELLS.get(&spell) else { return };
    release(caster.id, false);
    let interval = Duration::from_millis(10 * s.stats.channeling_interval.unwrap_or(50) as u64);
    let ticks = s.stats.channeling_ticks.unwrap_or(1);
    let ends_when_hit = s.lists.values().flatten().any(|a| a.kind == "CastNano" && a.u("Value").is_some_and(|v| ENDS_WHEN_HIT.contains(&v)));
    let run = Arc::new(Run {
        caster,
        spell,
        started: Instant::now(),
        length: interval * ticks,
        released: AtomicBool::new(false),
        counters: Mutex::new(HashMap::new()),
        mitigation_buffs: Mutex::new(Vec::new()),
        ends_when_hit,
        aim: Mutex::new(None),
        end_record,
    });
    info!(caster = ?run.caster.id, spell, name = %s.name, ticks, ?interval, "channel");
    ACTIVE.lock().unwrap().insert(run.caster.id, run.clone());
    for list in [PRE_USE, ON_USE, CHANNEL_START] {
        run_list(&run, spell, list, root_target(&run), 0);
    }
    if let Some(delay) = s.stats.recharge_delay.filter(|_| s.lists.contains_key(ON_RECHARGE_DELAY)) {
        let run = run.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10 * delay as u64)).await;
            run_list(&run, spell, ON_RECHARGE_DELAY, root_target(&run), 0);
        });
    }
    tokio::spawn(async move {
        let mut done = 0;
        let natural = loop {
            if run.released.load(Ordering::Relaxed) {
                break false;
            }
            let t = run.started.elapsed();
            if done < ticks && t >= interval * (done + 1) {
                done += 1;
                run_list(&run, spell, ON_CHANNELING, root_target(&run), 0);
            }
            if done >= ticks {
                break true;
            }
            tokio::time::sleep(POLL).await;
        };
        {
            let mut active = ACTIVE.lock().unwrap();
            if active.get(&run.caster.id).is_some_and(|r| Arc::ptr_eq(r, &run)) {
                active.remove(&run.caster.id);
            }
        }
        if natural {
            // the channel ran its course: end the client's command too
            world::send_to(run.caster.id, world::Out::Records(vec![run.end_record.clone()]));
        }
        info!(caster = ?run.caster.id, spell, ticks = done, natural, "channel end");
        run_list(&run, spell, ON_CHANNELING_END, root_target(&run), 0);
        for buff in run.mitigation_buffs.lock().unwrap().drain(..) {
            crate::npc::set_buff_mitigation(run.caster.id, buff, None);
        }
    });
}

/// Ends `caster`'s channel (released, aborted, or ended by the server: `tell_client`).
/// Returns whether one was running.
pub fn release(caster: Id32, tell_client: bool) -> bool {
    let Some(run) = ACTIVE.lock().unwrap().remove(&caster) else { return false };
    run.released.store(true, Ordering::Relaxed);
    if tell_client {
        world::send_to(caster, world::Out::Records(vec![run.end_record.clone()]));
    }
    true
}

/// Whether `caster` is channelling an ability.
pub fn is_channelling(caster: Id32) -> bool {
    ACTIVE.lock().unwrap().contains_key(&caster)
}

/// `UpdateSpellTargetPosition`: the caster aims its channel at `pos` now.
pub fn aim(caster: Id32, pos: [f32; 3]) {
    if let Some(run) = ACTIVE.lock().unwrap().get(&caster) {
        *run.aim.lock().unwrap() = Some(pos);
    }
}

/// `player` was hit: channels that end when their caster is hit (the Samurai Warrior's stance)
/// end now.
pub fn caster_hit(player: Id32) {
    let ends = ACTIVE.lock().unwrap().get(&player).is_some_and(|r| r.ends_when_hit);
    if ends {
        release(player, true);
    }
}

fn here(run: &Run) -> [f32; 3] {
    *run.caster.pos.lock().unwrap()
}

/// The spell's own target: the caster for self-cast spells, else the targeted enemy, the
/// clicked point, or the caster.
fn root_target(run: &Run) -> Target {
    let me = Target { id: Some(run.caster.id), pos: here(run) };
    let self_cast = SPELLS.get(&run.spell).and_then(|s| s.stats.targeting_mode) == Some(TARGETING_SELF);
    if self_cast {
        return me;
    }
    if let Some((id, pos)) = run.caster.target.filter(|t| *t != run.caster.id).and_then(|t| Some((t, crate::npc::position(t)?))) {
        return Target { id: Some(id), pos };
    }
    match (*run.aim.lock().unwrap()).or(run.caster.ground) {
        Some(pos) => Target { id: None, pos },
        None => me,
    }
}

fn show(run: &Run, records: Vec<Bytes>) {
    if !records.is_empty() {
        world::send_playfield(run.caster.id, true, world::Out::Records(records));
    }
}

fn criteria_met(run: &Run, a: &Action) -> bool {
    a.criteria.iter().all(|&[x, y, kind]| match kind {
        CRITERIA_HAS_ABILITY => crate::combat::has_ability(run.caster.template, run.caster.upgrades, y),
        CRITERIA_BUFF_COUNTER => run.counters.lock().unwrap().get(&x).copied().unwrap_or(0) == y,
        _ => true,
    })
}

/// Runs the server actions of `spell`'s list `event` with `target` as the current target.
fn run_list(run: &Arc<Run>, spell: u32, event: &str, target: Target, depth: u32) {
    let Some(s) = SPELLS.get(&spell) else { return };
    let Some(list) = s.lists.get(event) else { return };
    for a in list.iter().filter(|a| !a.client) {
        if criteria_met(run, a) {
            act(run, spell, a, target, depth);
        }
    }
}

fn act(run: &Arc<Run>, spell: u32, a: &Action, target: Target, depth: u32) {
    let caster = run.caster.id;
    let me = Target { id: Some(caster), pos: here(run) };
    let who = if a.on_caster() { me } else { target };
    match a.kind.as_str() {
        "CastNano" => {
            if let Some(nano) = a.u("Value") {
                cast_nano(run, nano, who, depth + 1);
            }
        }
        "WeaponHit" => {
            if let Some(id) = who.id.filter(|id| *id != caster) {
                let damage = crate::combat::damage_with_mod(run.caster.template, run.caster.upgrades, run.spell, a.f("AbilityMod").unwrap_or(1.0), run.caster.level);
                crate::npc::hit(caster, id, spell, damage);
            }
        }
        "RemoveBuff" => {
            if let (Some(buff), Some(id)) = (a.u("TemplateID"), who.id) {
                run.counters.lock().unwrap().remove(&buff);
                effects::forget_buff(id, buff);
                if id == caster {
                    crate::npc::set_buff_mitigation(caster, buff, None);
                }
                show(run, vec![effects::buff_remove(id, buff, caster)]);
            }
        }
        "AddBuffCounter" => {
            if let (Some(buff), Some(id)) = (a.u("TemplateID"), who.id) {
                let add = a.u("Value").unwrap_or(1);
                let count = {
                    let mut counters = run.counters.lock().unwrap();
                    let c = counters.entry(buff).or_insert(0);
                    *c += add;
                    *c
                };
                let left = channel_left_cs(run);
                show(run, vec![if count == add {
                    effects::buff_add_counter(id, buff, caster, count, left as i32)
                } else {
                    effects::buff_update_counter(id, buff, caster, count, left)
                }]);
            }
        }
        "CoreFilteredAreaFormula" => {
            let center = if a.on_caster() { me.pos } else { target.pos };
            let radius = a.f("Radius").unwrap_or(2.0);
            let min = a.f("Min").unwrap_or(0.0);
            let angle = a.f("Angle").unwrap_or(360.0);
            let from = a.f("AngleStart").unwrap_or(-angle / 2.0);
            let facing = *run.caster.facing.lock().unwrap();
            let mut hits = crate::npc::targets_where(run.caster.playfield, |p| {
                let d = distance(p, center);
                d <= radius && d >= min && (angle >= 360.0 || in_sector(center, facing, p, from, angle))
            });
            hits.sort_by(|(_, x), (_, y)| distance(*x, center).total_cmp(&distance(*y, center)));
            if let Some(max) = a.u("Max1").filter(|m| *m > 0) {
                hits.truncate(max as usize);
            }
            if let Some(nano) = a.u("NanoTemplate") {
                // the client plays the nano's effect on every hit and at the centre
                let ids: Vec<Id32> = hits.iter().map(|(id, _)| *id).collect();
                show(run, vec![area_formula_record(caster, &ids, nano, center)]);
                for (id, pos) in hits {
                    cast_nano(run, nano, Target { id: Some(id), pos }, depth + 1);
                }
            }
        }
        "ColumnFormula" => {
            let from = me.pos;
            let dir = *run.caster.facing.lock().unwrap();
            let length = a.f("Distance").unwrap_or(5.0);
            let width = a.f("FloatValue1").unwrap_or(1.0);
            let hits = crate::npc::targets_where(run.caster.playfield, |p| {
                let (dx, dz) = (p[0] - from[0], p[2] - from[2]);
                let along = dx * dir[0] + dz * dir[1];
                let across = (dx * dir[1] - dz * dir[0]).abs();
                (0.0..=length).contains(&along) && across <= width
            });
            if let Some(nano) = a.u("NanoTemplate") {
                let ids: Vec<Id32> = hits.iter().map(|(id, _)| *id).collect();
                let mid = [from[0] + dir[0] * length / 2.0, from[1], from[2] + dir[1] * length / 2.0];
                show(run, vec![area_formula_record(caster, &ids, nano, mid)]);
                for (id, pos) in hits {
                    cast_nano(run, nano, Target { id: Some(id), pos }, depth + 1);
                }
            }
        }
        "CoreRandomPointAreaFormula" => {
            let center = if a.on_caster() { me.pos } else { target.pos };
            let radius = a.f("FloatRadius").unwrap_or(3.0);
            if let Some(nano) = a.u("NanoTemplate") {
                for _ in 0..a.u("Max1").unwrap_or(1) {
                    let (angle, r) = (random_unit() * std::f32::consts::TAU, radius * random_unit().sqrt());
                    let pos = [center[0] + r * angle.sin(), center[1], center[2] + r * angle.cos()];
                    cast_nano(run, nano, Target { id: None, pos }, depth + 1);
                }
            }
        }
        "CorePersistentGroundEffect" => {
            if let Some(package) = a.u("EffectPackage") {
                show(run, vec![effects::effect_package_at(caster, package, who.pos)]);
            }
            if let Some(nano) = a.u("NanoTemplate") {
                let lifetime = a.u("Arg1").map_or(GROUND_DEFAULT_LIFETIME, |t| Duration::from_millis(100 * t as u64));
                let delay = Duration::from_millis(10 * a.u("DelayTime").unwrap_or(0) as u64);
                ground_zone(run, nano, who.pos, delay, GROUND_PULSE, lifetime, depth);
            }
        }
        "PlaceTrap" => {
            if let Some(nano) = a.u("NanoTemplate") {
                let pulse = Duration::from_millis(10 * a.u("Interval").unwrap_or(100).max(10) as u64);
                let lifetime = Duration::from_secs(a.u("TimedLength").unwrap_or(3) as u64);
                ground_zone(run, nano, who.pos, Duration::ZERO, pulse, lifetime, depth);
            }
        }
        "ColumnDash" => {
            let dash = crate::dash::Dash::column(a.f("Distance").unwrap_or(5.0), a.u("Value1").unwrap_or(0));
            let facing = *run.caster.facing.lock().unwrap();
            crate::dash::start(caster, run.caster.playfield, run.spell, dash, run.caster.pos.clone(), None, facing, |_| 0);
        }
        other => debug!(spell, action = other, "spell action not run"),
    }
}

/// Casts nano `nano` on `target`: shows it as a buff when it is one, then runs its list 0 there.
fn cast_nano(run: &Arc<Run>, nano: u32, target: Target, depth: u32) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(n) = SPELLS.get(&nano) else { return };
    let caster = run.caster.id;
    let on_caster = target.id == Some(caster);
    let server_actions = n.lists.get(ON_USE).is_some_and(|l| l.iter().any(|a| !a.client));
    if let Some(id) = target.id
        && (n.stats.time_exist.is_some() || (on_caster && !server_actions))
    {
        // a buff without TimeExist lasts until the channel's end list removes it, or on a plain
        // press (a combo buff) for the combo window
        let duration = match n.stats.time_exist {
            Some(cs) => Duration::from_millis(10 * cs as u64),
            None if run.length.is_zero() => COMBO_WINDOW,
            None => Duration::from_millis(10 * (channel_left_cs(run) as u64 + 100)),
        };
        show(run, effects::show_buff(id, nano, caster, Some(duration)));
        let mitigation: f32 = n
            .lists
            .get(ON_USE)
            .into_iter()
            .flatten()
            .filter(|a| a.kind == "Modify" && a.u("Stat") == Some(STAT_MITIGATION_FLAT))
            .filter_map(|a| a.f("FloatValue1"))
            .sum();
        if on_caster && mitigation > 0.0 {
            crate::npc::set_buff_mitigation(caster, nano, Some(mitigation));
            run.mitigation_buffs.lock().unwrap().push(nano);
            if let Some(t) = n.stats.time_exist {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(10 * t as u64)).await;
                    crate::npc::set_buff_mitigation(caster, nano, None);
                });
            }
        }
    }
    run_list(run, nano, ON_USE, target, depth);
}

/// A ground zone at `at`: after `delay`, re-applies `nano` there every `pulse` for `lifetime`.
fn ground_zone(run: &Arc<Run>, nano: u32, at: [f32; 3], delay: Duration, pulse: Duration, lifetime: Duration, depth: u32) {
    let run = run.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let until = Instant::now() + lifetime;
        while Instant::now() < until {
            cast_nano(&run, nano, Target { id: None, pos: at }, depth + 1);
            tokio::time::sleep(pulse).await;
        }
    });
}

/// Centiseconds until the channel's full length is over (buff durations).
fn channel_left_cs(run: &Run) -> u32 {
    (run.length.saturating_sub(run.started.elapsed()).as_millis() / 10) as u32
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether `p` lies in the sector from `start` to `start + angle` degrees around `facing`, seen
/// from `center`.
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
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    (nanos.wrapping_mul(2_654_435_761) % 10_000) as f32 / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_data_loads() {
        assert!(is_channel(596_784), "Samurai Warrior's stance");
        assert!(is_channel(357_869), "Barbarian's Spin Slash");
        assert!(!is_channel(596_786), "a nano isn't an ability");
        let stance = &SPELLS[&596_784];
        assert_eq!(stance.stats.channeling_interval, Some(200));
        assert!(stance.lists[ON_CHANNELING_END].iter().any(|a| a.kind == "CoreFilteredAreaFormula"));
    }

    #[test]
    fn sectors() {
        let c = [0.0, 0.0, 0.0];
        let north = [0.0, 1.0];
        assert!(in_sector(c, north, [0.0, 0.0, 2.0], -30.0, 60.0));
        assert!(!in_sector(c, north, [2.0, 0.0, 0.0], -30.0, 60.0));
        assert!(in_sector(c, north, [2.0, 0.0, 0.1], -45.0, 180.0));
        assert!(!in_sector(c, north, [0.0, 0.0, -2.0], -45.0, 90.0));
    }
}

/// `AreaFormulaIIR_t` (caster): u32 (n+1)·1009 + n targets, u32 formula nano, u32 (m+1)·1009 + m
/// points (the original server sent the centre), u32 0. The client plays the nano's
/// EffectPackage on every target and point (`CoreMagicHolder::DoAreaFormula`); sent also
/// without targets.
fn area_formula_record(caster: Id32, targets: &[Id32], nano: u32, center: [f32; 3]) -> Bytes {
    lmoxide_net::iir::record("AreaFormulaIIR_t", caster, false, |w| {
        let w = targets.iter().fold(w.container_len(targets.len()), |w, t| t.write(w));
        w.u32(nano).container_len(1).f32(center[0]).f32(center[1]).f32(center[2]).u32(0)
    })
}
