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
//! the caster's power × AbilityMod, combat.rs; with an Interval, over the nano's time: `ticking`), AddBuffCounter/RemoveBuff, CorePersistentGroundEffect
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
//!
//! What a spell's server Lua (LuaScriptID, RDB 1010223) did is in its content skill's `effects`
//! (the script named in its `comment`), run with its OnUse (`lua`): heals (Healing Song,
//! Carpenter's Craft, Circle of Hearts; their area formulas pick allies: `friendly`), attacks
//! whose script casts other spells (Shooting Stars, Mythic Song, the distance dashes of the Evil
//! Dwarf, Skater Girl and Wrestling Champion, with the recast times they set: `RECHARGE`), and
//! the pets whose owner casts their damage (`pet_attack`). A ColumnDash's TemplateID2 nano is
//! cast where it ends (the dashes' landing areas).

use crate::instance::Pf;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
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
    /// What the spell's server Lua did (content skills `effects`, run with list 0: `lua`).
    #[serde(default)]
    effects: Vec<lmoxide_content::Action>,
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
/// TargetingMode of nanos for allies (`friendly`).
const TARGETING_FRIENDLY: u32 = 1;
/// TargetingMode of the Faun's Healing Song buff 667243 (the ring that heals around it): taken
/// as cast on the caster when its target is an enemy, as TARGETING_SELF (a guess: the song's
/// AttackRange is 0.5 m; docs/TUNABLES.md).
const TARGETING_SELF_OR_ALLY: u32 = 4;
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
/// Melee sectors (a minifigure ability's CoreFilteredAreaFormula around the caster, under 360°,
/// with an enemy clicked): an enemy is in reach when its edge is (characters' CharRadius 50, as
/// projectile.rs `TARGET_RADIUS`), not only its centre.
const MELEE_TARGET_RADIUS: f32 = 0.5;
/// ... and the clicked enemy is hit this much further still (the server's position of a running
/// enemy lags the clients' by a step or so).
const MELEE_LAG_SLACK: f32 = 0.5;

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
/// plain press (`cast`): it has server-side PreUse or OnUse actions, or Lua `effects` (the
/// attacks done in Lua: Shooting Stars, Mythic Song, the distance dashes).
/// Only those whose actions reach a WeaponHit (the others' damage came from actions the server
/// doesn't run, so they keep the plain hit in game.rs), a shortcut override (mode switches
/// like Mount Up!, which don't hit) or a nano with Lua `effects` (heals).
pub fn is_ability(spell: u32) -> bool {
    SPELLS.get(&spell).is_some_and(|s| {
        s.minifigure.is_some()
            && (!s.effects.is_empty()
                || ([PRE_USE, ON_USE].iter().any(|l| s.lists.get(*l).is_some_and(|l| l.iter().any(Action::on_server)))
                    && reaches(spell, &["WeaponHit", "OverrideShortcut", LUA], &mut Vec::new())))
    })
}

/// For `reaches`: a spell with Lua `effects` (the heals of Healing Song and Carpenter's Craft).
const LUA: &str = "lua";

/// Whether `spell`'s server actions (and the nanos they cast) include one of `kinds`.
fn reaches(spell: u32, kinds: &[&str], seen: &mut Vec<u32>) -> bool {
    if seen.contains(&spell) {
        return false;
    }
    seen.push(spell);
    let Some(s) = SPELLS.get(&spell) else { return false };
    (kinds.contains(&LUA) && !s.effects.is_empty()) || s.lists.values().flatten().filter(|a| a.on_server()).any(|a| action_reaches(a, kinds, seen))
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
    cast_as(caster, spell, false)
}

