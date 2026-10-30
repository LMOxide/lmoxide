//! Channelled abilities (held specials: the Barbarian's Spin Slash, the Cyclops' Laser Eye, the
//! Samurai Warrior's stance, ...). The server runs the server-only actions of the spell's game
//! action lists (content/skills `client`, skills.rs) while the client runs
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
//! and PlaceTrap (a ground zone re-applying its nano), CoreRandomPointAreaFormula, ColumnDash,
//! Blink (a leap to the target, then its nano where it lands), Chain (a nano jumping from enemy
//! to enemy, shown with `ChainSpellIIR_t`) and Projectile: a client action the server carries
//! out (the client's does nothing by itself), creating the shot (projectile.rs) whose item's
//! actions run on what it hits or where it lands (content/items projectile `actions`). Area
//! formulas with an Interval and a Duration pulse (-1: for as long as the buff holding them).
//! Crowd control (snares, knockbacks, stuns), diminishing returns and resistances are only shown
//! as the buffs' visuals, except on enemies: their movement is the server's, so a player's slow
//! (AddSnare), stun (Stun) and knockback or pull (Knockaway, AreaKnockaway), client actions the
//! client only shows, are done to them here (npc.rs `snare`, `stun`, `knock`), for the nano's
//! TimeExist and shortened by diminishing returns (AddDiminishingReturn). Timings the client
//! data doesn't give are in docs/TUNABLES.md.

use crate::instance::Pf;
use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    /// The nano's visual (played by the client for buffs; the server sends it for the others).
    #[serde(rename = "EffectPackage")]
    effect_package: Option<u32>,
    /// Spell flags 33-64 (`StatSpellItem::HasFlag`: flag n is bit n-33), e.g. MORE_FLAG_BLOCKS_TURNING.
    #[serde(rename = "MoreFlags")]
    more_flags: Option<u32>,
    /// A counter buff's highest count (AddBuffCounter stops there).
    #[serde(rename = "MaxBuffCounters")]
    max_buff_counters: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Action {
    #[serde(rename = "type")]
    kind: String,
    client: bool,
    #[serde(default)]
    args: HashMap<String, serde_json::Value>,
    /// (a, b, kind): 91 = has upgrade ability b, 101 = lacks it, 166 = buff a's counter is b, 159 / 160 = above / below b; 18 / 19 = the
    /// counters after it are the current target's / the caster's (Petrifying Gaze counts on the enemy, Vladek's Champion on the caster).
    #[serde(default)]
    criteria: Vec<[u32; 3]>,
}

impl Action {
    /// Whether the server runs it: the server-only actions, and Projectile, which the server
    /// carries out (the client's does nothing by itself).
    fn on_server(&self) -> bool {
        !self.client || self.kind == "Projectile"
    }
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
/// The caster lacks upgrade ability b (Triple Shot's plain arrows vs. the ricochet ones).
const CRITERIA_LACKS_ABILITY: u32 = 101;
const CRITERIA_BUFF_COUNTER: u32 = 166;
/// Buff a's counter is above / below b (MOAR RAWR's letters: A while 1-4, W while 5-7).
const CRITERIA_COUNTER_ABOVE: u32 = 159;
const CRITERIA_COUNTER_BELOW: u32 = 160;
const CRITERIA_ON_TARGET: u32 = 18;
const CRITERIA_ON_CASTER: u32 = 19;
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
/// Ground zones (CorePersistentGroundEffect: Arg1 the pulse interval in centiseconds, Arg2 the
/// number of pulses, e.g. Sand Storm 20 x 25): the interval when Arg1 is missing (single pulses).
const GROUND_DEFAULT_PULSE_CS: u32 = 100;
/// The shortest a ground effect's visual is kept.
const GROUND_MIN_VISUAL: Duration = Duration::from_secs(1);
/// Nested nanos deeper than this aren't followed (a guard against loops: the Sad Clown's cloud
/// damage is 6 deep: spawn, 245949, 245964, the orb, its burst, the blast's hit nano).
const MAX_DEPTH: u32 = 10;
/// How long a combo buff without TimeExist stays (the original server removed them ~1 s after
/// the press: recorded Gladius Slash).
const COMBO_WINDOW: Duration = Duration::from_millis(1000);
const POLL: Duration = Duration::from_millis(50);
/// Spell flag 62 (`MagicCommand_c::BlocksTurning`): the caster can't turn while it runs (Squad,
/// Echo, Forceful Yodel). Its shots fly the way it was cast, without homing in, retargeting or
/// turning the caster.
const MORE_FLAG_BLOCKS_TURNING: u32 = 1 << (62 - 33);
/// Projectile items whose shot is a volley that hits every enemy along its line, as it gets
/// there (the Revolutionary Soldier's Squad: a row of soldiers firing), within this width.
const LINE_SHOT_ITEMS: &[u32] = &[234_947];
const LINE_SHOT_WIDTH: f32 = 1.5;
/// How far a channel looks for its next enemy when its target fell (the default shot range).
const RETARGET_RANGE: f32 = 15.0;
/// A caster that moved less than this between shots stands still (turned to its shots).
const STANDING_STILL: f32 = 0.3;
/// Shots within about 15° of the facing don't turn the caster (cos 15°).
const FACING_ALREADY: f32 = 0.966;

static SPELLS: LazyLock<HashMap<u32, Spell>> =
    LazyLock::new(|| crate::skills::spells().into_iter().map(|(k, v)| (k, serde_json::from_value(v).expect("skill"))).collect());

/// A projectile item: what it does where it hits or lands.
#[derive(Debug, Deserialize)]
struct Item {
    #[serde(default)]
    actions: Vec<Action>,
}

static ITEMS: LazyLock<HashMap<u32, Item>> =
    LazyLock::new(|| crate::skills::projectile_items().into_iter().map(|(k, v)| (k, serde_json::from_value(v).expect("projectile item"))).collect());

/// Whether `spell` is a minifigure ability (or combo step) whose actions this module runs on a
/// plain press (`cast`): it has server-side PreUse or OnUse actions.
/// Only those whose actions reach a WeaponHit (the others' damage came from actions the server
/// doesn't run, so they keep the plain hit in game.rs) or a shortcut override (mode switches
/// like Mount Up!, which don't hit).
pub fn is_ability(spell: u32) -> bool {
    SPELLS.get(&spell).is_some_and(|s| {
        s.minifigure.is_some()
            && [PRE_USE, ON_USE].iter().any(|l| s.lists.get(*l).is_some_and(|l| l.iter().any(Action::on_server)))
            && reaches(spell, &["WeaponHit", "OverrideShortcut"], &mut Vec::new())
    })
}

/// Whether `spell`'s server actions (and the nanos they cast) include one of `kinds`.
fn reaches(spell: u32, kinds: &[&str], seen: &mut Vec<u32>) -> bool {
    if seen.contains(&spell) {
        return false;
    }
    seen.push(spell);
    let Some(s) = SPELLS.get(&spell) else { return false };
    s.lists.values().flatten().filter(|a| a.on_server()).any(|a| action_reaches(a, kinds, seen))
}

/// Whether action `a` is one of `kinds` or reaches one (its nanos, its projectile's item).
fn action_reaches(a: &Action, kinds: &[&str], seen: &mut Vec<u32>) -> bool {
    kinds.contains(&a.kind.as_str())
        || ["Value", "NanoTemplate", "TemplateID"].iter().filter_map(|k| a.u(k)).any(|n| reaches(n, kinds, seen))
        || a.u("ItemInstance").and_then(|i| ITEMS.get(&i)).is_some_and(|i| i.actions.iter().filter(|a| a.on_server()).any(|a| action_reaches(a, kinds, seen)))
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
        aborted: AtomicBool::new(false),
        counted: Mutex::new(Vec::new()),
        mitigation_buffs: Mutex::new(Vec::new()),
        ends_when_hit: false,
        aim: Mutex::new(None),
        end_record: Bytes::new(),
        shot_from: Mutex::new(None),
        target: Mutex::new(None),
        locked: Mutex::new(None),
    });
    debug!(caster = ?run.caster.id, spell, name = %s.name, ?delay, "ability");
    lock_direction(&run);
    run_list(&run, spell, PRE_USE, root_target(&run), 0);
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        run_list(&run, spell, ON_USE, root_target(&run), 0);
        end_counters(&run);
    });
}