/// `cast`; `server_cast`: the server casts it for the caster (see `Run::server_cast`).
fn cast_as(caster: Caster, spell: u32, server_cast: bool) {
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
        server_cast,
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

/// A pet's attack `pet_spell` on `target`, when its server Lua has the pet's owner cast the
/// damage (content skills `effects` `pet.owner_casts`: Team Zombie, Gigantuan and Queen's Guard,
/// scripts 4355 / 4463 / 4581: `PetOwner:castspell(<damage spell>, spellObject.target)`). The
/// owner casts that spell on `target` with their active minifigure's power. Returns whether it
/// was such an attack (the others keep npc.rs's flat pet damage).
pub fn pet_attack(owner: Id32, pet_spell: u32, target: Id32) -> bool {
    let Some(spell) = owner_casts(pet_spell) else { return false };
    let Some(caster) = server_caster(owner, Some(target)) else { return true };
    info!(?owner, pet_spell, spell, ?target, "pet attack: the owner casts");
    cast_as(caster, spell, true);
    true
}

/// The spell `pet_spell`'s Lua has the pet's owner cast (`pet.owner_casts`).
fn owner_casts(pet_spell: u32) -> Option<u32> {
    SPELLS.get(&pet_spell)?.effects.iter().find_map(|e| match e {
        lmoxide_content::Action::Command { name, args } if name == "pet.owner_casts" => args.get("spell")?.as_u64().map(|v| v as u32),
        _ => None,
    })
}

/// `player` as the caster of a spell the server casts for them (where they are, their active
/// minifigure), aimed at `target`.
fn server_caster(player: Id32, target: Option<Id32>) -> Option<Caster> {
    let (template, upgrades, level) = crate::procs::active_of(player)?;
    let (playfield, _) = world::whereabouts(player)?;
    let pos = world::position_handles(playfield).into_iter().find(|(id, _)| *id == player)?.1;
    let facing = Arc::new(Mutex::new(world::facing_of(player).unwrap_or([0.0, 1.0])));
    Some(Caster { id: player, playfield, pos, facing, template, upgrades, level, target, ground: None })
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
#[derive(Clone)]
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
    /// Cast by the server for the caster (a pet's owner: `pet_attack`): its client knows nothing
    /// of it, so the server shows the spell's client StartEffect actions too.
    server_cast: bool,
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
        server_cast: false,
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
    RECHARGE.lock().unwrap().retain(|(p, _), _| *p != player);
}

/// The recast times the abilities' Lua set (the distance dashes' cooldown spells, Air Time's
/// `ModifyTemplateCooldown`), by (player, ability): until then the server refuses the ability
/// (game.rs). The client isn't told (no known record sets a recast time; its own spell has none).
static RECHARGE: LazyLock<Mutex<HashMap<(Id32, u32), Instant>>> = LazyLock::new(Default::default);

/// Whether `player`'s ability `spell` is still recharging (`RECHARGE`).
pub fn recharging(player: Id32, spell: u32) -> bool {
    RECHARGE.lock().unwrap().get(&(player, spell)).is_some_and(|t| *t > Instant::now())
}

/// Sets `player`'s recast time of `spell` to `d` from now.
fn set_recharge(player: Id32, spell: u32, d: Duration) {
    debug!(?player, spell, ?d, "recharge");
    RECHARGE.lock().unwrap().insert((player, spell), Instant::now() + d);
}

/// Changes the time left of `player`'s recharging `spell` by `secs` (negative: shorter).
fn add_recharge(player: Id32, spell: u32, secs: f32) {
    if let Some(t) = RECHARGE.lock().unwrap().get_mut(&(player, spell)) {
        let d = Duration::from_secs_f32(secs.abs());
        *t = if secs < 0.0 { t.checked_sub(d).unwrap_or(*t) } else { *t + d };
        debug!(?player, spell, secs, left = ?t.saturating_duration_since(Instant::now()), "recharge changed");
    }
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
        // the caster's upgrade ability, or (on the target) a buff it holds: Honey Pot renews the
        // swarm 221524 only on enemies Plan Bee already swarmed, the Tomahawk's mark 302200
        CRITERIA_HAS_ABILITY if holder == run.caster.id => crate::combat::has_ability(run.caster.template, run.caster.upgrades, y),
        CRITERIA_HAS_ABILITY => effects::has_buff(holder, y),
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
    if event == ON_USE && !s.effects.is_empty() {
        // what its server Lua did (OnUse)
        lua(run, spell, &s.effects, target, depth);
    }
    let Some(list) = s.lists.get(event) else { return };
    blast(|| {
        for a in list.iter().filter(|a| a.on_server() || CROWD_CONTROL.contains(&a.kind.as_str()) || (run.server_cast && a.kind == "StartEffect")) {
            if criteria_met(run, a, target) {
                if a.on_server() || a.kind == "StartEffect" {
                    act(run, spell, a, target, depth);
                } else {
                    crowd_control(run, spell, a, target);
                }
            }
        }
    })
}

/// Spell `spell`'s Lua `effects` (content skills `effects`) on `target`. The spells a script
/// casts are cast by the server (`server_run`).
fn lua(run: &Arc<Run>, spell: u32, effects: &[lmoxide_content::Action], target: Target, depth: u32) {
    use lmoxide_content::{Action as Do, Target as Whom};
    let caster = run.caster.id;
    let me = Target { id: Some(caster), pos: here(run) };
    let u = |args: &std::collections::BTreeMap<String, serde_json::Value>, k: &str| args.get(k).and_then(|v| v.as_u64()).map(|v| v as u32);
    let list = |args: &std::collections::BTreeMap<String, serde_json::Value>, k: &str| -> Vec<u32> {
        args.get(k).and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_u64()).map(|v| v as u32).collect()
    };
    for e in effects {
        match e {
            Do::Command { name, args } if name == "ability.ground_cast" => {
                // Shooting Stars (462): `CastSpellOnGround` at the ground target, else where the
                // target stands; the empowered spell while a buff of its line is on the caster
                let empowered = list(args, "buffs").into_iter().any(|b| effects::is_shown(caster, b));
                let Some(cast) = u(args, if empowered { "spell_with_buff" } else { "spell" }) else { continue };
                let at = Target { id: None, pos: run.caster.ground.unwrap_or(target.pos) };
                info!(?caster, spell, cast, empowered, ?at, "lua: ground cast");
                cast_nano_shown(&server_run(run), cast, at, depth + 1, false);
            }
            Do::Command { name, args } if name == "ability.remove_last_buff" => {
                // the empowered Shooting Stars (202): the newest of the caster's pixies goes
                if let Some(buff) = list(args, "buffs").into_iter().rev().find(|b| effects::is_shown(caster, *b)) {
                    remove_buff(run, caster, buff);
                }
            }
            Do::Command { name, args } if name == "ability.alternate" => {
                // Mythic Song (4621): without the marker buff the first shot and the marker,
                // with it the second. The shots are cast on the caster: they fly the way she
                // faces, at her target (`target`)
                let (Some(marker), Some(first), Some(second)) = (u(args, "marker"), u(args, "first"), u(args, "second")) else { continue };
                let child = server_run(run);
                let marked = effects::is_shown(caster, marker);
                info!(?caster, spell, shot = if marked { second } else { first }, "lua: alternate");
                if marked {
                    cast_nano_shown(&child, second, target, depth + 1, false);
                } else {
                    cast_nano_shown(&child, first, target, depth + 1, false);
                    cast_nano(&child, marker, me, depth + 1);
                }
            }
            Do::Command { name, args } if name == "ability.dash_by_distance" => {
                // Skyward Smash, Air Time, Elbow Press Drop (3821, 3822, 4622): the cooldown, then
                // the dash for the whole metres to the ground target or the target (≤ 1 ... ≥ 10)
                let to = run.caster.ground.unwrap_or(target.pos);
                let metres = (to[0] - me.pos[0]).hypot(to[2] - me.pos[2]);
                let Some(dash) = dash_for(&list(args, "dashes"), metres) else { continue };
                if let Some((ability, d)) = u(args, "cooldown_spell").and_then(recast_time) {
                    set_recharge(caster, ability, d);
                }
                if let Some(d) = args.get("cooldown").and_then(|v| serde_json::from_value::<lmoxide_content::units::Duration>(v.clone()).ok()) {
                    set_recharge(caster, run.spell, d.to_std());
                }
                // the dash spells are cast on the caster and go the way she faces: at the target
                if let Some(dir) = direction(me.pos, to) {
                    *run.caster.facing.lock().unwrap() = dir;
                    world::set_facing(caster, dir);
                }
                info!(?caster, spell, metres, dash, "lua: dash by distance");
                cast_nano(&server_run(run), dash, me, depth + 1);
            }
            Do::Command { name, args } if name == "ability.cooldown" => {
                // Air Time's landing hits (3823: `ModifyTemplateCooldown(595462, false, -1, 0, 2)`)
                if let (Some(ability), Some(secs)) = (u(args, "spell"), args.get("add").and_then(|v| v.as_f64())) {
                    add_recharge(caster, ability, secs as f32);
                }
            }
            Do::Heal { amount, target: whom } => {
                // `setstat(27, health + life × share)` (scripts 4582, 3242, 802): on whom the
                // nano is cast (each ally an area formula picked; the caster for Circle of Hearts)
                let id = match whom {
                    Whom::Owner => Some(run.caster.id),
                    _ => target.id,
                };
                if let Some(id) = id {
                    heal(id, amount.fraction() as f32, spell);
                }
            }
            // (`pet.owner_casts` is the pet's attack: `pet_attack`)
            Do::Command { name, .. } if name == "pet.owner_casts" => {}
            other => debug!(spell, ?other, depth, "lua effect not run"),
        }
    }
}

/// A run for the spells a script casts for `run`'s caster (`Run::server_cast`): the same caster
/// and ability (damage), its client not knowing them.
fn server_run(run: &Arc<Run>) -> Arc<Run> {
    Arc::new(Run {
        caster: run.caster.clone(),
        spell: run.spell,
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
        target: Mutex::new(*run.target.lock().unwrap()),
        locked: Mutex::new(*run.locked.lock().unwrap()),
        server_cast: true,
    })
}

/// The distance dash for `metres` to the target: the Lua's whole metres (`math.modf`), the
/// first dash up to 1 m, the last from 10 m (the dashes' Distance 1 ... 10).
fn dash_for(dashes: &[u32], metres: f32) -> Option<u32> {
    let whole = metres.max(0.0).trunc() as usize;
    dashes.get(whole.clamp(1, dashes.len().max(1)) - 1).copied()
}

/// A cooldown spell's recast time (its SetSpellRecastTime: the ability it names and for how long).
fn recast_time(cooldown: u32) -> Option<(u32, Duration)> {
    SPELLS.get(&cooldown)?.lists.values().flatten().filter(|a| a.kind == "SetSpellRecastTime").find_map(|a| Some((a.u("NanoTemplate")?, Duration::from_millis(10 * u64::from(a.u("Value")?)))))
}

/// Removes buff `buff` from `id`.
fn remove_buff(run: &Run, id: Id32, buff: u32) {
    let caster = run.caster.id;
    clear_counter(id, buff);
    effects::forget_buff(id, buff);
    if id == caster {
        crate::npc::set_buff_mitigation(caster, buff, None);
    }
    shortcuts::clear(id, buff);
    show(run, vec![effects::buff_remove(id, buff, caster)]);
}