/// Spells that OverrideShortcut actions of `spells` (and the nanos they reach) put on the
/// shortcut bar: they are cast, so they must be learned.
pub fn override_targets(spells: &[u32]) -> Vec<u32> {
    let mut seen = Vec::new();
    let mut todo = spells.to_vec();
    let mut out = Vec::new();
    while let Some(spell) = todo.pop() {
        if seen.contains(&spell) || seen.len() > 256 {
            continue;
        }
        seen.push(spell);
        let Some(s) = SPELLS.get(&spell) else { continue };
        for a in s.lists.values().flatten() {
            if a.kind == "OverrideShortcut" {
                // (cast in turn: its own buffs may override further)
                out.extend(a.u("TemplateID2"));
                todo.extend(a.u("TemplateID2"));
            }
            todo.extend(["Value", "NanoTemplate"].iter().filter_map(|k| a.u(k)));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Shortcut overrides (`GameCharInterface::OverrideShortcut`), as the original server sent
/// them for a buff's OverrideShortcut actions: the override when the buff goes on (int type 2
/// spell, int position, u32 old template, u32 new template, int switch time in cs; the
/// recorded combo buffs: 2, 104, 317739, 317740/317741, 100), `ClearOverridingShortcut(int
/// position)` when it is removed or runs out.
mod shortcuts {
    use std::{
        collections::HashMap,
        sync::{LazyLock, Mutex},
        time::Duration,
    };

    use lmoxide_net::{Endpoint, Writer, methods::game_char_interface, types::Id32};

    use crate::world;

    const SPELL_SHORTCUT: i32 = 2;
    /// The HUD's normal and special attack shortcuts.
    const FIRST_ATTACK_SLOT: i32 = 104;

    /// (player, buff) -> (positions overridden, generation).
    static ON: LazyLock<Mutex<HashMap<(Id32, u32), (Vec<i32>, u64)>>> = LazyLock::new(Default::default);
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// Buff `buff` on `player` (active minifigure `template`) shows `new` instead of `old`.
    /// `duration`: `None` until the buff is removed (switch time 0, assumed: only the combo
    /// buffs' 100 was recorded).
    pub fn set(player: Id32, buff: u32, template: u32, old: u32, new: u32, duration: Option<Duration>) {
        let Some(i) = crate::collection::spells(template).iter().position(|&s| s == old) else {
            tracing::debug!(buff, old, template, "shortcut override: not the minifigure's attack");
            return;
        };
        let position = FIRST_ATTACK_SLOT + i as i32;
        let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cleared = {
            let mut on = ON.lock().unwrap();
            let entry = on.entry((player, buff)).or_insert_with(|| (Vec::new(), generation));
            // the buff applied again: its overrides start over
            let cleared = if entry.1 != generation && entry.0.contains(&position) { Some(position) } else { None };
            if !entry.0.contains(&position) {
                entry.0.push(position);
            }
            entry.1 = generation;
            cleared
        };
        if let Some(p) = cleared {
            send_clear(player, p);
        }
        let cs = duration.map_or(0, |d| (d.as_millis() / 10) as i32);
        let args = Writer::new().i32(SPELL_SHORTCUT).i32(position).u32(old).u32(new).i32(cs).finish();
        world::send_to(player, world::Out::Rpc(Endpoint::GameCharInterface, game_char_interface::OVERRIDE_SHORTCUT, args));
        if let (Some(duration), Ok(rt)) = (duration, tokio::runtime::Handle::try_current()) {
            rt.spawn(async move {
                tokio::time::sleep(duration).await;
                let expired = {
                    let mut on = ON.lock().unwrap();
                    match on.get(&(player, buff)) {
                        Some((_, g)) if *g == generation => on.remove(&(player, buff)).map(|(p, _)| p),
                        _ => None,
                    }
                };
                for p in expired.into_iter().flatten() {
                    send_clear(player, p);
                }
            });
        }
    }

    /// Buff `buff` was removed from `player`: its overrides end.
    pub fn clear(player: Id32, buff: u32) {
        let positions = ON.lock().unwrap().remove(&(player, buff)).map(|(p, _)| p);
        for p in positions.into_iter().flatten() {
            send_clear(player, p);
        }
    }

    fn send_clear(player: Id32, position: i32) {
        let args = Writer::new().i32(position).finish();
        world::send_to(player, world::Out::Rpc(Endpoint::GameCharInterface, game_char_interface::CLEAR_OVERRIDING_SHORTCUT, args));
    }
}

/// Whether `spell` is a channelled ability this module runs.
pub fn is_channel(spell: u32) -> bool {
    SPELLS.get(&spell).is_some_and(|s| s.minifigure.is_some() && s.stats.channeling_interval.is_some())
}

/// Who casts, and at what.
pub struct Caster {
    pub id: Id32,
    pub playfield: Pf,
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
    /// Ended by a minifigure change: its end list doesn't run.
    aborted: AtomicBool,
    /// The buff counters this run counted ((holder, buff)): those on the caster, and those
    /// without a TimeExist, end with it (`end_counters`).
    counted: Mutex<Vec<(Id32, u32)>>,
    /// Buffs on the caster that mitigate damage, cleared at the end at the latest.
    mitigation_buffs: Mutex<Vec<u32>>,
    ends_when_hit: bool,
    /// Where the player aims now (`UpdateSpellTargetPosition` while channelling).
    aim: Mutex<Option<[f32; 3]>>,
    /// The client's command with action 20: ends its channel.
    end_record: Bytes,
    /// Where the caster stood at the last shot (a caster who stays put is turned to its shots).
    shot_from: Mutex<Option<[f32; 3]>>,
    /// The enemy a channel switched to after its target fell (`root_target`).
    target: Mutex<Option<Id32>>,
    /// The direction a spell that blocks turning was cast in (its shots keep it).
    locked: Mutex<Option<[f32; 2]>>,
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
        aborted: AtomicBool::new(false),
        counted: Mutex::new(Vec::new()),
        mitigation_buffs: Mutex::new(Vec::new()),
        ends_when_hit,
        aim: Mutex::new(None),
        end_record,
        shot_from: Mutex::new(None),
        target: Mutex::new(None),
        locked: Mutex::new(None),
    });
    info!(caster = ?run.caster.id, spell, name = %s.name, ticks, ?interval, "channel");
    lock_direction(&run);
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
        if !run.aborted.load(Ordering::Relaxed) {
            run_list(&run, spell, ON_CHANNELING_END, root_target(&run), 0);
        }
        end_counters(&run);
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

/// Buffs abilities put on their caster (stances, combo buffs, Mr. Good and Evil's evil side),
/// by caster: they belong to the minifigure that cast them (`minifigure_changed`).
static CASTER_BUFFS: LazyLock<Mutex<HashMap<Id32, Vec<u32>>>> = LazyLock::new(Default::default);

/// `player`'s active minifigure changed: what its abilities left on the player ends. Its
/// channel ends at once (the client's command too) without its end list, and the buffs, buff
/// counters and shortcut overrides its abilities put on the player are removed (they stayed on
/// the next minifigure: Vladek's Champion's charge-up, Mr. Good and Evil's evil side).
pub fn minifigure_changed(player: Id32) {
    if let Some(run) = ACTIVE.lock().unwrap().get(&player) {
        run.aborted.store(true, Ordering::Relaxed);
    }
    release(player, true);
    let buffs = CASTER_BUFFS.lock().unwrap().remove(&player).unwrap_or_default();
    let counters: Vec<u32> = {
        let mut all = COUNTERS.lock().unwrap();
        let mine: Vec<u32> = all.keys().filter(|(holder, _)| *holder == player).map(|(_, buff)| *buff).collect();
        for buff in &mine {
            all.remove(&(player, *buff));
        }
        mine
    };
    let mut records = Vec::new();
    for buff in buffs {
        crate::npc::set_buff_mitigation(player, buff, None);
        shortcuts::clear(player, buff);
        if effects::is_shown(player, buff) {
            effects::forget_buff(player, buff);
            records.push(effects::buff_remove(player, buff, player));
        }
    }
    records.extend(counters.into_iter().map(|buff| effects::buff_remove(player, buff, player)));
    if !records.is_empty() {
        debug!(?player, n = records.len(), "minifigure changed: ability buffs removed");
        world::send_playfield(player, true, world::Out::Records(records));
    }
}

/// Forgets `player`'s ability buffs (it left).
pub fn forget_caster_buffs(player: Id32) {
    CASTER_BUFFS.lock().unwrap().remove(&player);
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

/// The spell's own target: the caster for self-cast spells (TargetingMode 8, and channels
/// without a TargetingMode, as enemies' spins: npc_spells.rs), else the targeted enemy, the
/// clicked point, or the caster.
fn root_target(run: &Run) -> Target {
    let me = Target { id: Some(run.caster.id), pos: here(run) };
    let self_cast = SPELLS.get(&run.spell).is_some_and(|s| match s.stats.targeting_mode {
        Some(mode) => mode == TARGETING_SELF,
        None => s.stats.channeling_interval.is_some(),
    });
    if self_cast {
        return me;
    }
    let aimed_at = run.caster.target.filter(|t| *t != run.caster.id);
    let current = (*run.target.lock().unwrap()).or(aimed_at);
    if let Some((id, pos)) = current.and_then(|t| Some((t, crate::npc::position(t)?))) {
        return Target { id: Some(id), pos };
    }
    // a channel whose enemy fell goes on at the nearest one, as the R key's auto-aim picks one
    // (else the rest of a volley flew straight ahead)
    if aimed_at.is_some() && !run.length.is_zero() && run.locked.lock().unwrap().is_none() {
        let next = crate::npc::targets_where(run.caster.playfield, |p| distance(p, me.pos) <= RETARGET_RANGE)
            .into_iter()
            .min_by(|(_, x), (_, y)| distance(*x, me.pos).total_cmp(&distance(*y, me.pos)));
        if let Some((id, pos)) = next {
            *run.target.lock().unwrap() = Some(id);
            return Target { id: Some(id), pos };
        }
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

fn criteria_met(run: &Run, a: &Action, target: Target) -> bool {
    let mut holder = run.caster.id;
    a.criteria.iter().all(|&[x, y, kind]| match kind {
        CRITERIA_ON_TARGET => {
            holder = target.id.unwrap_or(run.caster.id);
            true
        }
        CRITERIA_ON_CASTER => {
            holder = run.caster.id;
            true
        }
        CRITERIA_HAS_ABILITY => crate::combat::has_ability(run.caster.template, run.caster.upgrades, y),
        CRITERIA_LACKS_ABILITY => !crate::combat::has_ability(run.caster.template, run.caster.upgrades, y),
        CRITERIA_BUFF_COUNTER => counter(holder, x) == y,
        CRITERIA_COUNTER_ABOVE => counter(holder, x) > y,
        CRITERIA_COUNTER_BELOW => counter(holder, x) < y,
        _ => true,
    })
}

/// A buff counter (AddBuffCounter) and the generation of its latest change.
struct Counter {
    count: u32,
    generation: u64,
}

/// Buff counters by (holder, buff). They live on the character, as the counter buff did on
/// live: an enemy's Petrifying Gaze count is its own and outlasts a press, until the buff's
/// TimeExist runs out after the last count.
static COUNTERS: LazyLock<Mutex<HashMap<(Id32, u32), Counter>>> = LazyLock::new(Default::default);
static COUNTER_GENERATION: AtomicU64 = AtomicU64::new(0);

/// `holder`'s count of counter buff `buff`.
fn counter(holder: Id32, buff: u32) -> u32 {
    COUNTERS.lock().unwrap().get(&(holder, buff)).map_or(0, |c| c.count)
}

/// Counts `add` on `holder`'s counter buff `buff` (up to its MaxBuffCounters) and shows it.
/// It is removed when its TimeExist is up after this count, else at the run's end.
fn add_counter(run: &Run, holder: Id32, buff: u32, add: u32) -> Bytes {
    let stats = SPELLS.get(&buff).map(|s| &s.stats);
    let max = stats.and_then(|s| s.max_buff_counters).unwrap_or(u32::MAX);
    let generation = COUNTER_GENERATION.fetch_add(1, Ordering::Relaxed);
    let (count, first) = {
        let mut counters = COUNTERS.lock().unwrap();
        let c = counters.entry((holder, buff)).or_insert(Counter { count: 0, generation });
        let first = c.count == 0;
        c.count = (c.count + add).min(max);
        c.generation = generation;
        (c.count, first)
    };
    run.counted.lock().unwrap().push((holder, buff));
    let lasts = stats.and_then(|s| s.time_exist).map(|cs| Duration::from_millis(10 * u64::from(cs)));
    let left = lasts.map_or_else(|| channel_left_cs(run), |d| (d.as_millis() / 10) as u32);
    if let Some(d) = lasts
        && let Ok(rt) = tokio::runtime::Handle::try_current()
    {
        let caster = run.caster.id;
        rt.spawn(async move {
            tokio::time::sleep(d).await;
            let gone = {
                let mut counters = COUNTERS.lock().unwrap();
                let current = counters.get(&(holder, buff)).is_some_and(|c| c.generation == generation);
                current && counters.remove(&(holder, buff)).is_some()
            };
            if gone {
                world::send_playfield(holder, true, world::Out::Records(vec![effects::buff_remove(holder, buff, caster)]));
            }
        });
    }
    if first {
        effects::buff_add_counter(holder, buff, run.caster.id, count, left as i32)
    } else {
        effects::buff_update_counter(holder, buff, run.caster.id, count, left)
    }
}

/// Forgets `holder`'s counter buff `buff`; whether it had one (the caller removes it on the
/// client).
fn clear_counter(holder: Id32, buff: u32) -> bool {
    COUNTERS.lock().unwrap().remove(&(holder, buff)).is_some()
}

/// The run is over: its counters on the caster, and those without a TimeExist, end.
fn end_counters(run: &Run) {
    let counted: Vec<_> = std::mem::take(&mut *run.counted.lock().unwrap());
    let mut records = Vec::new();
    for (holder, buff) in counted {
        let timed = SPELLS.get(&buff).is_some_and(|s| s.stats.time_exist.is_some());
        if (holder == run.caster.id || !timed) && clear_counter(holder, buff) {
            records.push(effects::buff_remove(holder, buff, run.caster.id));
        }
    }
    show(run, records);
}

/// Forgets every buff counter `id` holds (it left).
pub fn forget_counters(id: Id32) {
    COUNTERS.lock().unwrap().retain(|(holder, _), _| *holder != id);
}

/// Runs the server actions of `spell`'s list `event` with `target` as the current target, and
/// the crowd control its client actions do to an enemy.
fn run_list(run: &Arc<Run>, spell: u32, event: &str, target: Target, depth: u32) {
    let Some(s) = SPELLS.get(&spell) else { return };
    let Some(list) = s.lists.get(event) else { return };
    for a in list.iter().filter(|a| a.on_server() || CROWD_CONTROL.contains(&a.kind.as_str())) {
        if criteria_met(run, a, target) {
            if a.on_server() {
                act(run, spell, a, target, depth);
            } else {
                crowd_control(run, spell, a, target);
            }
        }
    }
}

/// Client actions that move or hold a character: the client does them to players itself, the
/// server to enemies.
const CROWD_CONTROL: &[&str] = &["AddSnare", "Stun", "Knockaway", "AreaKnockaway"];
/// How long a slow or stun without TimeExist lasts (assumed).
const DEFAULT_CROWD_CONTROL: Duration = Duration::from_secs(2);
/// Knockback speed when the action has none (m/s; the data says 25-50).
const DEFAULT_KNOCK_SPEED: f32 = 25.0;

/// `spell`'s crowd control action `a` on an enemy `target` (the caster's own slows are its
/// client's).
fn crowd_control(run: &Arc<Run>, spell: u32, a: &Action, target: Target) {
    let caster = run.caster.id;
    if a.on_caster() {
        return;
    }
    let duration = SPELLS.get(&spell).and_then(|s| s.stats.time_exist).map_or(DEFAULT_CROWD_CONTROL, |cs| Duration::from_millis(10 * u64::from(cs)));
    let speed = a.f("Speed").unwrap_or(DEFAULT_KNOCK_SPEED);
    let enemy = target.id.filter(|id| *id != caster && crate::npc::is_enemy(*id));
    match a.kind.as_str() {
        "AddSnare" => {
            if let Some(id) = enemy {
                let got = crate::npc::snare(id, a.f("FloatMultiplier").unwrap_or(1.0), duration);
                debug!(?caster, spell, enemy = ?id, ?got, "snare");
            }
        }
        "Stun" => {
            if let Some(id) = enemy {
                let got = crate::npc::stun(id, stun_linger().unwrap_or(duration));
                debug!(?caster, spell, enemy = ?id, ?got, "stun");
            }
        }
        "Knockaway" => {
            if let Some(id) = enemy {
                let from = here(run);
                // the data names pulls ("knocktowards", "darkpull", "pull"); the rest push away
                let name = SPELLS.get(&spell).map(|s| s.name.to_lowercase()).unwrap_or_default();
                let towards = name.contains("pull") || name.contains("towards");
                crate::npc::knock(id, from, a.f("Distance").unwrap_or(0.0), towards, speed);
            }
        }
        "AreaKnockaway" => {
            let Some((playfield, _)) = world::whereabouts(caster) else { return };
            let center = target.pos;
            let radius = a.f("Radius").unwrap_or(2.0);
            let distance = a.f("Distance").unwrap_or(0.0);
            for (id, _) in crate::npc::targets_where(playfield, |p| (p[0] - center[0]).hypot(p[2] - center[2]) <= radius) {
                if crate::npc::is_enemy(id) {
                    crate::npc::knock(id, center, distance, false, speed);
                }
            }
        }
        _ => {}
    }
}

fn act(run: &Arc<Run>, spell: u32, a: &'static Action, target: Target, depth: u32) {
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
            // (a smashable takes no buffs: see cast_nano_shown)
            if let (Some(buff), Some(id)) = (a.u("TemplateID"), who.id.filter(|id| !crate::npc::is_smashable(*id))) {
                clear_counter(id, buff);
                effects::forget_buff(id, buff);
                if id == caster {
                    crate::npc::set_buff_mitigation(caster, buff, None);
                }
                shortcuts::clear(id, buff);
                show(run, vec![effects::buff_remove(id, buff, caster)]);
            }
        }
        "AddDiminishingReturn" => {
            if let Some(id) = who.id.filter(|id| *id != caster && crate::npc::is_enemy(*id)) {
                let window = Duration::from_millis(10 * u64::from(a.u("Value").unwrap_or(1000)));
                crate::npc::diminish(id, a.u("Type").unwrap_or(0), window, a.f("FloatValue1").unwrap_or(0.0));
            }
        }
        "AddBuffCounter" => {
            // a smashable takes no buffs (the clients crashed on Ancient Bolas' snare counter on
            // one: see cast_nano_shown)
            if let (Some(buff), Some(id)) = (a.u("TemplateID"), who.id.filter(|id| !crate::npc::is_smashable(*id))) {
                show(run, vec![add_counter(run, id, buff, a.u("Value").unwrap_or(1))]);
            }
        }
        "CoreFilteredAreaFormula" => {
            let centre = if a.on_caster() { me } else { target };
            // with a Duration (-1: as long as the buff holding it lasts), it pulses every
            // Interval around its centre, following a character (the Lifeguard's whirlpool)
            let lasts = match a.f("Duration") {
                Some(d) if d < 0.0 => SPELLS.get(&spell).and_then(|s| s.stats.time_exist).map(|cs| cs as f32),
                d => d.filter(|d| *d > 0.0),
            };
            match (lasts, a.f("Interval").filter(|i| *i > 0.0)) {
                (Some(lasts), Some(interval)) => {
                    let run = run.clone();
                    tokio::spawn(async move {
                        let until = Instant::now() + Duration::from_millis(10 * lasts as u64);
                        while Instant::now() < until {
                            let pos = match centre.id {
                                Some(id) if id == run.caster.id => here(&run),
                                Some(id) => match crate::npc::position(id) {
                                    Some(p) => p,
                                    None => break,
                                },
                                None => centre.pos,
                            };
                            area_formula(&run, a, pos, depth);
                            tokio::time::sleep(Duration::from_millis(10 * interval as u64)).await;
                        }
                    });
                }
                _ => area_formula(run, a, centre.pos, depth),
            }
        }
        "ColumnFormula" => {
            // from the caster, or from a ground point (where a bomb landed); Value1 turns it
            // by quarters (the Cross Bomb's four arms)
            let from = if target.id.is_none() && !a.on_caster() { target.pos } else { me.pos };
            let dir = rotate(*run.caster.facing.lock().unwrap(), 90.0 * a.u("Value1").unwrap_or(0) as f32);
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
                    cast_nano_shown(run, nano, Target { id: Some(id), pos }, depth + 1, false);
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
            let pulse = Duration::from_millis(10 * a.u("Arg1").unwrap_or(GROUND_DEFAULT_PULSE_CS).max(1) as u64);
            let pulses = a.u("Arg2").unwrap_or(1).max(1);
            let delay = Duration::from_millis(10 * a.u("DelayTime").unwrap_or(0) as u64);
            let lifetime = (delay + pulse * pulses).max(GROUND_MIN_VISUAL);
            if let Some(package) = a.u("EffectPackage") {
                // the ground effect (e.g. Sand Storm's ring) lasts as long as the zone
                let instance = effects::new_instance();
                show(run, vec![effects::ground_effect(caster, package, who.pos, spell, instance)]);
                let run = run.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(lifetime).await;
                    show(&run, vec![effects::stop_effect_package(run.caster.id, package, spell, instance)]);
                });
            }
            if let Some(nano) = a.u("NanoTemplate") {
                ground_zone(run, nano, who.pos, delay, pulse, pulse * pulses, depth);
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
        "Projectile" => shoot(run, spell, a, who, depth),
        "SpawnMonster" => {
            // the original server's monster (StringHash, content npcs `spawn_hash`) where the
            // spell aims (the clicked point, or where the enemy clicked on stands: it stays
            // there), then the spell's NanoTemplate cast on it (the Sad Clown's clouds circle it)
            let Some(hash) = a.args.get("StringHash").and_then(|v| v.as_str()) else { return };
            match crate::npc::spawn_monster(caster, run.caster.playfield, hash, who.pos, None) {
                Some(id) => {
                    // (without the nano's own effect: live showed nothing where the anchor appeared)
                    if let Some(nano) = a.u("NanoTemplate") {
                        cast_nano_shown(run, nano, Target { id: Some(id), pos: who.pos }, depth + 1, false);
                    }
                }
                None => debug!(spell, hash, "spawn monster: no template with this spawn_hash"),
            }
        }
        "Chain" => {
            if let (Some(first), Some(nano)) = (who.id.filter(|id| *id != caster && chainable(*id)), a.u("TemplateID")) {
                let jumps = a.u("Value").unwrap_or(1);
                let radius = a.f("Radius").or(a.f("Distance")).unwrap_or(4.0);
                // (DelayTime 100 read as milliseconds: a second per jump was far too slow)
                let delay = Duration::from_millis(a.u("DelayTime").unwrap_or(0) as u64);
                chain(run, nano, first, jumps, radius, delay, depth);
            }
        }
        "Blink" => {
            let point = (who.id != Some(caster)).then_some(who.pos);
            let facing = *run.caster.facing.lock().unwrap();
            let (run, nano) = (run.clone(), a.u("TemplateID"));
            crate::dash::leap(caster, run.caster.pos.clone(), point, facing, a.f("Distance").unwrap_or(10.0), move |at| {
                if let Some(nano) = nano {
                    cast_nano(&run, nano, Target { id: Some(run.caster.id), pos: at }, depth + 1);
                }
            });
        }
        "OverrideShortcut" => {
            // buff `spell` puts TemplateID2 on the shortcut showing TemplateID while it is on
            // (combo steps, mode switches)
            if let (Some(id), Some(old), Some(new), Some(n)) = (who.id, a.u("TemplateID"), a.u("TemplateID2"), SPELLS.get(&spell))
                && !crate::npc::is_npc(id)
            {
                shortcuts::set(id, spell, run.caster.template, old, new, buff_duration(run, n));
            }
        }
        other => debug!(spell, action = other, "spell action not run"),
    }
}

/// Casts nano `nano` on `target`: shows it as a buff when it is one, then runs its list 0 there.
fn cast_nano(run: &Arc<Run>, nano: u32, target: Target, depth: u32) {
    cast_nano_shown(run, nano, target, depth, true)
}

/// `cast_nano`; `own_effect`: send a non-buff nano's EffectPackage on the target (not for an
/// area formula's nano: the formula record shows it).
fn cast_nano_shown(run: &Arc<Run>, nano: u32, target: Target, depth: u32, own_effect: bool) {
    cast_nano_on(run, nano, target, depth, own_effect, false)
}

/// `cast_nano_shown`; `at_hit`: cast by a projectile where it hit, so a self-cast nano is
/// centred there (the Sad Clown's cloud burst 222207, TargetingMode 8), not on the caster.
fn cast_nano_on(run: &Arc<Run>, nano: u32, target: Target, depth: u32, own_effect: bool, at_hit: bool) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(n) = SPELLS.get(&nano) else { return };
    let caster = run.caster.id;
    // a self-cast nano (TargetingMode 8: the Lifeguard's whirlpool buff) goes on the caster
    let target = if !at_hit && n.stats.targeting_mode == Some(TARGETING_SELF) && target.id.is_some() { Target { id: Some(caster), pos: here(run) } } else { target };
    let on_caster = target.id == Some(caster);
    // (a shortcut override is what a buff does, not a hit)
    let server_actions = n.lists.get(ON_USE).is_some_and(|l| l.iter().any(|a| a.on_server() && a.kind != "OverrideShortcut"));
    // (a server-made nano's TimeExist is only how long what it sends lasts: the clients don't know it)
    let is_buff = target.id.is_some() && !crate::skills::is_server_made(nano) && (n.stats.time_exist.is_some() || (on_caster && !server_actions));
    // a smashable takes no buffs: it isn't a character, and the clients crashed on one with a
    // client action (Chicken Suit Guy's egg splash snares, AddSnare, on a Broken Wagon)
    if is_buff && target.id.is_some_and(crate::npc::is_smashable) {
        run_list(run, nano, ON_USE, target, depth);
        return;
    }
    if !is_buff
        && own_effect
        && let (Some(id), Some(package)) = (target.id, n.stats.effect_package)
    {
        // a hit nano's own effect on whom it hits (live: e.g. 318181 -> 319901)
        show(run, vec![effects::effect_package(id, package)]);
    }
    if let Some(id) = target.id
        && is_buff
    {
        // a buff without TimeExist lasts until the channel's end list removes it, or on a plain
        // press (a combo buff) for the combo window; a channel tick re-casting a buff still on
        // only renews it (adding it again restarts its animation: the Barbarian's spin)
        let duration = buff_duration(run, n);
        let refreshed = duration.filter(|_| !run.length.is_zero()).and_then(|d| effects::refresh_buff(id, nano, caster, d));
        show(run, refreshed.unwrap_or_else(|| effects::show_buff(id, nano, caster, duration)));
        if on_caster {
            let mut buffs = CASTER_BUFFS.lock().unwrap();
            let mine = buffs.entry(caster).or_default();
            if !mine.contains(&nano) {
                mine.push(nano);
            }
        }
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

/// One pulse of a CoreFilteredAreaFormula around `center`: the enemies in the shape (nearest
/// first, at most Max1) get its nano.
fn area_formula(run: &Arc<Run>, a: &'static Action, center: [f32; 3], depth: u32) {
    let caster = run.caster.id;
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
            cast_nano_shown(run, nano, Target { id: Some(id), pos }, depth + 1, false);
        }
    }
}

/// A Projectile action of `spell`: the server shoots its item from the caster, towards `target`
/// (an enemy, a ground point, or along the facing when it is the caster), turned by RotY degrees
/// and started DisplaceMentX/Z centimetres to the side/ahead (a fan of shots, the Body Guard's
/// row). An unturned shot at an enemy is aimed at it. What it hits, or where a lobbed one lands,
/// gets the item's actions.
fn shoot(run: &Arc<Run>, spell: u32, a: &'static Action, target: Target, depth: u32) {
    let Some(item) = a.u("ItemInstance") else { return };
    let caster = run.caster.id;
    let from = here(run);
    let (move_type, speed) = (a.u("ProjectileType").unwrap_or(1), a.f("Speed").unwrap_or(1000.0) / 100.0);
    if move_type == crate::projectile::ORBITING {
        // it circles whom it is cast on: the caster (the Fortune Teller's cards go where she
        // goes) or the spell's target (a spawned anchor)
        let centre = target.id.unwrap_or(caster);
        let circle = crate::projectile::Circle {
            item,
            speed,
            centre,
            angle: a.f("RotY").unwrap_or(0.0),
            offset: ["DisplaceMentX", "DisplaceMentY", "DisplaceMentZ"].map(|k| a.f(k).unwrap_or(0.0) / 100.0),
            lifetime: orbit_lifetime(spell, centre),
            hits_players: false,
        };
        if centre != caster && SPELLS.get(&spell).is_some_and(|s| s.stats.time_exist.is_none()) {
            // the original server showed the spell sending them as a buff on what they circle
            // for as long as they last (live: the clouds' 222171/222197/222198 on the anchor, 500 cs)
            show(run, effects::show_buff(centre, spell, caster, Some(circle.lifetime)));
        }
        let playfield = run.caster.playfield;
        let run = run.clone();
        let on_hit: crate::projectile::OnHit = Arc::new(move |hit, pos| impact(&run, spell, item, Target { id: hit, pos }, depth));
        crate::projectile::launch_orbit(caster, playfield, spell, &circle, on_hit);
        return;
    }
    let shots = if move_type == crate::projectile::BALLISTIC { crate::projectile::Shots::Lobbed } else { crate::projectile::Shots::Straight };
    // an enemy is aimed at as every player's shot is (`projectile::aim_at`); one that is no
    // longer there (smashed, defeated) isn't, and isn't named to the clients either
    let aim = target.id.filter(|t| *t != caster && crate::npc::is_npc(*t)).and_then(|t| crate::projectile::aim_at(t, from, speed, shots));
    let target = Target { pos: aim.unwrap_or(target.pos), id: target.id.filter(|t| *t == caster || aim.is_some() || !crate::npc::is_npc(*t)) };
    let facing = *run.caster.facing.lock().unwrap();
    let at_self = target.id == Some(caster);
    let (dx, dz) = (target.pos[0] - from[0], target.pos[2] - from[2]);
    let d = dx.hypot(dz);
    let locked = *run.locked.lock().unwrap();
    let base = match locked {
        Some(dir) => dir,
        None if at_self || d < 0.01 => facing,
        None => [dx / d, dz / d],
    };
    let turn = a.f("RotY").unwrap_or(0.0);
    let dir = rotate(base, turn);
    let (side, ahead) = (a.f("DisplaceMentX").unwrap_or(0.0) / 100.0, a.f("DisplaceMentZ").unwrap_or(0.0) / 100.0);
    let start = [from[0] + dir[1] * side + dir[0] * ahead, from[1], from[2] - dir[0] * side + dir[1] * ahead];
    let reach = if at_self { 0.5 } else { d.max(0.5) };
    let towards = [start[0] + dir[0] * reach, if at_self { from[1] } else { target.pos[1] }, start[2] + dir[1] * reach];
    let aimed = target.id.filter(|t| *t != caster && turn == 0.0 && locked.is_none() && crate::npc::is_npc(*t));
    if locked.is_none() {
        face_shot(run, from, base);
    }
    // the volley hits every enemy along its line, as it gets there
    let volley = LINE_SHOT_ITEMS.contains(&item);
    let shot = crate::projectile::Shot {
        item,
        move_type,
        speed,
        attractor: a.u("AttractorID").unwrap_or(0),
        ricochets: a.u("MaxRicochets").unwrap_or(0),
        chains: a.u("MaxChains").unwrap_or(0),
        range: a.f("AttackRange").map_or(0.0, |r| r / 100.0),
        min_range: a.f("MinimumAttackRange").map_or(0.0, |r| r / 100.0),
        width: volley.then_some(LINE_SHOT_WIDTH),
        piercing: volley,
    };
    let on_hit: crate::projectile::OnHit = {
        let run = run.clone();
        Arc::new(move |hit, pos| impact(&run, spell, item, Target { id: hit, pos }, depth))
    };
    crate::projectile::launch_shot(caster, run.caster.playfield, spell, &shot, start, towards, aimed.filter(|_| !volley), on_hit);
}

/// How long the orbiting shots of `spell` circle `centre`: the spell's TimeExist (the Wheel of
/// Fortune's 5 s), else `projectile::ORBIT_LIFETIME` (live's clouds), at most as long as the
/// spawned anchor they circle stays.
fn orbit_lifetime(spell: u32, centre: Id32) -> Duration {
    let lasts = SPELLS.get(&spell).and_then(|s| s.stats.time_exist).map_or(crate::projectile::ORBIT_LIFETIME, |cs| Duration::from_millis(10 * u64::from(cs)));
    crate::npc::spawned_time_left(centre).map_or(lasts, |left| lasts.min(left))
}

/// A spell that blocks turning keeps the direction it was cast in: towards its target (an enemy
/// or the clicked point), else the caster's facing.
fn lock_direction(run: &Run) {
    let flags = SPELLS.get(&run.spell).and_then(|s| s.stats.more_flags).unwrap_or(0);
    if flags & MORE_FLAG_BLOCKS_TURNING == 0 {
        return;
    }
    let from = here(run);
    let to = root_target(run);
    let (dx, dz) = (to.pos[0] - from[0], to.pos[2] - from[2]);
    let d = dx.hypot(dz);
    let dir = if to.id == Some(run.caster.id) || d < 0.01 { *run.caster.facing.lock().unwrap() } else { [dx / d, dz / d] };
    *run.locked.lock().unwrap() = Some(dir);
}

/// Turns the caster towards a shot's direction `dir` when it stands still (hasn't moved since its
/// last shot or the press): the client turns only to a command's target character, so shots at
/// an auto-aimed point (the R key) or at the next enemy (Wild Ride) would leave it facing away.
/// A walking caster keeps its own facing (the turn record would stop it).
fn face_shot(run: &Run, at: [f32; 3], dir: [f32; 2]) {
    let standing = {
        let mut last = run.shot_from.lock().unwrap();
        let standing = last.is_none_or(|p| distance(p, at) < STANDING_STILL);
        *last = Some(at);
        standing
    };
    let mut facing = run.caster.facing.lock().unwrap();
    if !standing || facing[0] * dir[0] + facing[1] * dir[1] > FACING_ALREADY {
        return;
    }
    *facing = dir;
    drop(facing);
    show(run, vec![crate::dash::stop_record(run.caster.id, at, dir)]);
}

/// A projectile item's actions where it hit or landed.
fn impact(run: &Arc<Run>, spell: u32, item: u32, target: Target, depth: u32) {
    let Some(i) = ITEMS.get(&item) else { return };
    for a in i.actions.iter().filter(|a| a.on_server() && criteria_met(run, a, target)) {
        match (a.kind.as_str(), a.u("Value")) {
            ("CastNano", Some(nano)) if !a.on_caster() => cast_nano_on(run, nano, target, depth + 2, true, true),
            _ => act(run, spell, a, target, depth + 1),
        }
    }
}

/// A Chain: `nano` on `first`, then up to `jumps` times on the nearest enemy within `radius` of
/// the last one that it hasn't hit yet (a smashable only when no enemy is in reach), `delay`
/// apart. Each jump after the first is shown with `ChainSpellIIR_t` (the arc from the previous
/// one); the first bolt is the spell's own effect (Lightning Blast's 506677: an arc from the
/// caster too showed it twice).
fn chain(run: &Arc<Run>, nano: u32, first: Id32, jumps: u32, radius: f32, delay: Duration, depth: u32) {
    let run = run.clone();
    tokio::spawn(async move {
        let caster = run.caster.id;
        let (mut last, mut next) = (caster, Some(first));
        let mut hit = Vec::new();
        while let Some(to) = next {
            let Some(pos) = crate::npc::position(to) else { break };
            if last != caster {
                show(&run, vec![effects::chain_jump(caster, to, last, nano)]);
            }
            cast_nano_shown(&run, nano, Target { id: Some(to), pos }, depth + 1, false);
            hit.push(to);
            if hit.len() > jumps as usize {
                break;
            }
            // the nearest enemy, else the nearest smashable
            next = crate::npc::targets_where(run.caster.playfield, |p| distance(p, pos) <= radius)
                .into_iter()
                .filter(|(id, _)| !hit.contains(id) && chainable(*id))
                .min_by(|(a, x), (b, y)| crate::npc::is_smashable(*a).cmp(&crate::npc::is_smashable(*b)).then(distance(*x, pos).total_cmp(&distance(*y, pos))))
                .map(|(id, _)| id);
            last = to;
            if next.is_some() {
                tokio::time::sleep(delay).await;
            }
        }
        debug!(?caster, nano, hits = hit.len(), "chain");
    });
}

/// What a chain jumps to: enemies and smashables.
fn chainable(id: Id32) -> bool {
    crate::npc::is_enemy(id) || crate::npc::is_smashable(id)
}

/// `dir` turned by `degrees` (positive: clockwise seen from above, as RotY).
fn rotate(dir: [f32; 2], degrees: f32) -> [f32; 2] {
    let (s, c) = degrees.to_radians().sin_cos();
    [dir[0] * c + dir[1] * s, -dir[0] * s + dir[1] * c]
}

/// How long buff `n` is shown (`None`: until a RemoveBuff). A buff without TimeExist lasts
/// until the channel's end list removes it; on a plain press, a state that swaps the special
/// too (Mr. Good and Evil's "iamevil", ended by Good Potion; Big Bad Wolf) until removed, and
/// anything else (a combo buff, which swaps only the normal attack) for the combo window.
fn buff_duration(run: &Run, n: &Spell) -> Option<Duration> {
    if let Some(linger) = stun_linger().filter(|_| n.lists.get(ON_USE).is_some_and(|l| l.iter().any(|a| a.kind == "Stun"))) {
        return Some(linger);
    }
    match n.stats.time_exist {
        Some(cs) => Some(Duration::from_millis(10 * cs as u64)),
        None if run.length.is_zero() && swaps_special(run.caster.template, n) => None,
        None if run.length.is_zero() => Some(COMBO_WINDOW),
        None => Some(Duration::from_millis(10 * (channel_left_cs(run) as u64 + 100))),
    }
}

/// The `combat.stun_linger` tunable: how long players' stuns last, when set.
fn stun_linger() -> Option<Duration> {
    crate::gamedata::get().tunables.combat.stun_linger.map(|d| d.to_std())
}

/// Whether buff `n` puts something else on `template`'s special attack shortcut.
fn swaps_special(template: u32, n: &Spell) -> bool {
    let special = crate::collection::spells(template).get(1).copied();
    n.lists.values().flatten().any(|a| a.kind == "OverrideShortcut" && a.u("TemplateID") == special && special.is_some())
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
    fn combo_overrides() {
        // Gladius Slash: step 1's combo buff puts step 2 on the shortcut, step 2's buff step 3
        assert_eq!(override_targets(&[317_739]), vec![317_740, 317_741]);
        let learned = crate::collection::spells_to_learn(317_609);
        assert!([317_739, 317_740, 317_741].iter().all(|s| learned.contains(s)), "{learned:?}");
        // a combo buff is still shown as a buff (its only server action is the override)
        assert!(SPELLS[&317_746].lists[ON_USE].iter().all(|a| a.kind == "OverrideShortcut"));
        // Hun Warrior's Mount Up! doesn't hit, but its buff swaps both attacks
        assert!(is_ability(500_468));
        assert_eq!(override_targets(&[500_468]), vec![505_615, 505_955]);
        let learned = crate::collection::spells_to_learn(498_522);
        assert!(learned.contains(&505_615) && learned.contains(&505_955), "{learned:?}");
        // "iamevil" (no TimeExist) swaps the special too: it lasts until Good Potion; the
        // combo buff swaps only the normal attack: combo window
        assert!(swaps_special(41_894, &SPELLS[&358_710]));
        assert!(!swaps_special(317_609, &SPELLS[&317_746]));
    }

    #[test]
    fn shooting_abilities_run_here() {
        // channels that shoot every tick (were taken for charges)
        for spell in [195_930, 550_766, 596_792, 596_851, 663_939, 596_820] {
            assert!(is_channel(spell), "{spell}");
            assert!(crate::projectile::params(spell).is_none_or(|p| p.charge_items.is_empty()), "{spell}");
        }
        // shots, lobbed bombs, the Wizard's chain and the Skydiver's Blink reach their hits
        for spell in [15_351, 15_352, 576_069, 500_489, 301_766, 500_472, 33_017] {
            assert!(is_ability(spell), "{spell}");
        }
        assert!(ITEMS.contains_key(&94_115) && ITEMS.contains_key(&234_947));
        // real charges stay charges
        assert!(crate::projectile::params(31_414).is_some_and(|p| p.charge_items.len() == 4));
    }

    #[test]
    fn turn_blocking_flag() {
        // Squad and Pretzel Girl's attacks block turning (MagicCommand_c::BlocksTurning); Blaster
        // Storm doesn't
        let blocks = |spell: u32| SPELLS[&spell].stats.more_flags.unwrap_or(0) & MORE_FLAG_BLOCKS_TURNING != 0;
        assert!(blocks(195_930) && blocks(465_035) && blocks(465_036));
        assert!(!blocks(550_766));
    }

    fn test_run(caster: Id32) -> Arc<Run> {
        Arc::new(Run {
            caster: Caster {
                id: caster,
                playfield: Pf::main(9999),
                pos: Arc::new(Mutex::new([0.0; 3])),
                facing: Arc::new(Mutex::new([0.0, 1.0])),
                template: 0,
                upgrades: 0,
                level: 30,
                target: None,
                ground: None,
            },
            spell: 0,
            started: Instant::now(),
            length: Duration::ZERO,
            released: AtomicBool::new(true),
            aborted: AtomicBool::new(false),
            counted: Mutex::new(Vec::new()),
            mitigation_buffs: Mutex::new(Vec::new()),
            ends_when_hit: false,
            aim: Mutex::new(None),
            end_record: Bytes::new(),
            shot_from: Mutex::new(None),
            target: Mutex::new(None),
            locked: Mutex::new(None),
        })
    }

    /// Players' slows, stuns and knockbacks act on enemies (the client only shows them).
    #[tokio::test]
    async fn crowd_control_on_enemies() {
        let run = test_run(Id32::new(crate::territory::CHARACTER_TYPE, 1_000_000_777));
        let at = |id| Target { id: Some(id), pos: crate::npc::crowd_state(id).2 };
        // Banshee's slow (60% speed, 3 s, diminishing returns first)
        let e = crate::npc::test_enemy(11, [5.0, 0.0, 0.0], [50.0, 0.0, 0.0]);
        cast_nano(&run, 641_764, at(e), 0);
        assert!((crate::npc::crowd_state(e).0 - 0.6).abs() < 1e-4, "{:?}", crate::npc::crowd_state(e));
        // Petrifying Gaze: a stun without diminishing returns
        let s = crate::npc::test_enemy(12, [5.0, 0.0, 0.0], [50.0, 0.0, 0.0]);
        cast_nano(&run, 16_090, at(s), 0);
        assert!(crate::npc::crowd_state(s).1);
        // the Barbarian's knockaway pushes 1 m away from the caster; its knocktowards pulls
        let k = crate::npc::test_enemy(13, [5.0, 0.0, 0.0], [5.0, 0.0, 0.0]);
        cast_nano(&run, 459_329, at(k), 0);
        assert!((crate::npc::crowd_state(k).2[0] - 6.0).abs() < 1e-3, "{:?}", crate::npc::crowd_state(k));
        cast_nano(&run, 388_455, at(k), 0);
        // (5 m towards the caster, stopping at melee range)
        assert!((crate::npc::crowd_state(k).2[0] - 1.8).abs() < 1e-3, "{:?}", crate::npc::crowd_state(k));
        for id in [e, s, k] {
            crate::npc::remove_test_enemy(id);
        }
    }

    /// Petrifying Gaze counts on each enemy of its own, up to 3 (MaxBuffCounters): the third
    /// gaze stuns every enemy in it, and each later one again.
    #[tokio::test]
    async fn gaze_counts_per_enemy() {
        let run = test_run(Id32::new(crate::territory::CHARACTER_TYPE, 1_000_000_778));
        let at = |id| Target { id: Some(id), pos: crate::npc::crowd_state(id).2 };
        let a = crate::npc::test_enemy(14, [5.0, 0.0, 0.0], [50.0, 0.0, 0.0]);
        let b = crate::npc::test_enemy(15, [5.0, 0.0, 1.0], [50.0, 0.0, 0.0]);
        for _ in 0..2 {
            cast_nano(&run, 16_080, at(a), 0);
            cast_nano(&run, 16_080, at(b), 0);
        }
        assert!(!crate::npc::crowd_state(a).1 && !crate::npc::crowd_state(b).1);
        for _ in 0..3 {
            cast_nano(&run, 16_080, at(a), 0);
            cast_nano(&run, 16_080, at(b), 0);
        }
        assert_eq!((counter(a, 16_083), counter(b, 16_083)), (3, 3));
        assert!(crate::npc::crowd_state(a).1 && crate::npc::crowd_state(b).1);
        for id in [a, b] {
            forget_counters(id);
            crate::npc::remove_test_enemy(id);
        }
    }

    #[test]
    fn rotations() {
        let r = rotate([0.0, 1.0], 90.0);
        assert!((r[0] - 1.0).abs() < 1e-5 && r[1].abs() < 1e-5, "{r:?}");
        let back = rotate(r, -90.0);
        assert!(back[0].abs() < 1e-5 && (back[1] - 1.0).abs() < 1e-5);
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
pub(crate) fn area_formula_record(caster: Id32, targets: &[Id32], nano: u32, center: [f32; 3]) -> Bytes {
    lmoxide_net::iir::record("AreaFormulaIIR_t", caster, false, |w| {
        let w = targets.iter().fold(w.container_len(targets.len()), |w, t| t.write(w));
        w.u32(nano).container_len(1).f32(center[0]).f32(center[1]).f32(center[2]).u32(0)
    })
}