/// Heals player `id` (a minifigure not smashed) by `share` of its maximum health, as the Lua's
/// `setstat` (the client's whole-number Health: rounded down).
fn heal(id: Id32, share: f32, spell: u32) {
    if !crate::npc::is_player(id) || crate::npc::player_health(id).is_none_or(|h| h == 0) {
        return;
    }
    let max = crate::npc::player_max_health(id);
    let amount = (max as f32 * share) as u32;
    if amount == 0 {
        return;
    }
    let health = crate::npc::heal_player(id, amount);
    info!(?id, spell, amount, health, max, "ability heal");
    world::send_to(id, world::Out::Records(vec![crate::game::int32_stat(id, crate::game::STAT_HEALTH, health)]));
}

thread_local! {
    /// Who the area and column formulas of each action list running now have hit with which
    /// nano (innermost list last; a list runs at once, its delayed parts in their own lists).
    static BLASTS: RefCell<Vec<HashSet<(Id32, u32)>>> = const { RefCell::new(Vec::new()) };
}

/// Runs an action list (`f`) as one blast: its formulas hit each target once with a nano (the
/// user's rule 2026-10-29: the Cross Bomb's and Captain Dreadleg's bomb's centre and four arms
/// overlap where it lands; a pulsing ground zone's every pulse is a list of its own).
pub(crate) fn blast<R>(f: impl FnOnce() -> R) -> R {
    BLASTS.with(|b| b.borrow_mut().push(HashSet::new()));
    let result = f();
    BLASTS.with(|b| b.borrow_mut().pop());
    result
}

/// Drops the `hits` the running list's formulas already hit with `nano`, and counts the rest.
pub(crate) fn first_hits<T>(hits: &mut Vec<(Id32, T)>, nano: u32) {
    BLASTS.with(|b| {
        if let Some(seen) = b.borrow_mut().last_mut() {
            hits.retain(|(id, _)| seen.insert((*id, nano)));
        }
    });
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
                match ticking(spell, a) {
                    Some((every, ticks)) => damage_over_time(caster, id, spell, damage, every, ticks),
                    None => {
                        crate::npc::hit(caster, id, spell, damage);
                    }
                }
            }
        }
        "RemoveBuff" => {
            // (a smashable takes no buffs: see cast_nano_shown)
            if let (Some(buff), Some(id)) = (a.u("TemplateID"), who.id.filter(|id| !crate::npc::is_smashable(*id))) {
                remove_buff(run, id, buff);
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
                            area_formula(&run, a, pos, false, depth);
                            tokio::time::sleep(Duration::from_millis(10 * interval as u64)).await;
                        }
                    });
                }
                _ => area_formula(run, a, centre.pos, centre.id == Some(caster), depth),
            }
        }
        "ColumnFormula" => {
            // from the caster, or from a ground point (where a bomb landed); Value1 turns it
            // by quarters (the Cross Bomb's four arms)
            let from = if target.id.is_none() && !a.on_caster() { target.pos } else { me.pos };
            let dir = rotate(*run.caster.facing.lock().unwrap(), 90.0 * a.u("Value1").unwrap_or(0) as f32);
            let length = a.f("Distance").unwrap_or(5.0);
            let width = a.f("FloatValue1").unwrap_or(1.0);
            let mut hits = crate::npc::targets_where(run.caster.playfield, |p| {
                let (dx, dz) = (p[0] - from[0], p[2] - from[2]);
                let along = dx * dir[0] + dz * dir[1];
                let across = (dx * dir[1] - dz * dir[0]).abs();
                (0.0..=length).contains(&along) && across <= width
            });
            if let Some(nano) = a.u("NanoTemplate") {
                first_hits(&mut hits, nano);
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
            // along the facing; where it ends, its TemplateID2 nano (the distance dashes' landing
            // area: Skyward Smash 595418, Air Time 595494, Elbow Press Drop 667141)
            let dash = crate::dash::Dash::column(a.f("Distance").unwrap_or(5.0), a.u("Value1").unwrap_or(0));
            let facing = *run.caster.facing.lock().unwrap();
            let (landing, run) = (a.u("TemplateID2"), run.clone());
            crate::dash::start_then(caster, run.caster.playfield, run.spell, dash, run.caster.pos.clone(), None, facing, |_| 0, move |at| {
                if let Some(nano) = landing {
                    cast_nano(&run, nano, Target { id: Some(run.caster.id), pos: at }, depth + 1);
                }
            });
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
        "StartEffect" => {
            // (only for a server cast: `run_list`) the effect package on whom it applies to
            if let (Some(id), Some(package)) = (who.id, a.u("TemplateID")) {
                show(run, vec![effects::effect_package(id, package)]);
            }
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

/// A WeaponHit with an Interval and a Duration (-1: the TimeExist of the nano holding it) hits
/// every Interval for that long: (the interval, how many hits). Plan Bee's swarm 221524: x0.25
/// every 0.5 s for 2 s (4 stings). `None`: one hit (no Interval, or nothing says how long).
fn ticking(spell: u32, a: &Action) -> Option<(Duration, u32)> {
    let interval = a.f("Interval").filter(|i| *i > 0.0)?;
    let lasts = match a.f("Duration") {
        Some(d) if d < 0.0 => SPELLS.get(&spell).and_then(|s| s.stats.time_exist).map(|cs| cs as f32),
        d => d.filter(|d| *d > 0.0),
    }?;
    let ticks = (lasts / interval).floor() as u32;
    (ticks > 0).then(|| (Duration::from_millis((10.0 * interval) as u64), ticks))
}

/// The damage over time running per (target, nano, caster), by the generation of its latest
/// cast: casting it again (Plan Bee hitting a swarmed enemy, Honey Pot) starts it over.
static DOTS: LazyLock<Mutex<HashMap<(Id32, u32, Id32), u64>>> = LazyLock::new(Default::default);
static DOT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// `caster` hits `target` for `damage` every `every`, `ticks` times (the first one `every` after
/// the cast), until it is defeated or the nano is cast on it again.
fn damage_over_time(caster: Id32, target: Id32, spell: u32, damage: u32, every: Duration, ticks: u32) {
    let key = (target, spell, caster);
    let generation = DOT_GENERATION.fetch_add(1, Ordering::Relaxed);
    DOTS.lock().unwrap().insert(key, generation);
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    rt.spawn(async move {
        for _ in 0..ticks {
            tokio::time::sleep(every).await;
            if DOTS.lock().unwrap().get(&key) != Some(&generation) {
                return;
            }
            // (None: it is gone or can't be hurt; Some(true): this sting defeated it)
            if crate::npc::hit(caster, target, spell, damage) != Some(false) {
                break;
            }
        }
        let mut dots = DOTS.lock().unwrap();
        if dots.get(&key) == Some(&generation) {
            dots.remove(&key);
        }
    });
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
    let self_cast = match n.stats.targeting_mode {
        Some(TARGETING_SELF) => true,
        Some(TARGETING_SELF_OR_ALLY) => target.id.is_some_and(|t| t != caster && !crate::npc::is_player(t)),
        _ => false,
    };
    let target = if !at_hit && self_cast && target.id.is_some() { Target { id: Some(caster), pos: here(run) } } else { target };
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
/// Only a zone that goes off once plays the nano's own effect there (the Cross Bomb's
/// explosion); pulsing ones and traps don't repeat it every pulse (not known from live).
fn ground_zone(run: &Arc<Run>, nano: u32, at: [f32; 3], delay: Duration, pulse: Duration, lifetime: Duration, depth: u32) {
    let run = run.clone();
    let once = lifetime <= pulse;
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let until = Instant::now() + lifetime;
        if once && let Some(package) = SPELLS.get(&nano).and_then(|n| n.stats.effect_package) {
            // (the Pirate Captain's Cross Bomb aoe 576066: the explosion 579335; npc_spells
            // plays enemies' the same way)
            show(&run, vec![effects::effect_package_at(run.caster.id, package, at)]);
        }
        while Instant::now() < until {
            cast_nano(&run, nano, Target { id: None, pos: at }, depth + 1);
            tokio::time::sleep(pulse).await;
        }
    });
}

/// A CoreFilteredAreaFormula's shape: within `radius` (m) and not nearer than `min`, in the
/// sector from `from` to `from + angle` degrees around the facing (`angle` 360: all round).
#[derive(Clone, Copy, Debug)]
struct Shape {
    radius: f32,
    min: f32,
    angle: f32,
    from: f32,
}

impl Shape {
    fn of(a: &Action) -> Shape {
        let angle = a.f("Angle").unwrap_or(360.0);
        Shape { radius: a.f("Radius").unwrap_or(2.0), min: a.f("Min").unwrap_or(0.0), angle, from: a.f("AngleStart").unwrap_or(-angle / 2.0) }
    }

    /// Whether `p` is in the shape around `center` facing `facing`, `extra` m further out.
    fn holds(&self, center: [f32; 3], facing: [f32; 2], p: [f32; 3], extra: f32) -> bool {
        let d = distance(p, center);
        d <= self.radius + extra && d >= self.min && (self.angle >= 360.0 || in_sector(center, facing, p, self.from, self.angle))
    }
}

/// The enemies of `candidates` a formula of `shape` around `center` hits, nearest first. With a
/// clicked enemy `melee` (a melee sector: `melee_target`) the caster first turns to it as its
/// client does (`facing` is the server's, often stale: the client turns to a clicked enemy
/// without reporting it), enemies are in reach by their edge (`MELEE_TARGET_RADIUS`), and the
/// clicked one even `MELEE_LAG_SLACK` further. Returns the facing used, and the hits.
fn shape_hits(shape: Shape, center: [f32; 3], facing: [f32; 2], candidates: Vec<(Id32, [f32; 3])>, melee: Option<(Id32, [f32; 3])>) -> ([f32; 2], Vec<(Id32, [f32; 3])>) {
    let facing = melee.and_then(|(_, p)| direction(center, p)).unwrap_or(facing);
    let extra = if melee.is_some() { MELEE_TARGET_RADIUS } else { 0.0 };
    let mut hits: Vec<_> = candidates
        .into_iter()
        .filter(|(id, p)| shape.holds(center, facing, *p, extra) || melee.is_some_and(|(t, _)| t == *id && shape.holds(center, facing, *p, extra + MELEE_LAG_SLACK)))
        .collect();
    hits.sort_by(|(_, x), (_, y)| distance(*x, center).total_cmp(&distance(*y, center)));
    (facing, hits)
}

/// The horizontal direction from `a` to `b` (none when they are on top of each other).
fn direction(a: [f32; 3], b: [f32; 3]) -> Option<[f32; 2]> {
    let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
    let d = dx.hypot(dz);
    (d > 0.01).then(|| [dx / d, dz / d])
}

/// Whether `nano` goes on allies (TargetingMode 1: the heals of Healing Song 667242 and
/// Carpenter's Craft 563178, whose formulas' NPCSelectMode 3 is the same as the attacks'; a
/// guess from the data, docs/TUNABLES.md): an area formula casting it picks players.
fn friendly(nano: u32) -> bool {
    SPELLS.get(&nano).is_some_and(|n| n.stats.targeting_mode == Some(TARGETING_FRIENDLY))
}

/// The enemy clicked for a melee sector of `shape` (a minifigure ability's formula around the
/// caster, under 360°), and where it is, if it is still there.
fn melee_target(run: &Run, shape: Shape, around_caster: bool) -> Option<(Id32, [f32; 3])> {
    if !around_caster || shape.angle >= 360.0 || !SPELLS.get(&run.spell).is_some_and(|s| s.minifigure.is_some()) {
        return None;
    }
    let clicked = (*run.target.lock().unwrap()).or(run.caster.target).filter(|t| *t != run.caster.id)?;
    crate::npc::targets_where(run.caster.playfield, |_| true).into_iter().find(|(id, _)| *id == clicked)
}

/// One pulse of a CoreFilteredAreaFormula around `center` (the caster's position when
/// `around_caster`): the enemies in the shape (nearest first, at most Max1) get its nano; the
/// players instead, the caster too, when the nano is a friendly one (`friendly`).
fn area_formula(run: &Arc<Run>, a: &'static Action, center: [f32; 3], around_caster: bool, depth: u32) {
    let caster = run.caster.id;
    let shape = Shape::of(a);
    let stale = *run.caster.facing.lock().unwrap();
    let allies = a.u("NanoTemplate").is_some_and(friendly);
    let melee = if allies { None } else { melee_target(run, shape, around_caster) };
    let reach = shape.radius + if melee.is_some() { MELEE_TARGET_RADIUS + MELEE_LAG_SLACK } else { 0.0 };
    let candidates = if allies {
        world::live_positions(run.caster.playfield).into_iter().filter(|(id, p)| distance(*p, center) <= reach && crate::npc::is_player(*id)).collect()
    } else {
        crate::npc::targets_where(run.caster.playfield, |p| distance(p, center) <= reach)
    };
    let (facing, mut hits) = shape_hits(shape, center, stale, candidates, melee);
    if let Some((target, at)) = melee {
        // the caster turned to the clicked enemy (its client did so without saying)
        *run.caster.facing.lock().unwrap() = facing;
        let stale_angle = direction(center, at).map_or(0.0, |d| (d[0].atan2(d[1]) - stale[0].atan2(stale[1])).to_degrees());
        debug!(
            spell = run.spell,
            ?target,
            distance = distance(center, at),
            radius = shape.radius,
            stale_angle = (stale_angle + 540.0).rem_euclid(360.0) - 180.0,
            ?stale,
            ?facing,
            hit = hits.iter().any(|(id, _)| *id == target),
            stale_hit = shape.holds(center, stale, at, 0.0),
            hits = hits.len(),
            "melee check"
        );
    }
    if let Some(nano) = a.u("NanoTemplate") {
        first_hits(&mut hits, nano);
    }
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

    /// Plan Bee's swarm (221524, also renewed by Honey Pot) stings every 0.5 s for its 2 s
    /// TimeExist; the Sailor's anchor 210630 for 5 s; a ticking hit with nothing saying how long
    /// (the Tarot towers) hits once.
    #[test]
    fn ticking_hits_sting_for_their_buff() {
        let tick = |spell: u32| ticking(spell, SPELLS[&spell].lists[ON_USE].iter().find(|a| a.kind == "WeaponHit").unwrap());
        assert_eq!(tick(221_524), Some((Duration::from_millis(500), 4)));
        assert_eq!(tick(210_630), Some((Duration::from_millis(500), 10)));
        let plain = SPELLS[&212_834].lists[ON_USE].iter().find(|a| a.kind == "WeaponHit").unwrap();
        assert_eq!(ticking(212_834, plain), None);
        // Honey Pot casts the swarm on enemies that already have it (criterion 91 on the target)
        let renew = SPELLS[&238_239].lists[ON_USE].iter().find(|a| a.u("Value") == Some(221_524)).unwrap();
        assert_eq!(renew.criteria, vec![[0, 0, CRITERIA_ON_TARGET], [0, 221_524, CRITERIA_HAS_ABILITY]]);
    }

    #[test]
    fn turn_blocking_flag() {
        // Squad and Pretzel Girl's attacks block turning (MagicCommand_c::BlocksTurning); Blaster
        // Storm doesn't
        let blocks = |spell: u32| SPELLS[&spell].stats.more_flags.unwrap_or(0) & MORE_FLAG_BLOCKS_TURNING != 0;
        assert!(blocks(195_930) && blocks(465_035) && blocks(465_036));
        assert!(!blocks(550_766));
    }

    /// Team Zombie's, Gigantuan's and Queen's Guard's pets attack with their Lua pet spell, which
    /// has the owner cast the damage (server Lua 4355 / 4463 / 4581); other pets don't.
    #[test]
    fn lua_pets_have_their_owner_cast_the_damage() {
        let summons = crate::skills::summons();
        for (summon, pet, damage, ability_mod) in [(641_633, 642_562, 642_556, 0.25), (641_620, 654_694, 654_693, 0.5), (663_959, 667_155, 667_153, 0.5)] {
            assert_eq!(summons[&summon]["spell"], serde_json::json!(pet), "summon {summon}");
            assert_eq!(owner_casts(pet), Some(damage));
            let hit = SPELLS[&damage].lists[ON_USE].iter().find(|a| a.kind == "WeaponHit").unwrap();
            assert_eq!(hit.f("AbilityMod"), Some(ability_mod));
            // its hit effect is a client action: shown by the server for these casts
            assert!(SPELLS[&damage].lists[ON_USE].iter().any(|a| a.kind == "StartEffect" && a.client));
        }
        assert_eq!(owner_casts(crate::npc::DEFAULT_SPELL), None);
        assert!(!pet_attack(Id32::new(crate::territory::CHARACTER_TYPE, 1_000_000_778), crate::npc::DEFAULT_SPELL, Id32::new(50_000, 1)));
    }

    /// The Lua heals (scripts 4582, 3242, 802): Healing Song 1.25% and Carpenter's Craft 2% of
    /// the ally's maximum per pulse, Circle of Hearts 10% of the caster's; never a smashed one.
    #[test]
    fn ability_heals() {
        assert!(is_ability(663_945) && is_ability(563_103), "Healing Song and Carpenter's Craft run here");
        assert!(friendly(667_242) && friendly(563_178) && !friendly(667_226) && !friendly(642_556));
        let caster = Id32::new(crate::territory::CHARACTER_TYPE, 1_000_000_779);
        let ally = Id32::new(crate::territory::CHARACTER_TYPE, 1_000_000_780);
        let run = test_run(caster);
        for id in [caster, ally] {
            crate::npc::set_active_minifigure(id, 1, 1000, 0.0);
            crate::npc::set_player_health(id, 500);
        }
        cast_nano(&run, 667_242, Target { id: Some(ally), pos: [0.0; 3] }, 0);
        assert_eq!(crate::npc::player_health(ally), Some(512));
        cast_nano(&run, 563_178, Target { id: Some(ally), pos: [0.0; 3] }, 0);
        assert_eq!(crate::npc::player_health(ally), Some(532));
        // the hearts' nano is cast on the Trendsetter herself
        cast_nano(&run, 300_712, Target { id: Some(caster), pos: [0.0; 3] }, 0);
        assert_eq!(crate::npc::player_health(caster), Some(600));
        crate::npc::set_player_health(ally, 0);
        cast_nano(&run, 667_242, Target { id: Some(ally), pos: [0.0; 3] }, 0);
        assert_eq!(crate::npc::player_health(ally), Some(0));
        for id in [caster, ally] {
            crate::npc::player_left(id);
        }
    }

    /// The attacks done in Lua run here: Shooting Stars (462), Mythic Song (4621) and the
    /// distance dashes (3821, 3822 + 3823, 4622) with their cooldowns and landing areas.
    #[test]
    fn lua_attacks() {
        for spell in [16_195, 663_944, 595_406, 595_462, 663_965] {
            assert!(is_ability(spell), "{spell}");
        }
        // the dash for the whole metres to the target
        let skater = [595_489, 595_490, 595_491, 595_492, 595_493, 595_487, 595_486, 595_484, 595_483, 595_488];
        for (metres, want) in [(0.0, 595_489), (1.9, 595_489), (2.0, 595_490), (5.5, 595_493), (9.99, 595_483), (10.0, 595_488), (25.0, 595_488)] {
            assert_eq!(dash_for(&skater, metres), Some(want), "{metres} m");
        }
        // every dash goes its whole metres and lands with its area (Skyward Smash from 4 m on
        // the Wrestling Champion's, as its script names them)
        let effects = |spell: u32| SPELLS[&spell].effects.iter().find_map(|e| match e {
            lmoxide_content::Action::Command { args, .. } => args.get("dashes").cloned(),
            _ => None,
        });
        for (ability, landing) in [(595_406, [595_418, 667_141]), (595_462, [595_494, 595_494]), (663_965, [667_141, 667_141])] {
            let dashes: Vec<u32> = serde_json::from_value(effects(ability).unwrap()).unwrap();
            for (i, dash) in dashes.iter().enumerate() {
                let a = SPELLS[dash].lists[ON_USE].iter().find(|a| a.kind == "ColumnDash").unwrap();
                assert_eq!(a.f("Distance"), Some(i as f32 + 1.0), "{dash}");
                assert_eq!(a.u("TemplateID2"), Some(landing[usize::from(i >= 3)]), "{dash}");
                assert!(SPELLS.contains_key(&a.u("TemplateID2").unwrap()));
            }
        }
        // the cooldown spells: 7.5 s on Skyward Smash and Elbow Press Drop
        assert_eq!(recast_time(595_442), Some((595_406, Duration::from_millis(7500))));
        assert_eq!(recast_time(668_775), Some((663_965, Duration::from_millis(7500))));
        // Air Time: 6.5 s, 1 s less for each enemy its landing hits (595567, script 3823)
        let p = Id32::new(crate::territory::CHARACTER_TYPE, 1_000_000_781);
        set_recharge(p, 595_462, Duration::from_secs_f32(6.5));
        add_recharge(p, 595_462, -1.0);
        let left = RECHARGE.lock().unwrap()[&(p, 595_462)].saturating_duration_since(Instant::now());
        assert!(recharging(p, 595_462) && left > Duration::from_secs(5) && left <= Duration::from_millis(5500), "{left:?}");
        forget_caster_buffs(p);
        assert!(!recharging(p, 595_462));
        // the shots' items hit with the minifigures' damage nanos
        for (item, nano) in [(50_717, 16_210), (50_728, 16_211), (667_240, 667_226), (668_570, 667_226)] {
            assert!(ITEMS[&item].actions.iter().any(|a| a.kind == "CastNano" && a.u("Value") == Some(nano)), "{item}");
            assert!(SPELLS[&nano].lists[ON_USE].iter().any(|a| a.kind == "WeaponHit"), "{nano}");
        }
    }

    /// Mythic Song's shots alternate: the first shot leaves the 0.6 s marker, the next sees it.
    #[tokio::test]
    async fn mythic_song_alternates() {
        let caster = Id32::new(crate::territory::CHARACTER_TYPE, 1_000_000_782);
        let run = test_run(caster);
        assert!(!effects::is_shown(caster, 668_568));
        run_list(&run, 663_944, ON_USE, Target { id: None, pos: [0.0, 0.0, 10.0] }, 0);
        assert!(effects::is_shown(caster, 668_568), "the first shot puts the marker on");
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(!effects::is_shown(caster, 668_568), "the marker lasts 0.6 s");
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
            server_cast: false,
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

    /// Gladius Slash's sector (Radius 2, Angle 60, AngleStart -30) with the server's facing stale.
    #[test]
    fn a_blast_hits_each_target_once_per_nano() {
        // the Cross Bomb: centre and four arms overlap where it lands
        let (a, b) = (Id32::new(50000, 1), Id32::new(50000, 2));
        blast(|| {
            let mut centre = vec![(a, ()), (b, ())];
            first_hits(&mut centre, 576065);
            assert_eq!(centre.len(), 2);
            let mut arm = vec![(a, ())];
            first_hits(&mut arm, 576065);
            assert!(arm.is_empty());
            // another nano of the same list (a slow besides the damage) still lands
            let mut slow = vec![(a, ())];
            first_hits(&mut slow, 1);
            assert_eq!(slow.len(), 1);
            // a list run inside it (a hit nano's own list) has its own blast
            blast(|| {
                let mut inner = vec![(a, ())];
                first_hits(&mut inner, 576065);
                assert_eq!(inner.len(), 1);
            });
        });
        // outside any list nothing is filtered
        let mut free = vec![(a, ()), (a, ())];
        first_hits(&mut free, 576065);
        assert_eq!(free.len(), 2);
    }

    #[test]
    fn melee_turns_to_the_clicked_enemy() {
        let slash = Shape { radius: 2.0, min: 0.0, angle: 60.0, from: -30.0 };
        let c = [0.0, 0.0, 0.0];
        let north = [0.0, 1.0];
        let (east, west, behind) = (Id32::new(50_000, 1), Id32::new(50_000, 2), Id32::new(50_000, 3));
        let enemies = vec![(east, [1.5, 0.0, 0.0]), (west, [-1.5, 0.0, 0.0]), (behind, [0.0, 0.0, -1.5])];
        // no clicked enemy: the stale facing (north) hits nobody
        let (_, hits) = shape_hits(slash, c, north, enemies.clone(), None);
        assert!(hits.is_empty());
        // the east one clicked: the caster turns east and hits it, not the others
        let (facing, hits) = shape_hits(slash, c, north, enemies.clone(), Some((east, [1.5, 0.0, 0.0])));
        assert!((facing[0] - 1.0).abs() < 1e-5 && facing[1].abs() < 1e-5, "{facing:?}");
        assert_eq!(hits.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![east]);
        // in reach by its edge (2 + 0.5), and the clicked one with the lag slack (2 + 0.5 + 0.5)
        let near = Id32::new(50_000, 4);
        let far = vec![(east, [2.9, 0.0, 0.0]), (near, [2.4, 0.0, 0.3])];
        let (_, hits) = shape_hits(slash, c, north, far.clone(), Some((east, [2.9, 0.0, 0.0])));
        assert_eq!(hits.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![near, east]);
        // ... but not beyond
        let (_, hits) = shape_hits(slash, c, north, vec![(east, [3.1, 0.0, 0.0])], Some((east, [3.1, 0.0, 0.0])));
        assert!(hits.is_empty());
        // other formulas keep the plain shape: centre within the radius, around the facing
        let (_, hits) = shape_hits(slash, c, [1.0, 0.0], far, None);
        assert!(hits.is_empty());
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
