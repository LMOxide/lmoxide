//! Server-controlled NPCs (enemies).
//!
//! LMO's spawn tables and monster templates were server-side, so spawns come from the content
//! tree (spawns.rs: each world's spawns with their NPC and object templates). An enemy is
//! an ordinary character dynel (type 50000; instances <= 16,000,000 count as NPCs) whose
//! ProtoCoreCharacterIIR has `stats.cars_group = 3` (Enemies) and a non-zero
//! `npc_stats.template_id`: the client's CARS rules then let players target and attack it while
//! its Health is non-zero. Everything an NPC does is driven from here.

use crate::instance::Pf;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use lmoxide_net::{iir, proto::proto_core_character_iir::NpcStats, types::Id32};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    game::{
        BCC_STATE_LIMBO, COMMAND_MAGIC, STAT_LIFE, create_game_object, CharSpec, STAT_BCC_STATE, STAT_HEALTH, combat_log_record, int32_stat, remote_spawn_records},
    world,
};

/// CarsGroup of hostile NPCs (target group "Enemies" in the client's CARS rules).
const CARS_ENEMIES: u32 = 3;
/// CarsGroup of friendly NPCs players can use (talk to).
const CARS_FRIENDS: u32 = 2;
/// Engine stat Flags (0) bit "custom name" (CoreCharacter::SetName sets it on a rename). An NPC
/// with a non-zero NPCTemplateInstance (npc_stats.template_id) is named by its template's TDB
/// 51000 text (CoreCharacter::GetDisplayNameEntry); with this bit, by the name sent. Only the
/// recorded spawns have real templates: the others' monsterdata ids aren't TDB 51000 keys, so
/// their target bar and nametag showed no name.
const STAT_FLAGS: u32 = 0;
const FLAG_CUSTOM_NAME: u32 = 0x8000;
/// NPC dynel instances start here (players are accounts, so keep clear of small numbers).
static NEXT_INSTANCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(900_000);
const PET_FOLLOW: f32 = 2.0;
/// Damage of a pet whose attack has no server Lua (Team Zombie, Gigantuan and Queen's Guard
/// have their owner cast theirs: actions.rs `pet_attack`).
const PET_DAMAGE: u32 = 60;
/// Pets that shoot keep this distance from their target.
const PET_SHOOT_RANGE: f32 = 8.0;
const AGGRO_RANGE: f32 = 8.0;
const LEASH_RANGE: f32 = 25.0;
/// How close an enemy walks up to its target.
const MELEE_RANGE: f32 = 1.8;
const RESPAWN: Duration = Duration::from_secs(20);
const DESTRUCTIBLE_TYPE: u32 = 51321;
/// CarsGroup of breakable objects: 3, as every recorded smashable had (the same group as
/// enemies). With 4 the client took a left-click on one for an object interaction (walked up
/// and did nothing) instead of an attack.
const CARS_DESTRUCTIBLES: u32 = 3;
const TICK: Duration = Duration::from_millis(250);
/// A playfield without players is asleep: its NPCs don't move or fight (the AI skips it). One
/// empty this long is put back as it was loaded (`unload`), so a teleport within it or a quick
/// trip out doesn't undo a fight.
const UNLOAD_AFTER: Duration = Duration::from_secs(10);
/// A walking NPC gets a new path when its goal moved this far from the current path's end.
const REPATH: f32 = 1.5;
/// Patrols (`SpawnDef::patrol`): the live server re-sent a patrolling NPC's route about every
/// 15 s from where the NPC was (live capture, in real seconds). The speed along it couldn't be
/// measured (re-sends 15 s apart on 16 m loops are ambiguous): 0.7 × its walk speed is a guess
/// that looks right in the client.
const PATROL_RESEND: Duration = Duration::from_secs(15);
const PATROL_SPEED: f32 = 0.7;
const WALK_SPEED: u32 = 1396;
/// An NPC this close to its goal stays where it is.
const ARRIVED: f32 = 0.3;
/// Movement speed stats (as game::PLAYER_SPEEDS) the original server gave the recorded
/// session's Pirates, Musketeers and Captain Dreadleg; NPC templates were server-side, so every
/// enemy without recorded values gets these.
const ENEMY_SPEEDS: &[(u32, f32)] =
    &[(1396, 2.1), (1397, 4.2), (1398, 8.4), (1399, 2.1), (1400, 8.4), (1401, 50.0), (1402, 50.0)];
/// The recorded Scallywags' and Tavern Pirates'.
const SLOW_ENEMY_SPEEDS: &[(u32, f32)] =
    &[(1396, 1.4), (1397, 1.4), (1398, 2.8), (1399, 1.4), (1400, 2.8), (1401, 50.0), (1402, 50.0)];
/// The stat the clients move characters along paths with: the recorded NPCs walked their paths
/// at it (one with 1396 = 2.1 and 1397 = 6.0 measured 4.8–6.1 m/s, Scallywags 1.3–1.4 m/s).
const RUN_SPEED: u32 = 1397;
/// Live enemies attacked about every 2.0 s (live capture, 961 hits).
const ATTACK_INTERVAL: Duration = Duration::from_millis(2000);
/// Share of player damage enemies absorb (the original server's, measured in the recorded
/// session: ~20% for normal enemies, ~40% for bosses).
const ENEMY_MITIGATION: f32 = 0.2;
const BOSS_MITIGATION: f32 = 0.4;
/// Cap on a player's total damage mitigation (upgrades plus buffs).
const MAX_MITIGATION: f32 = 0.9;
/// Ranks of enemies by their health over a regular enemy's of their level (`rank_of`): the live
/// server's bosses had 7.5–11.25 times it, its champions (Gargantuan, Banshee, T.E.S.L.A)
/// 4.25–6 (tunables.json `npc_life`); proposed bosses get 10, champions 4.
const BOSS_RANK: u32 = 7;
const CHAMPION_RANK: u32 = 4;
/// NPC-against-NPC fights (friendly fighters and enemies, world.json `npc_fights`): one hit takes
/// this share of the target's maximum health (± FIGHT_SPREAD), so a fight lasts about a minute.
const FIGHT_SHARE: f32 = 0.04;
const FIGHT_SPREAD: f32 = 0.25;
/// Friendly fighters attack enemies this close to them, but not farther than FIGHTER_LEASH from
/// their post.
const FIGHTER_AGGRO: f32 = 16.0;
const FIGHTER_LEASH: f32 = 18.0;
/// An enemy a player hit within this time keeps fighting that player (not a friendly fighter).
const THREAT_MEMORY: Duration = Duration::from_secs(6);
/// Rival enemies (world.json `npc_fights.rivals`) attack each other this close.
const RIVAL_AGGRO: f32 = 12.0;

/// Players' combat state: each minifigure has its own health.
#[derive(Default)]
struct PlayerCombat {
    active: u32,
    /// The active minifigure's maximum health and damage mitigation (combat.rs).
    max: u32,
    mitigation: f32,
    /// Mitigation from buffs active on the player (e.g. a stance's MitigationOverrideFlat), by buff.
    buff_mitigation: HashMap<u32, f32>,
    health: HashMap<u32, u32>,
    /// Each minifigure's maximum health when it was last active (inactive ones regenerate up to it).
    maxes: HashMap<u32, u32>,
}

/// When each player was last in combat: hit by an enemy, attacked by one (a dodged shot too) or
/// dealing damage to one (smashables don't count). A leaf lock: nothing else is locked under it.
static IN_COMBAT: LazyLock<Mutex<HashMap<Id32, std::time::Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn mark_in_combat(player: Id32) {
    IN_COMBAT.lock().unwrap().insert(player, std::time::Instant::now());
}

static PLAYERS: LazyLock<Mutex<HashMap<Id32, PlayerCombat>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Makes `template` the player's active minifigure and returns its health.
/// Makes `template` the player's active minifigure (with its maximum health and mitigation)
/// and returns its health.
pub fn set_active_minifigure(player: Id32, template: u32, max: u32, mitigation: f32) -> u32 {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.entry(player).or_default();
    p.active = template;
    p.max = max;
    p.mitigation = mitigation;
    p.maxes.insert(template, max);
    let h = p.health.entry(template).or_insert(max);
    *h = (*h).min(max);
    *h
}

/// Whether `player` is in combat (hit, attacked or hitting an enemy within `regen_delay`).
pub fn in_combat(player: Id32) -> bool {
    let delay = crate::gamedata::get().tunables.npc.regen_delay.to_std();
    IN_COMBAT.lock().unwrap().get(&player).is_some_and(|t| t.elapsed() < delay)
}

pub fn player_left(player: Id32) {
    PLAYERS.lock().unwrap().remove(&player);
    IN_COMBAT.lock().unwrap().remove(&player);
    crate::procs::left(player);
}

/// Sets the player's active minifigure's health (at most its maximum; `!hurt`); returns it.
pub fn set_player_health(player: Id32, health: u32) -> u32 {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.entry(player).or_default();
    let h = health.min(p.max);
    p.health.insert(p.active, h);
    h
}

/// Sets an inactive minifigure's health (with its maximum, `!hurt all`); the active one's is
/// `set_player_health`.
pub fn set_inactive_health(player: Id32, template: u32, max: u32, health: u32) {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.entry(player).or_default();
    if template != p.active {
        p.maxes.insert(template, max);
        p.health.insert(template, health.min(max));
    }
}

/// The player's active minifigure's health now.
pub fn player_health(player: Id32) -> Option<u32> {
    PLAYERS.lock().unwrap().get(&player).map(|p| p.health.get(&p.active).copied().unwrap_or(p.max))
}

/// The player's active minifigure's maximum health (0 if unknown).
pub fn player_max_health(player: Id32) -> u32 {
    PLAYERS.lock().unwrap().get(&player).map_or(0, |p| p.max)
}

/// Sets (or with `None` clears) the damage mitigation buff `buff` gives the player.
pub fn set_buff_mitigation(player: Id32, buff: u32, mitigation: Option<f32>) {
    let mut players = PLAYERS.lock().unwrap();
    let Some(p) = players.get_mut(&player) else { return };
    match mitigation {
        Some(m) => p.buff_mitigation.insert(buff, m),
        None => p.buff_mitigation.remove(&buff),
    };
}

/// Every minifigure of the player back to full health (smashed ones too): the health of those not
/// active starts full again when they are switched to. Returns the active one's health.
pub fn revive_player(player: Id32) -> u32 {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.entry(player).or_default();
    p.health.clear();
    p.health.insert(p.active, p.max);
    p.max
}

/// Heals the player's active minifigure (up to full); returns its new health.
pub fn heal_player(player: Id32, amount: u32) -> u32 {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.entry(player).or_default();
    let max = p.max;
    let h = p.health.entry(p.active).or_insert(max);
    *h = (*h + amount).min(max);
    *h
}

/// Damages the player's active minifigure; returns it and its new health (`None` if unknown).
fn damage_player(player: Id32, damage: u32) -> Option<(u32, u32)> {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.get_mut(&player)?;
    let (max, mitigation) = (p.max, (p.mitigation + p.buff_mitigation.values().sum::<f32>()).min(MAX_MITIGATION));
    let h = p.health.entry(p.active).or_insert(max);
    let dealt = ((damage as f32) * (1.0 - mitigation)).round().max(1.0) as u32;
    *h = h.saturating_sub(dealt);
    Some((p.active, *h))
}

/// Smashes the player's active minifigure (`!smashme`): a hit for its whole health, as from an
/// enemy.
pub fn smash_player(player: Id32) {
    hurt_player(player, player, default_spell(), u32::MAX / 4);
}

/// A smashed minifigure of the player (`health` 0 until it is rebuilt).
pub fn set_smashed(player: Id32, template: u32) {
    PLAYERS.lock().unwrap().entry(player).or_default().health.insert(template, 0);
}

/// A rebuilt minifigure of the player back at full health; returns that health if it is the
/// active one.
pub fn rebuild(player: Id32, template: u32) -> Option<u32> {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.get_mut(&player)?;
    p.health.remove(&template);
    (p.active == template).then(|| {
        p.health.insert(template, p.max);
        p.max
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A hostile character that chases and attacks.
    #[default]
    Enemy,
    /// A breakable object (destructible dynel, type 51321).
    Smashable,
    /// A friendly character: not attackable, stays put.
    Npc,
    /// A player's summon: follows its owner and attacks enemies near it, then vanishes.
    Pet,
    /// A friendly character that fights the enemies near its post (world.json `npc_fights`: the
    /// Jacket Officers against the ghost pirates on the Isle of Yarr's beach, the Knights of the
    /// Crown against the trolls at the Medieval Kingdom's gate). Players can't attack it; enemies
    /// can, and it respawns like them.
    Fighter,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpawnDef {
    #[serde(default)]
    pub kind: Kind,
    /// Smashables: their item template (dynel type 51321 templates in RDB 1000620).
    #[serde(default)]
    pub template: u32,
    pub name: String,
    pub monster_data: u32,
    pub level: u32,
    pub health: u32,
    pub pos: [f32; 3],
    /// Attack spell (RDB 1000621); defaults to "XX_Axe Chop".
    #[serde(default = "default_spell")]
    pub spell: u32,
    /// CARS group (3 enemies/smashables, 2 friendly NPCs players can use, 5 story NPCs);
    /// defaults by kind.
    #[serde(default)]
    pub cars_group: Option<u32>,
    /// NPC template id (npc_stats.template_id); defaults to the monsterdata.
    #[serde(default)]
    pub npc_template: Option<u32>,
    /// Conversation portrait (npc_stats.icon); none: the client's default, the DJ.
    #[serde(default)]
    pub icon: Option<u32>,
    /// Enemies that never leave their spot (Crikey the Kraken in its water): they attack players
    /// this close instead of chasing them.
    #[serde(default)]
    pub reach: Option<f32>,
    /// Never leaves its spot (its template's `ai.stationary`): like `reach`, within the reach
    /// of its attacks.
    #[serde(default)]
    pub stationary: bool,
    /// Has no attack (its template's `ai.unarmed`).
    #[serde(default)]
    pub unarmed: bool,
    /// Proposed content: the bounded area the camp was placed in (the spawn's `anchor`).
    #[serde(default)]
    pub anchor: Option<String>,
    /// A friendly NPC that also fights the enemies near it (`npc_fights.armed_npcs`).
    #[serde(default)]
    pub fights: bool,
    /// Enemies: its side among rival enemies (`npc_fights.rivals`): enemies of another
    /// side near it are its enemies too.
    #[serde(default)]
    pub faction: Option<u32>,
    /// The looping route it walks while it has nobody to fight, as the live server's patrolling
    /// NPCs did (the spawn's `patrol`, from the live capture's patrol MoveTasks).
    #[serde(default)]
    pub patrol: Vec<[f32; 3]>,
    /// Where it comes from (the spawn's `source`: live, demo, proposed, client, ...);
    /// survey mode tells guesses ("proposed") from recorded content by it.
    #[serde(default)]
    pub source: Option<String>,
    /// Its own loot table (`loot/...`, its template's `loot`), when it isn't the usual one for
    /// its kind (tunables `loot.tables`): it drops that instead, whatever its rank.
    #[serde(default)]
    pub loot: Option<String>,
    /// Its loot power level (its template's `stats.power_level`, the client's PowerLevel 499);
    /// none: level x tunables `loot.npc_power` (loot::npc_power).
    #[serde(default)]
    pub power_level: Option<u32>,
    /// Facing (radians around the vertical axis, the spawn's `rot`); shown only in
    /// survey mode.
    #[serde(default)]
    pub rot: Option<f32>,
    /// A Lost Creations level's variants it is in (none: every one; lost_creations.rs).
    #[serde(default)]
    pub variants: Vec<String>,
    /// Script waves: how many spawn (0 counts as 1), the first on `pos`, the others within
    /// `spread` m of it (the spawn's `count`, its point's `spread`; scripts.rs).
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub spread: Option<f32>,
    /// Shown only to players with all of these tags (the spawn's `requires`: a story NPC of a
    /// scene after a story event; `gated`).
    #[serde(default)]
    pub requires: Vec<u32>,
    /// An invisible NPC that owns cinematics (spawns.rs `cinematic_owners`): the dialogues'
    /// speaker (knubot.rs), nothing players click.
    #[serde(default)]
    pub cinematic: bool,
}

impl SpawnDef {
    /// Whether it fights enemies: a friendly fighter, or an armed friendly NPC.
    fn fights(&self) -> bool {
        self.kind == Kind::Fighter || (self.kind == Kind::Npc && self.fights)
    }
}

/// The attack of enemies without one ("XX_Axe Chop").
pub const DEFAULT_SPELL: u32 = 2029;

fn default_spell() -> u32 {
    DEFAULT_SPELL
}

struct Npc {
    def: SpawnDef,
    /// Pets: the summoning player, when the pet vanishes, the summon spell and what its attack
    /// shoots.
    owner: Option<(Id32, std::time::Instant)>,
    summon: Option<(u32, Option<PetProjectile>)>,
    playfield: Pf,
    health: u32,
    pos: world::Position,
    presence: Option<world::Presence>,
    last_attack: Option<std::time::Instant>,
    /// The path the clients are moving it along.
    walk: Option<Walk>,
    /// Spawned by a test scenario: doesn't respawn, and is gone at that time.
    temporary: Option<std::time::Instant>,
    /// Spawned by a script wave (scripts.rs): doesn't respawn once defeated.
    once: bool,
    /// Spawned by a test command (`!spawn`, `!test`): `!despawn` removes it.
    test_spawn: bool,
    /// Brought by a SpawnMonster action of this character's spell (`spawn_monster`).
    spawned_by: Option<Id32>,
    /// A test spawn that casts only these of its spells (`!test` scenarios' `spells`).
    only_spells: Vec<u32>,
    /// When each of its spells can be cast again (npc_spells.rs).
    cooldowns: HashMap<u32, std::time::Instant>,
    /// Casting (or stunned) until then: it stands still.
    busy_until: Option<std::time::Instant>,
    /// Stunned by a player's ability until then.
    stunned_until: Option<std::time::Instant>,
    /// A champion's element aura is on until then.
    aura_until: Option<std::time::Instant>,
    /// The player who last hit it, and when (enemies keep fighting them).
    threat: Option<(Id32, std::time::Instant)>,
    /// Walking its patrol route: the route point it is heading for, and when the route was
    /// last sent.
    patrol: Option<(usize, std::time::Instant)>,
    /// Slowed by players' abilities: (speed multiplier, until); the strongest counts.
    snares: Vec<(f32, std::time::Instant)>,
    /// The speed multiplier its current path was made with.
    speed_factor: f32,
    /// Diminishing returns: when each crowd control of a kind (802 snare, 804 stun) it got
    /// stops counting, and how much each counted one shortens the next.
    diminishing: HashMap<u32, (Vec<std::time::Instant>, f32)>,
    /// Shielded (enemy_abilities.rs): the share of damage it still takes, until then.
    shield: Option<(f32, std::time::Instant)>,
}

impl Npc {
    fn new(mut def: SpawnDef, playfield: Pf, owner: Option<(Id32, std::time::Instant)>, temporary: Option<std::time::Instant>) -> Npc {
        if owner.is_some() || temporary.is_some() {
            def.patrol.clear(); // copies of a patroller placed elsewhere (adds, test spawns)
        }
        Npc {
            health: def.health,
            pos: Arc::new(Mutex::new(def.pos)),
            def,
            playfield,
            presence: None,
            last_attack: None,
            owner,
            summon: None,
            walk: None,
            temporary,
            once: false,
            test_spawn: false,
            spawned_by: None,
            only_spells: Vec::new(),
            cooldowns: HashMap::new(),
            busy_until: None,
            stunned_until: None,
            aura_until: None,
            threat: None,
            snares: Vec::new(),
            speed_factor: 1.0,
            diminishing: HashMap::new(),
            patrol: None,
            shield: None,
        }
    }
}

/// A straight path an NPC was sent along at `speed`: the server follows it there, so the next
/// path starts where the clients show the NPC (a start more than 3 m from the NPC's position on
/// a client makes it jump there: Vehicle_t::SetSyncPos).
#[derive(Clone, Copy)]
struct Walk {
    from: [f32; 3],
    to: [f32; 3],
    since: std::time::Instant,
    speed: f32,
}

impl Walk {
    /// Where the NPC is on the path at `now`, and whether it arrived.
    fn at(&self, now: std::time::Instant) -> ([f32; 3], bool) {
        let len = distance(self.from, self.to);
        let t = if len < 0.01 { 1.0 } else { (self.speed * now.duration_since(self.since).as_secs_f32() / len).min(1.0) };
        ([0, 1, 2].map(|i| self.from[i] + (self.to[i] - self.from[i]) * t), t >= 1.0)
    }
}

/// The movement speed stats of an NPC (see `ENEMY_SPEEDS`); pets and story NPCs get the
/// players' so pets keep up with their owner.
fn speeds(def: &SpawnDef) -> &'static [(u32, f32)] {
    match def.kind {
        Kind::Enemy if matches!(def.name.as_str(), "Scallywag" | "Tavern Pirate") => SLOW_ENEMY_SPEEDS,
        Kind::Enemy | Kind::Fighter => ENEMY_SPEEDS,
        _ => crate::game::PLAYER_SPEEDS,
    }
}

fn run_speed(def: &SpawnDef) -> f32 {
    speeds(def).iter().find(|(stat, _)| *stat == RUN_SPEED).map_or(4.0, |&(_, v)| v)
}

static NPCS: LazyLock<Mutex<HashMap<Id32, Npc>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// NPCs for test commands: enemies of a rank, "boss" (`BOSS_RANK`), "champion"
/// (`CHAMPION_RANK`) or "enemy" (all), or "npc" (friendly NPCs), "fighter" (friendly
/// fighters), "patrol" (NPCs with a patrol route) or "smashable":
/// (label, playfield, home position), by playfield.
pub fn points(rank: &str) -> Vec<(String, u32, [f32; 3])> {
    let mut v: Vec<_> = residents()
        .filter(|(_, d)| {
            let r = rank_of(d);
            match rank {
                "boss" => d.kind == Kind::Enemy && r >= BOSS_RANK,
                "champion" => d.kind == Kind::Enemy && (CHAMPION_RANK..BOSS_RANK).contains(&r),
                "npc" => d.kind == Kind::Npc,
                "fighter" => d.fights(),
                "patrol" => d.patrol.len() >= 2,
                "smashable" => d.kind == Kind::Smashable,
                _ => d.kind == Kind::Enemy,
            }
        })
        .map(|(pf, d)| (format!("{} (level {}, {} health)", d.name, d.level, d.health), pf, d.pos))
        .collect();
    v.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    v
}

/// A friendly (story or usable) NPC's name and playfield.
pub fn friendly(id: Id32) -> Option<(String, Pf)> {
    NPCS.lock().unwrap().get(&id).filter(|n| n.def.kind == Kind::Npc && !n.def.cinematic).map(|n| (n.def.name.clone(), n.playfield))
}

/// An NPC's name.
pub fn name_of(id: Id32) -> Option<String> {
    NPCS.lock().unwrap().get(&id).map(|n| n.def.name.clone())
}

/// A spawned NPC named `name` on `playfield`.
pub fn find(name: &str, playfield: Pf) -> Option<Id32> {
    NPCS.lock().unwrap().iter().find(|(_, n)| n.playfield == playfield && n.def.name == name && n.presence.is_some()).map(|(id, _)| *id)
}

/// The NPC named `name` on `playfield` nearest to `at` within `radius` m (a dialogue's speaker).
pub fn nearest(name: &str, playfield: Pf, at: [f32; 3], radius: f32) -> Option<Id32> {
    let npcs = NPCS.lock().unwrap();
    npcs.iter()
        .filter(|(_, n)| n.playfield == playfield && n.def.name == name && n.presence.is_some())
        .map(|(id, n)| (*id, (n.def.pos[0] - at[0]).hypot(n.def.pos[2] - at[2])))
        .filter(|(_, d)| *d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// An NPC the content placed on cinematic marker `anchor` (its spawn's `anchor`) whose name fits
/// `alias` (a dialogue's actor: `Wizard`, `Jester` for the Court Jester or The Jester, `Knight2`),
/// on `playfield`: friendly NPCs and the bosses placed where a conversation shows them before
/// their fight (5300's Jester, 5310's Elementius, 4210's Dreadleg, 6300's T-Rex).
pub fn on_anchor(playfield: Pf, anchor: &str, alias: &str) -> Option<Id32> {
    let squash = |s: &str| s.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
    let alias = squash(alias);
    let npcs = NPCS.lock().unwrap();
    npcs.iter()
        .find(|(_, n)| {
            let name = squash(&n.def.name);
            n.playfield == playfield && n.def.kind != Kind::Smashable && n.presence.is_some() && n.def.anchor.as_deref() == Some(anchor)
                && !alias.is_empty() && (name.contains(&alias) || alias.contains(&name))
        })
        .map(|(id, _)| *id)
}

/// The NPCs of copy `playfield` a player without some of their spawn's `requires` tags doesn't
/// see (story NPCs of a scene after a story event; game.rs hides them per player).
pub fn gated(playfield: Pf, has: impl Fn(u32) -> bool) -> std::collections::HashSet<Id32> {
    NPCS.lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.playfield == playfield && n.presence.is_some() && !n.def.requires.iter().all(|t| has(*t)))
        .map(|(id, _)| *id)
        .collect()
}

/// The playfield's own enemies (not test, event or summoned ones): defeating all of them in
/// one visit clears it.
pub fn resident_enemies(playfield: Pf) -> std::collections::HashSet<Id32> {
    NPCS.lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.playfield == playfield && n.def.kind == Kind::Enemy && n.temporary.is_none() && !n.once && n.owner.is_none())
        .map(|(id, _)| *id)
        .collect()
}

/// How a resident enemy ranks by its health over a regular enemy's of its level (see
/// `BOSS_RANK`).
fn rank_of(def: &SpawnDef) -> u32 {
    def.health / regular_life(def.level)
}

/// The life of a regular (1x) NPC per level, as the live server gave it (tunables.json
/// `npc_life`, from tools/re/live_npc_life.py).
static LIFE: LazyLock<Vec<(u32, f32)>> = LazyLock::new(|| {
    let mut v: Vec<(u32, f32)> = crate::gamedata::get()
        .tunables
        .npc_life
        .base
        .iter()
        .map(|(level, life)| (level.parse().expect("tunables.json npc_life level"), *life as f32))
        .collect();
    v.sort_by_key(|p| p.0);
    v
});

/// A regular enemy's life at `level`: npc_life.json's, linear between its levels and beyond its
/// ends (as tools/re/live_npc_life.py `base_life`).
pub fn regular_life(level: u32) -> u32 {
    let pts = &*LIFE;
    let i = pts.partition_point(|p| p.0 < level).clamp(1, pts.len() - 1);
    let (a, b) = (pts[i - 1], pts[i]);
    let life = if a.0 == level { a.1 } else if b.0 == level { b.1 } else { a.1 + (b.1 - a.1) * (level as f32 - a.0 as f32) / (b.0 - a.0) as f32 };
    (life.round() as u32).max(1)
}

/// The strongest of the playfield's own bosses within `radius` of `at` (x, z); of equally strong
/// ones the nearest (the Ruined Fort's Jester in his arena, not the proposed Troll Warlord by it).
pub fn boss_near(playfield: Pf, at: [f32; 2], radius: f32) -> Option<Id32> {
    NPCS.lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.playfield == playfield && n.def.kind == Kind::Enemy && n.temporary.is_none() && !n.once && n.owner.is_none())
        .map(|(id, n)| (id, n, (n.def.pos[0] - at[0]).hypot(n.def.pos[2] - at[1])))
        .filter(|(_, n, d)| rank_of(&n.def) >= BOSS_RANK && *d <= radius)
        .max_by(|a, b| a.1.def.health.cmp(&b.1.def.health).then(b.2.total_cmp(&a.2)))
        .map(|(id, _, _)| *id)
}

/// A live NPC's health and maximum (`None` while it's dead or gone).
pub fn health_of(id: Id32) -> Option<(u32, u32)> {
    NPCS.lock().unwrap().get(&id).filter(|n| n.presence.is_some() && n.health > 0).map(|n| (n.health, n.def.health))
}

/// Heals a live NPC by `amount` (an enemy healer, enemy_abilities.rs); returns its new health.
pub fn heal_npc(id: Id32, amount: u32) -> Option<u32> {
    let health = {
        let mut npcs = NPCS.lock().unwrap();
        let n = npcs.get_mut(&id).filter(|n| n.presence.is_some() && n.health > 0)?;
        n.health = (n.health + amount).min(n.def.health);
        n.health
    };
    world::send_playfield(id, true, world::Out::Records(vec![int32_stat(id, STAT_HEALTH, health)]));
    Some(health)
}

/// Shields a live NPC until `until`: it takes `taken` of the damage (enemy_abilities.rs).
pub fn shield_npc(id: Id32, taken: f32, until: std::time::Instant) {
    if let Some(n) = NPCS.lock().unwrap().get_mut(&id) {
        n.shield = Some((taken, until));
    }
}

/// Heals a live NPC to full (an encounter reset).
pub fn restore(id: Id32) {
    let health = {
        let mut npcs = NPCS.lock().unwrap();
        let Some(n) = npcs.get_mut(&id).filter(|n| n.presence.is_some() && n.health > 0) else { return };
        n.health = n.def.health;
        n.health
    };
    world::send_playfield(id, true, world::Out::Records(vec![int32_stat(id, STAT_HEALTH, health)]));
}

/// Up to `n` different regular enemies of the playfield, nearest to `at` (x, z) first, within
/// `radius`: what an ambush or a boss's adds are made of.
pub fn regulars_near(playfield: Pf, at: [f32; 2], radius: f32, n: usize) -> Vec<SpawnDef> {
    let npcs = NPCS.lock().unwrap();
    let mut v: Vec<&SpawnDef> = npcs
        .values()
        .filter(|x| x.playfield == playfield && x.def.kind == Kind::Enemy && x.temporary.is_none() && !x.once && x.owner.is_none())
        .map(|x| &x.def)
        .filter(|d| rank_of(d) < CHAMPION_RANK && (d.pos[0] - at[0]).hypot(d.pos[2] - at[1]) <= radius)
        .collect();
    v.sort_by(|a, b| (a.pos[0] - at[0]).hypot(a.pos[2] - at[1]).total_cmp(&(b.pos[0] - at[0]).hypot(b.pos[2] - at[1])));
    let mut out: Vec<SpawnDef> = Vec::new();
    for d in v {
        if out.len() < n && !out.iter().any(|o| o.name == d.name) {
            out.push(d.clone());
        }
    }
    out
}

/// Where a live NPC is (along its walk, between AI ticks).
pub fn position(id: Id32) -> Option<[f32; 3]> {
    NPCS.lock().unwrap().get(&id).filter(|n| n.presence.is_some()).map(|n| n.now_at(std::time::Instant::now()))
}

/// Where a live NPC will be `after` from now if it keeps on along its walk (it stops at the
/// walk's end), for leading shots at it.
pub fn position_after(id: Id32, after: Duration) -> Option<[f32; 3]> {
    NPCS.lock().unwrap().get(&id).filter(|n| n.presence.is_some()).map(|n| n.now_at(std::time::Instant::now() + after))
}

impl Npc {
    /// Where it is at `now`: along its walk, else where it stands.
    fn now_at(&self, now: std::time::Instant) -> [f32; 3] {
        self.walk.map_or_else(|| *self.pos.lock().unwrap(), |w| w.at(now).0)
    }
}

/// Whether a world member is a player's character (not an NPC, nor scenery, jump pads, doors
/// or collectibles, which are world members too).
pub fn is_player(id: Id32) -> bool {
    id.kind == crate::territory::CHARACTER_TYPE && !is_npc(id)
}

pub fn is_npc(id: Id32) -> bool {
    NPCS.lock().unwrap().contains_key(&id)
}

/// Whether `id` is a smashable (a breakable object, no character).
pub fn is_smashable(id: Id32) -> bool {
    NPCS.lock().unwrap().get(&id).is_some_and(|n| n.def.kind == Kind::Smashable)
}

/// Every playfield's residents (the content's spawns, without the tutorial's per-player ones),
/// by playfield: what a copy of the world spawns (`spawn_world`).
static RESIDENTS: std::sync::OnceLock<std::collections::BTreeMap<u32, Vec<SpawnDef>>> = std::sync::OnceLock::new();

/// Every playfield's residents (playfield, spawn).
pub fn residents() -> impl Iterator<Item = (u32, &'static SpawnDef)> {
    RESIDENTS.get().into_iter().flatten().flat_map(|(&pf, defs)| defs.iter().map(move |d| (pf, d)))
}

/// Spawns the residents of a copy of their world (worlds.rs). The world editor's copies are the
/// survey set's (survey.rs spawns them).
pub fn spawn_world(pf: Pf) {
    if crate::instance::rules(pf).editor {
        return;
    }
    // a preview: the world editor's working copy's residents (preview.rs)
    if let Some(p) = crate::preview::of(pf) {
        for def in &p.residents {
            spawn_as(new_id(def), def.clone(), pf);
        }
        return;
    }
    // a Lost Creations level's copy: the variant its run picked
    let variant = crate::lost_creations::variant(pf);
    for def in RESIDENTS.get().and_then(|r| r.get(&pf.model())).into_iter().flatten() {
        if def.variants.is_empty() || variant.as_ref().is_some_and(|v| def.variants.contains(v)) {
            spawn_as(new_id(def), def.clone(), pf);
        }
    }
}

/// A spawns table's residents (spawns.rs `table`, `table_of`): the cannon's wall made strong, the
/// tutorial's scripted NPCs left out (they appear per player, tutorial.rs).
pub fn resident_list(table: HashMap<String, Vec<SpawnDef>>) -> Vec<(u32, SpawnDef)> {
    let mut all = Vec::new();
    for (playfield, defs) in table {
        let Ok(playfield) = playfield.parse::<u32>() else { continue };
        for mut def in defs {
            if let Some(health) = crate::cannon::health_override(playfield, &def.name) {
                def.health = health;
            }
            if crate::tutorial::scripted(playfield, &def.name) {
                continue;
            }
            all.push((playfield, def));
        }
    }
    all
}

/// Removes every NPC of copy `pf` (worlds.rs, a copy dropped).
pub fn despawn_world(pf: Pf) {
    let gone: Vec<Npc> = {
        let mut npcs = NPCS.lock().unwrap();
        let ids: Vec<Id32> = npcs.iter().filter(|(_, n)| n.playfield == pf).map(|(id, _)| *id).collect();
        ids.iter().filter_map(|id| npcs.remove(id)).collect()
    };
    // (their presences despawn them outside the NPCS lock)
    drop(gone);
}

/// Reads every playfield's residents (the content tree's spawns, spawns.rs), starts the world
/// editor's survey set or the play content, and the NPCs' AI. Copies of worlds spawn their
/// residents when someone first comes (worlds.rs).
pub fn start() {
    let all = resident_list(crate::spawns::table());
    let mut by_playfield: std::collections::BTreeMap<u32, Vec<SpawnDef>> = Default::default();
    // (the cinematics' owners aren't the world editor's: it shows their trigger points)
    for (pf, def) in all.iter().cloned().chain(resident_list(crate::spawns::cinematic_owners())) {
        by_playfield.entry(pf).or_default().push(def);
    }
    let _ = RESIDENTS.set(by_playfield);
    if crate::survey::enabled() {
        // what the world editor's copies show, and as what, is the survey set's business
        crate::survey::start(all);
    }
    if crate::content::enabled() {
        // the approved world content's walls, scripts and rules (play copies, also on a server
        // that runs the world editor too)
        crate::content::start();
    }
    info!(playfields = RESIDENTS.get().map_or(0, |r| r.len()), "NPCs read");
    tokio::spawn(ai());
    tokio::spawn(regenerate());
}

/// A fresh id for an NPC or smashable (`spawn_as`).
pub fn new_id(def: &SpawnDef) -> Id32 {
    let kind = if def.kind == Kind::Smashable { DESTRUCTIBLE_TYPE } else { crate::territory::CHARACTER_TYPE };
    Id32::new(kind, NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// Spawns a resident NPC or smashable under `id` (from `new_id`).
pub fn spawn_as(id: Id32, def: SpawnDef, playfield: Pf) {
    NPCS.lock().unwrap().insert(id, Npc::new(def, playfield, None, None));
    appear(id);
}

/// Spawns an NPC or smashable for a script wave (scripts.rs) under `id` (from `new_id`): it
/// fights, drops loot and counts like a resident, but doesn't respawn once defeated (it is gone
/// 3 s after) and isn't one of the playfield's own (`resident_enemies`).
pub fn spawn_once(id: Id32, def: SpawnDef, playfield: Pf) {
    NPCS.lock().unwrap().insert(id, Npc { once: true, ..Npc::new(def, playfield, None, None) });
    appear(id);
}

fn spec(def: &SpawnDef, playfield: Pf, pos: [f32; 3], health: u32) -> CharSpec<'_> {
    CharSpec {
        name: &def.name,
        playfield,
        pos,
        monster_data: def.monster_data,
        level: def.level,
        health,
        max_health: def.health,
        cars_group: def.cars_group.unwrap_or(CARS_ENEMIES),
        npc: Some(NpcStats { template_id: Some(def.npc_template.unwrap_or(def.monster_data)), icon: def.icon, ..Default::default() }),
        speeds: speeds(def).into(),
    }
}

/// Puts the NPC into the world (at full health, at home).
fn appear(id: Id32) {
    let mut npcs = NPCS.lock().unwrap();
    let Some(npc) = npcs.get_mut(&id) else { return };
    npc.health = npc.def.health;
    npc.walk = None;
    npc.patrol = None;
    *npc.pos.lock().unwrap() = npc.def.pos;
    let (def, playfield, pos) = (npc.def.clone(), npc.playfield, npc.pos.clone());
    let spawn_pos = pos.clone();
    let spawn = move || {
        let health = NPCS.lock().unwrap().get(&id).map_or(def.health, |n| n.health);
        let pos = *spawn_pos.lock().unwrap();
        let mut records = match def.kind {
            Kind::Enemy => remote_spawn_records(id, &spec(&def, playfield, pos, health)),
            Kind::Pet => remote_spawn_records(id, &CharSpec { cars_group: 0, ..spec(&def, playfield, pos, health) }),
            Kind::Npc | Kind::Fighter => remote_spawn_records(
                id,
                &CharSpec { cars_group: def.cars_group.unwrap_or(CARS_FRIENDS), ..spec(&def, playfield, pos, health) },
            ),
            Kind::Smashable => return smashable_records(id, &def, playfield, pos, health),
        };
        // survey mode names everything by its category ("[guess] Crate"), so the name must show
        let editor = crate::instance::rules(playfield).editor;
        if def.npc_template.is_none() || editor {
            // after the full update (which sets Flags from its stats)
            records.insert(2, int32_stat(id, STAT_FLAGS, FLAG_CUSTOM_NAME));
        }
        if let (Some(rot), true) = (def.rot, editor) {
            records.push(stop_record(id, pos, [rot.sin(), rot.cos()]));
        }
        records
    };
    // NPCs ignore what is sent to them
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    drop(npcs);
    let presence = world::join(id, playfield, pos, spawn, outbox);
    if let Some(npc) = NPCS.lock().unwrap().get_mut(&id) {
        npc.presence = Some(presence);
    }
}

#[derive(Deserialize)]
struct SummonDef {
    name: String,
    monster_data: u32,
    lifetime: u64,
    #[serde(default = "default_spell")]
    spell: u32,
    #[serde(default)]
    projectile: Option<PetProjectile>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct PetProjectile {
    item: u32,
    speed: f32,
}

/// Summons by spell (content/skills `summon` and the summoned NPCs, skills.rs).
static SUMMONS: LazyLock<HashMap<u32, serde_json::Value>> = LazyLock::new(crate::skills::summons);

/// What `spawn_test_enemies` can spawn (devcmd `!spawn`) and the looks of everything placed
/// in the world (`!skin`), for the control interface's test command catalog.
pub fn test_catalog() -> (Vec<lmoxide_control::Spawnable>, Vec<lmoxide_control::Look>) {
    use std::collections::BTreeMap;
    let kind_name = |k: &Kind| match k {
        Kind::Enemy => "enemy",
        Kind::Smashable => "smashable",
        Kind::Npc | Kind::Fighter => "npc",
        Kind::Pet => "pet",
    };
    let mut spawnable: BTreeMap<String, lmoxide_control::Spawnable> = BTreeMap::new();
    let mut looks: BTreeMap<u32, lmoxide_control::Look> = BTreeMap::new();
    for (playfield, d) in residents() {
        if matches!(d.kind, Kind::Enemy | Kind::Smashable) {
            let s = spawnable.entry(d.name.to_ascii_lowercase()).or_insert_with(|| lmoxide_control::Spawnable {
                name: d.name.clone(),
                kind: kind_name(&d.kind).into(),
                level: d.level,
                playfields: Vec::new(),
            });
            if !s.playfields.contains(&playfield) {
                s.playfields.push(playfield);
            }
        }
        if d.monster_data != 0 {
            let l = looks.entry(d.monster_data).or_insert_with(|| lmoxide_control::Look {
                monster_data: d.monster_data,
                names: Vec::new(),
                kind: kind_name(&d.kind).into(),
            });
            if !l.names.contains(&d.name) {
                l.names.push(d.name.clone());
            }
        }
    }
    let mut spawnable: Vec<_> = spawnable.into_values().collect();
    for s in &mut spawnable {
        s.playfields.sort_unstable();
    }
    (spawnable, looks.into_values().collect())
}

/// Test scenarios (devcmd `!test`): `count` copies of the enemy or smashable named `name` (any
/// spawn of it gives the look, level, health and attack) in a ring around `center` on
/// `playfield`, for `lifetime`. They fight (or break) like the others but don't respawn.
/// Returns how many appeared.
pub fn spawn_test_enemies(name: &str, count: u32, playfield: Pf, center: [f32; 3], lifetime: Duration) -> u32 {
    spawn_test_enemies_casting(name, count, playfield, center, lifetime, &[])
}

/// `spawn_test_enemies` that cast only `spells` (none: all of theirs), e.g. only Captain
/// Dreadleg's bomb.
pub fn spawn_test_enemies_casting(name: &str, count: u32, playfield: Pf, center: [f32; 3], lifetime: Duration, spells: &[u32]) -> u32 {
    let wanted = name.to_ascii_lowercase();
    // (of several by that name the strongest: the Pirate Grotto's Captain Dreadleg, not the
    // tutorial's)
    let Some(def) = residents()
        .filter(|(_, d)| matches!(d.kind, Kind::Enemy | Kind::Smashable) && d.name.to_ascii_lowercase() == wanted)
        .max_by_key(|(_, d)| d.health)
        .map(|(_, d)| d.clone())
    else {
        return 0;
    };
    let until = std::time::Instant::now() + lifetime;
    let mut ids = Vec::new();
    // (turned a bit per call, so several groups don't stand on each other)
    let turn = (NEXT_INSTANCE.load(std::sync::atomic::Ordering::Relaxed) % 7) as f32 * 0.45;
    for i in 0..count {
        let angle = turn + i as f32 / count.max(1) as f32 * std::f32::consts::TAU;
        let pos = [center[0] + TEST_SPAWN_RING * angle.cos(), center[1], center[2] + TEST_SPAWN_RING * angle.sin()];
        let kind = if def.kind == Kind::Smashable { DESTRUCTIBLE_TYPE } else { crate::territory::CHARACTER_TYPE };
        let id = Id32::new(kind, NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let def = SpawnDef { pos, ..def.clone() };
        let mut npc = Npc::new(def, playfield, None, Some(until));
        npc.test_spawn = true;
        npc.only_spells = spells.to_vec();
        NPCS.lock().unwrap().insert(id, npc);
        appear(id);
        ids.push(id);
    }
    tokio::spawn(async move {
        tokio::time::sleep(lifetime).await;
        let mut npcs = NPCS.lock().unwrap();
        let gone: Vec<_> = ids.iter().filter_map(|id| npcs.remove(id)).collect();
        drop(npcs);
        drop(gone); // despawns them
    });
    count
}

/// Removes the test spawns (`spawn_test_enemies`) on `playfield`, all or those within (centre,
/// radius); how many.
pub fn despawn_tests(playfield: Pf, near: Option<([f32; 3], f32)>) -> u32 {
    let mut npcs = NPCS.lock().unwrap();
    let gone: Vec<Id32> = npcs
        .iter()
        .filter(|(_, n)| n.test_spawn && n.playfield == playfield)
        .filter(|(_, n)| {
            near.is_none_or(|(c, r)| {
                let p = *n.pos.lock().unwrap();
                (p[0] - c[0]).hypot(p[2] - c[2]) <= r
            })
        })
        .map(|(id, _)| *id)
        .collect();
    let removed: Vec<Npc> = gone.iter().filter_map(|id| npcs.remove(id)).collect();
    drop(npcs);
    let n = removed.len() as u32;
    drop(removed); // despawns them
    n
}

/// Spawns `def` on `playfield` for a scripted event: it fights like the others but doesn't
/// respawn, and is gone after `lifetime` (or when `despawn`ed).
pub fn spawn_temporary(def: SpawnDef, playfield: Pf, lifetime: Duration) -> Id32 {
    let kind = if def.kind == Kind::Smashable { DESTRUCTIBLE_TYPE } else { crate::territory::CHARACTER_TYPE };
    let id = Id32::new(kind, NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let until = std::time::Instant::now() + lifetime;
    let npc = Npc::new(def, playfield, None, Some(until));
    NPCS.lock().unwrap().insert(id, npc);
    appear(id);
    tokio::spawn(async move {
        tokio::time::sleep(lifetime).await;
        despawn(id);
    });
    id
}

/// A friendly NPC (`def`) that follows `owner` and fights the enemies near it, like a pet, for
/// `lifetime` (an escort: Video Game Guy in the tutorial).
pub fn escort(owner: Id32, playfield: Pf, def: SpawnDef, lifetime: Duration) -> Id32 {
    let id = Id32::new(crate::territory::CHARACTER_TYPE, NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let def = SpawnDef { kind: Kind::Pet, cars_group: Some(0), ..def };
    let npc = Npc::new(def, playfield, Some((owner, std::time::Instant::now() + lifetime)), None);
    NPCS.lock().unwrap().insert(id, npc);
    appear(id);
    id
}

/// Removes an NPC (it disappears for everyone).
pub fn despawn(id: Id32) {
    let npc = NPCS.lock().unwrap().remove(&id);
    drop(npc); // drops the presence: despawns it
}

/// Removes the NPCs that any of `owners` summoned; how many.
pub fn despawn_summons_of(owners: &[Id32]) -> usize {
    let gone: Vec<Npc> = {
        let mut npcs = NPCS.lock().unwrap();
        let ids: Vec<Id32> = npcs.iter().filter(|(_, n)| n.owner.is_some_and(|(o, _)| owners.contains(&o))).map(|(id, _)| *id).collect();
        ids.iter().filter_map(|id| npcs.remove(id)).collect()
    };
    gone.len()
}

/// What SpawnMonster actions spawn, by the original server's template hash (content npcs
/// `spawn_hash`, spawns.rs), with their lifetimes.
static SPAWNED: LazyLock<HashMap<String, (SpawnDef, Option<Duration>)>> = LazyLock::new(crate::spawns::spawned_templates);

/// How long a spawned monster without a `lifetime` stays: an enemy's add until it is defeated
/// or this is up (assumed; docs/TUNABLES.md).
const SPAWNED_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// What spawned enemies drop: nothing, as live's summoned adds (encounter.rs).
const SPAWNED_LOOT: &str = "loot/none";

/// Whether a SpawnMonster action's template `hash` is known (content npcs `spawn_hash`).
pub fn can_spawn(hash: &str) -> bool {
    SPAWNED.contains_key(hash)
}

/// A SpawnMonster action of `caster`'s spell brings template `hash` at `at` on `playfield`: an
/// enemy (which drops nothing, starts on `foe` when it has one, and doesn't respawn) or an
/// anchor (an invisible friendly NPC orbiting shots circle: the Sad Clown's clouds), gone after
/// its template's `lifetime`. Returns it, or `None` for an unknown template.
pub fn spawn_monster(caster: Id32, playfield: Pf, hash: &str, at: [f32; 3], foe: Option<Id32>) -> Option<Id32> {
    let (def, lifetime) = SPAWNED.get(hash)?;
    let loot = if def.kind == Kind::Enemy { Some(SPAWNED_LOOT.to_string()) } else { def.loot.clone() };
    let def = SpawnDef { pos: at, loot, ..def.clone() };
    let lifetime = lifetime.unwrap_or(SPAWNED_LIFETIME);
    let id = new_id(&def);
    let now = std::time::Instant::now();
    info!(?caster, hash, name = %def.name, ?at, ?lifetime, "spawned monster");
    let threat = foe.filter(|f| def.kind == Kind::Enemy && is_player(*f)).map(|f| (f, now));
    let npc = Npc { spawned_by: Some(caster), threat, ..Npc::new(def, playfield, None, Some(now + lifetime)) };
    NPCS.lock().unwrap().insert(id, npc);
    appear(id);
    tokio::spawn(async move {
        tokio::time::sleep(lifetime).await;
        despawn(id);
    });
    Some(id)
}

/// Removes what `caster`'s SpawnMonster actions brought (a boss arena resetting); how many.
pub fn despawn_spawned_by(caster: Id32) -> usize {
    let gone: Vec<Npc> = {
        let mut npcs = NPCS.lock().unwrap();
        let ids: Vec<Id32> = npcs.iter().filter(|(_, n)| n.spawned_by == Some(caster)).map(|(id, _)| *id).collect();
        ids.iter().filter_map(|id| npcs.remove(id)).collect()
    };
    gone.len()
}

/// How long spawned monster `id` has left, if it is one.
pub fn spawned_time_left(id: Id32) -> Option<Duration> {
    NPCS.lock().unwrap().get(&id).filter(|n| n.spawned_by.is_some()).and_then(|n| n.temporary).map(|t| t.saturating_duration_since(std::time::Instant::now()))
}

/// How far from the player test enemies appear.
const TEST_SPAWN_RING: f32 = 7.0;

/// Whether `spell` summons a pet.
pub fn is_summon(spell: u32) -> bool {
    SUMMONS.contains_key(&spell)
}

/// `owner` casts summon `spell` at `pos`: a pet that follows them and fights for a while.
pub fn summon(owner: Id32, playfield: Pf, spell: u32, pos: [f32; 3]) {
    let Some(def) = SUMMONS.get(&spell).and_then(|v| serde_json::from_value::<SummonDef>(v.clone()).ok()) else {
        return;
    };
    let id = Id32::new(crate::territory::CHARACTER_TYPE, NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let spawn = SpawnDef {
                variants: Vec::new(),
            count: 1,
            spread: None,
        kind: Kind::Pet,
        template: 0,
        name: def.name,
        monster_data: def.monster_data,
        level: 1,
        health: 100,
        pos: [pos[0] + 1.0, pos[1], pos[2] + 1.0],
        spell: def.spell,
        cars_group: Some(0),
        npc_template: None,
        icon: None,
        reach: None,
        stationary: false,
        unarmed: false,
        anchor: None,
        fights: false,
        faction: None,
        patrol: Vec::new(),
        loot: None,
        power_level: None,
        source: None,
        rot: None,
        requires: Vec::new(),
        cinematic: false,
    };
    let until = std::time::Instant::now() + Duration::from_secs(def.lifetime);
    info!(?owner, spell, pet = %spawn.name, "summon");
    // one pet per summon ability: a new cast replaces the previous one
    let previous: Vec<Id32> = NPCS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.owner.is_some_and(|(o, _)| o == owner) && n.summon.is_some_and(|(s, _)| s == spell))
        .map(|(id, _)| *id)
        .collect();
    for old in previous {
        if let Some(npc) = NPCS.lock().unwrap().remove(&old) {
            drop(npc.presence);
        }
    }
    let npc = Npc { summon: Some((spell, def.projectile)), ..Npc::new(spawn, playfield, Some((owner, until)), None) };
    NPCS.lock().unwrap().insert(id, npc);
    appear(id);
}

/// `attacker` hits NPC `target` with `spell` for `damage`. Returns whether it died, or `None`
/// if `target` isn't a live NPC. A defeated NPC drops loot for the attacker.
pub fn hit(attacker: Id32, target: Id32, spell: u32, damage: u32) -> Option<bool> {
    let in_editor = NPCS.lock().unwrap().get(&target).is_some_and(|n| crate::instance::rules(n.playfield).editor);
    if in_editor {
        // nothing is hurt: attacking something selects it
        crate::survey::attacked(attacker, target);
        return None;
    }
    let mut npcs = NPCS.lock().unwrap();
    // (not `is_player`: NPCS is locked here)
    let by_npc = npcs.contains_key(&attacker);
    let by_player = attacker.kind == crate::territory::CHARACTER_TYPE && !by_npc;
    let npc = npcs.get_mut(&target)?;
    if npc.presence.is_none() || npc.health == 0 || npc.def.kind == Kind::Pet || (npc.def.kind == Kind::Npc && !npc.def.fights()) {
        return None;
    }
    // friendly fighters are hurt only by enemies
    if npc.def.fights() && !by_npc {
        return None;
    }
    if by_player {
        npc.threat = Some((attacker, std::time::Instant::now()));
        if npc.def.kind == Kind::Enemy {
            mark_in_combat(attacker);
        }
    }
    // (NPC-against-NPC damage is final: `fight_damage`)
    let mitigation = if by_npc {
        0.0
    } else if npc.def.kind == Kind::Enemy {
        if rank_of(&npc.def) >= BOSS_RANK { BOSS_MITIGATION } else { ENEMY_MITIGATION }
    } else {
        0.0
    };
    // the attacking player's upgrade procs (damage bonuses; what they do to the target runs
    // after NPCS is released)
    let damage = if by_player {
        let now = std::time::Instant::now();
        let slowed = npc.snares.iter().any(|(_, until)| *until > now) || npc.stunned_until.is_some_and(|t| t > now);
        crate::procs::outgoing(attacker, target, spell, damage, slowed && npc.def.kind == Kind::Enemy)
    } else {
        damage
    };
    // a shield (enemy_abilities.rs) lets `taken` of it through: 0% stops it all
    let taken = npc.shield.filter(|(_, until)| *until > std::time::Instant::now()).map_or(1.0, |(taken, _)| taken);
    let dealt = if taken <= 0.0 { 0 } else { (((damage as f32) * (1.0 - mitigation) * taken).round().max(1.0) as u32).min(npc.health) };
    npc.health -= dealt;
    let health = npc.health;
    info!(?attacker, npc = %npc.def.name, damage, health, "hit");
    let kind = npc.def.kind;
    let mut records = if kind == Kind::Smashable {
        // as live: Health, then the hit as an AttackResultIIR on the smashable (applied at
        // once), which plays the hit and the smash on the client right away
        vec![crate::game::stat_record(target, STAT_HEALTH, health), attack_result_record(attacker, target, spell, dealt, false)]
    } else {
        vec![combat_log_record(attacker, target, spell, dealt), int32_stat(target, STAT_HEALTH, health)]
    };
    if (kind == Kind::Enemy || npc.def.fights()) && health == 0 {
        // Limbo: the client plays the death and stops targeting it
        records.push(int32_stat(target, STAT_BCC_STATE, BCC_STATE_LIMBO));
    }
    if kind == Kind::Smashable && health == 0 {
        // as the original server did: Health 0 plays the smash, the visual change leaves the
        // debris, CarsGroup 5 makes it untargetable; the debris stays (it isn't despawned)
        records.push(visual_change_record(target, SMASHED_VISUAL_STATE, 0));
        records.push(crate::game::stat_record(target, STAT_CARS_GROUP, CARS_INACTIVE));
    }
    world::send_playfield(target, true, world::Out::Records(records));
    if health == 0 {
        // champions and bosses by their health over a regular enemy's of that level (rank_of)
        let source = match kind {
            Kind::Smashable => crate::loot::Source::Smashable,
            _ if rank_of(&npc.def) >= BOSS_RANK => crate::loot::Source::Boss,
            _ if rank_of(&npc.def) >= CHAMPION_RANK => crate::loot::Source::Champion,
            _ => crate::loot::Source::Enemy,
        };
        // a scripted shot (Pirate World's cannon, cannon.rs) drops no loot and earns nothing,
        // nor does an NPC's kill (a friendly fighter's), nor a defeated fighter
        if attacker.kind != DESTRUCTIBLE_TYPE && !by_npc && !npc.def.fights() {
            let power = crate::loot::npc_power(npc.def.level, npc.def.power_level);
            crate::loot::drop_loot(attacker, npc.playfield, *npc.pos.lock().unwrap(), target, source, npc.def.loot.as_deref(), power);
            if kind == Kind::Enemy && by_player {
                crate::procs::killed(attacker, target, spell, npc.playfield, *npc.pos.lock().unwrap());
            }
            // achievements (and XP) of the player who defeated it, and of the teammates on its
            // playfield (community::share)
            let event = crate::achievements::Event::Defeated {
                id: target,
                name: npc.def.name.clone(),
                smashable: kind == Kind::Smashable,
                boss: source == crate::loot::Source::Boss,
                champion: source == crate::loot::Source::Champion,
                level: npc.def.level,
                playfield: npc.playfield.model(),
            };
            if by_player {
                crate::community::share(attacker.instance, npc.playfield, &event);
            }
            world::send_to(attacker, world::Out::Progress(event));
        }
        // a dungeon's exit opens after its boss (teleport.rs `opens_after`; not under NPCS: the
        // door joins the world)
        if kind == Kind::Enemy
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            let (pf, name) = (npc.playfield, npc.def.name.clone());
            rt.spawn(async move { crate::teleport::defeated(pf, &name) });
        }
        remove_dead(npcs, target);
    }
    Some(health == 0)
}

/// NPC `id` destroyed itself (an exploding shroom: its spell sets its own Health to 0): it dies
/// like a defeated one, without loot, XP or achievements.
pub fn self_destruct(id: Id32) {
    let mut npcs = NPCS.lock().unwrap();
    let Some(npc) = npcs.get_mut(&id).filter(|n| n.presence.is_some() && n.health > 0) else { return };
    npc.health = 0;
    info!(npc = %npc.def.name, "self-destructed");
    world::send_playfield(id, true, world::Out::Records(vec![int32_stat(id, STAT_HEALTH, 0), int32_stat(id, STAT_BCC_STATE, BCC_STATE_LIMBO)]));
    remove_dead(npcs, id);
}

/// Dead NPC `target` is despawned (after its death plays) and respawns later.
fn remove_dead(mut npcs: std::sync::MutexGuard<'_, HashMap<Id32, Npc>>, target: Id32) {
    let Some(npc) = npcs.get_mut(&target) else { return };
    let kind = npc.def.kind;
    {
        // Dropping the presence despawns it for everyone (UpdateMessage op 1).
        let presence = npc.presence.take();
        let temporary = npc.temporary.is_some() || npc.once;
        let home = npc.playfield;
        let respawn = crate::cannon::respawn(home.model(), &npc.def.name).unwrap_or(RESPAWN);
        drop(npcs);
        tokio::spawn(async move {
            if temporary {
                tokio::time::sleep(Duration::from_secs(3)).await;
                drop(presence);
                NPCS.lock().unwrap().remove(&target);
                return;
            }
            if kind == Kind::Smashable {
                // the debris stays until it respawns
                tokio::time::sleep(respawn).await;
                drop(presence);
            } else {
                // let the death play before the body disappears
                tokio::time::sleep(Duration::from_secs(3)).await;
                drop(presence);
                tokio::time::sleep(RESPAWN).await;
            }
            // a pocket world (a dungeon) stays cleared while anyone is in it: live gave each
            // visit its own instance
            while crate::encounter::pocket(home) && world::positions(home).iter().any(|(id, _)| is_player(*id)) {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            appear(target);
        });
    }
}

/// Hits every live NPC on `playfield` within `radius` of `center`.
pub fn hit_area(attacker: Id32, playfield: Pf, center: [f32; 3], radius: f32, spell: u32, damage: u32) -> u32 {
    hit_area_with(attacker, playfield, center, radius, spell, damage, |_| ())
}

/// `hit_area`, calling `on_hit` with each NPC hit.
pub fn hit_area_with(attacker: Id32, playfield: Pf, center: [f32; 3], radius: f32, spell: u32, damage: u32, on_hit: impl Fn(Id32)) -> u32 {
    hit_where(attacker, playfield, spell, damage, |p| distance(p, center) <= radius, on_hit)
}

/// Hits every live NPC on `playfield` within `radius` of the line from `from` to `to` (a column,
/// e.g. a dash's path), calling `on_hit` with each NPC hit.
#[allow(clippy::too_many_arguments)]
pub fn hit_column_with(attacker: Id32, playfield: Pf, from: [f32; 3], to: [f32; 3], radius: f32, spell: u32, damage: u32, on_hit: impl Fn(Id32)) -> u32 {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    let len2 = (dx * dx + dz * dz).max(0.0001);
    let inside = |p: [f32; 3]| {
        let t = (((p[0] - from[0]) * dx + (p[2] - from[2]) * dz) / len2).clamp(0.0, 1.0);
        distance(p, [from[0] + dx * t, p[1], from[2] + dz * t]) <= radius
    };
    hit_where(attacker, playfield, spell, damage, inside, on_hit)
}

/// Hits the NPCs `inside`; returns how many died.
fn hit_where(attacker: Id32, playfield: Pf, spell: u32, damage: u32, inside: impl Fn([f32; 3]) -> bool, on_hit: impl Fn(Id32)) -> u32 {
    let targets = targets_where(playfield, inside);
    if crate::instance::rules(playfield).editor {
        // an area attack selects the one nearest the attacker
        let from = world::position_of(attacker).unwrap_or_default();
        if let Some((t, _)) = targets.iter().min_by(|a, b| distance(a.1, from).total_cmp(&distance(b.1, from))) {
            crate::survey::attacked(attacker, *t);
        }
        return 0;
    }
    let mut died = 0;
    for (t, _) in targets {
        if let Some(dead) = hit(attacker, t, spell, damage) {
            on_hit(t);
            died += u32::from(dead);
        }
    }
    died
}

/// The live enemies and smashables on `playfield` whose position is `inside`, with their positions.
/// The live enemies on `playfield` within `radius` of `center`.
pub fn enemies_near(playfield: Pf, center: [f32; 3], radius: f32) -> Vec<Id32> {
    let now = std::time::Instant::now();
    NPCS.lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.playfield == playfield && n.presence.is_some() && n.health > 0 && n.def.kind == Kind::Enemy)
        .filter(|(_, n)| distance(n.now_at(now), center) <= radius)
        .map(|(id, _)| *id)
        .collect()
}

pub fn targets_where(playfield: Pf, inside: impl Fn([f32; 3]) -> bool) -> Vec<(Id32, [f32; 3])> {
    let now = std::time::Instant::now();
    NPCS.lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.playfield == playfield && n.presence.is_some() && n.health > 0)
        .filter(|(_, n)| matches!(n.def.kind, Kind::Enemy | Kind::Smashable) || (crate::instance::rules(playfield).editor && matches!(n.def.kind, Kind::Npc | Kind::Fighter)))
        .map(|(id, n)| (*id, n.now_at(now)))
        .filter(|(_, p)| inside(*p))
        .collect()
}

/// Health regeneration of players' minifigures (live capture, 62 runs): every second while
/// below maximum, the share of the minifigure's own maximum health in the character record (`HealthRegenIC` 0.5% in combat, `HealthRegenOOC` 5% out of it; tunables.json
/// `npc`), rounded down as the client's (int)(Life × rate); out of combat = `regen_delay` since
/// the last hit taken, enemy attack or damage dealt to an enemy. The inactive minifigures
/// regenerate as the active one does (the user, 2026-10-30), up to their maximum when last
/// active; only the active one's health is sent (the client shows no other: live's RegenIIR sets
/// the character's Health). A defeated, 0-health minifigure stays down until rebuilt.
async fn regenerate() {
    let t = &crate::gamedata::get().tunables.npc;
    let mut tick = tokio::time::interval(t.regen_tick.to_std());
    let (delay, out_of_combat, in_combat) = (t.regen_delay.to_std(), t.regen_share.fraction() as f32, t.regen_share_in_combat.fraction() as f32);
    loop {
        tick.tick().await;
        let fighting: std::collections::HashSet<Id32> = {
            let mut combat = IN_COMBAT.lock().unwrap();
            combat.retain(|_, t| t.elapsed() < delay);
            combat.keys().copied().collect()
        };
        for (id, health) in regen_tick(&fighting, out_of_combat, in_combat) {
            world::send_to(id, world::Out::Records(vec![int32_stat(id, STAT_HEALTH, health)]));
        }
    }
}

/// One regeneration tick for every player's minifigures; returns the active ones' new health.
fn regen_tick(fighting: &std::collections::HashSet<Id32>, out_of_combat: f32, in_combat: f32) -> Vec<(Id32, u32)> {
    PLAYERS
        .lock()
        .unwrap()
        .iter_mut()
        .filter_map(|(id, p)| {
            let share = if fighting.contains(id) { in_combat } else { out_of_combat };
            let (active, active_max) = (p.active, p.max);
            p.health.entry(active).or_insert(active_max);
            let mut shown = None;
            for (&template, h) in p.health.iter_mut() {
                let max = if template == active { active_max } else { p.maxes.get(&template).copied().unwrap_or(0) };
                let gain = (max as f32 * share) as u32;
                if *h > 0 && *h < max && gain > 0 {
                    *h = (*h + gain).min(max);
                    if template == active {
                        shown = Some((*id, *h));
                    }
                }
            }
            shown
        })
        .collect()
}

/// Pets: vanish when their time is up, otherwise attack an enemy near them or follow the owner.
fn pets_tick() {
    let now = std::time::Instant::now();
    type Pet = (Id32, Id32, std::time::Instant, Pf, u32, Option<PetProjectile>);
    let pets: Vec<Pet> = NPCS
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(id, n)| {
            let (o, until) = n.owner?;
            Some((*id, o, until, n.playfield, n.def.spell, n.summon.and_then(|(_, p)| p)))
        })
        .collect();
    for (id, owner, until, playfield, spell, projectile) in pets {
        if now >= until {
            if let Some(npc) = NPCS.lock().unwrap().remove(&id) {
                drop(npc.presence); // despawns it
            }
            continue;
        }
        let Some(here) = current(id) else { continue };
        let enemy = NPCS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, n)| n.playfield == playfield && n.def.kind == Kind::Enemy && n.presence.is_some() && n.health > 0)
            .map(|(eid, n)| (*eid, *n.pos.lock().unwrap()))
            .filter(|(_, p)| distance(*p, here) < AGGRO_RANGE)
            .min_by(|(_, a), (_, b)| distance(*a, here).total_cmp(&distance(*b, here)));
        match enemy {
            Some((eid, p)) => {
                let reach = if projectile.is_some() { PET_SHOOT_RANGE } else { MELEE_RANGE + 1.0 };
                if distance(p, here) <= reach {
                    let ready = NPCS.lock().unwrap().get_mut(&id).is_some_and(|n| {
                        let ready = n.last_attack.is_none_or(|t| t.elapsed() >= ATTACK_INTERVAL);
                        if ready {
                            n.last_attack = Some(now);
                        }
                        ready
                    });
                    if ready {
                        let cast: Vec<_> = [BCC_QUEUE, 4, 10].iter().map(|&a| cast_record(id, eid, spell, a)).collect();
                        world::send_playfield(id, false, world::Out::Records(cast));
                        if let Some(pp) = projectile {
                            crate::projectile::launch_visual(id, playfield, pp.item, pp.speed, here, p);
                        }
                        // the Lua pets' owner casts the damage (actions.rs `pet_attack`)
                        if !crate::actions::pet_attack(owner, spell, eid) {
                            hit(owner, eid, spell, PET_DAMAGE);
                        }
                    }
                }
                if projectile.is_some() && distance(p, here) <= PET_SHOOT_RANGE {
                    steer(id, here, here, Some(p));
                } else {
                    steer(id, here, approach(here, p, MELEE_RANGE), Some(p));
                }
            }
            None => match world::position_of(owner) {
                Some(o) => steer(id, here, approach(here, o, PET_FOLLOW), Some(o)),
                None => steer(id, here, here, None),
            },
        }
    }
}

/// Where NPC `id` is now (following its path), also kept in its world position.
fn current(id: Id32) -> Option<[f32; 3]> {
    let mut npcs = NPCS.lock().unwrap();
    let npc = npcs.get_mut(&id)?;
    let Some(walk) = npc.walk else { return Some(*npc.pos.lock().unwrap()) };
    let (here, arrived) = walk.at(std::time::Instant::now());
    if arrived {
        npc.walk = None;
    }
    *npc.pos.lock().unwrap() = here;
    Some(here)
}

/// Moves NPC `id` (at `here`, from `current`) towards `goal`, as the original server did: one
/// path from where it is at its run speed, a new one only when the goal moved away from the
/// path's end, and a stop (facing `face`) when it is there while still walking.
fn steer(id: Id32, here: [f32; 3], goal: [f32; 3], face: Option<[f32; 3]>) {
    let now = std::time::Instant::now();
    let record = {
        let mut npcs = NPCS.lock().unwrap();
        let Some(npc) = npcs.get_mut(&id) else { return };
        // walls that block paths (triggers.rs): it stops short of them
        let goal = crate::triggers::clip_path(npc.playfield, here, goal);
        if distance(goal, here) <= ARRIVED {
            npc.walk.take().map(|w| {
                let towards = face.unwrap_or(w.to);
                let (dx, dz) = (towards[0] - here[0], towards[2] - here[2]);
                let d = dx.hypot(dz);
                let dir = if d > 0.01 { [dx / d, dz / d] } else { [0.0, 1.0] };
                *npc.pos.lock().unwrap() = here;
                stop_record(id, here, dir)
            })
        } else if npc.walk.is_none_or(|w| distance(w.to, goal) > REPATH) {
            npc.walk = Some(Walk { from: here, to: goal, since: now, speed: run_speed(&npc.def) * npc.speed_factor });
            Some(path_record(id, here, goal))
        } else {
            None
        }
    };
    if let Some(record) = record {
        world::broadcast(id, vec![record]);
    }
}

/// Walks NPC `id` (at `here`, from `current`) along its patrol route, as the live server did:
/// the route sent as a looping path, and sent again every `PATROL_RESEND` from where it is then
/// (so players who come near see it walking). The server follows it leg by leg at
/// `PATROL_SPEED`. A patrol that was interrupted (a fight) starts again at the nearest point.
fn patrol(id: Id32, here: [f32; 3]) {
    let now = std::time::Instant::now();
    let record = {
        let mut npcs = NPCS.lock().unwrap();
        let Some(npc) = npcs.get_mut(&id) else { return };
        let route = npc.def.patrol.clone();
        if route.len() < 2 {
            return;
        }
        let speed = npc.speed_factor * PATROL_SPEED * speeds(&npc.def).iter().find(|(stat, _)| *stat == WALK_SPEED).map_or(2.1, |&(_, v)| v);
        let resend = match npc.patrol {
            None => {
                let next = (0..route.len()).min_by(|&a, &b| distance(route[a], here).total_cmp(&distance(route[b], here))).unwrap_or(0);
                npc.walk = Some(Walk { from: here, to: route[next], since: now, speed });
                npc.patrol = Some((next, now));
                true
            }
            Some((mut next, sent)) => {
                // legs it finished: on to the next point, from when it got there
                let mut walk = npc.walk.unwrap_or(Walk { from: here, to: here, since: now, speed });
                while walk.at(now).1 {
                    let arrived = walk.since + Duration::from_secs_f32(distance(walk.from, walk.to) / speed);
                    let after = (next + 1) % route.len();
                    walk = Walk { from: route[next], to: route[after], since: arrived.min(now), speed };
                    next = after;
                }
                npc.walk = Some(walk);
                let resend = sent.elapsed() >= PATROL_RESEND;
                npc.patrol = Some((next, if resend { now } else { sent }));
                resend
            }
        };
        let (next, _) = npc.patrol.unwrap();
        let at = npc.walk.map_or(here, |w| w.at(now).0);
        *npc.pos.lock().unwrap() = at;
        resend.then(|| patrol_record(id, at, &route, next))
    };
    if let Some(record) = record {
        world::broadcast(id, vec![record]);
    }
}

/// `MoveTaskIIR_c` as the original server sent NPC paths: u8 0, start; ListMoveTask (4): u8 wait
/// flags 1, u8 current 0, u8 count 1; a PathMoveTask (`path_task`).
fn path_record(id: Id32, from: [f32; 3], to: [f32; 3]) -> bytes::Bytes {
    iir::record("MoveTaskIIR_c", id, false, |w| path_task(w.u8(0).f32(from[0]).f32(from[1]).f32(from[2]).u8(4).u8(1).u8(0).u8(1), &[from, to], false))
}

/// A patrol as the live server sent it (`MoveTaskIIR_c`, start `here`): ListMoveTask of two
/// paths, from `here` to route point `next`, then the route from that point on, looping.
fn patrol_record(id: Id32, here: [f32; 3], route: &[[f32; 3]], next: usize) -> bytes::Bytes {
    let looped: Vec<[f32; 3]> = route[next..].iter().chain(&route[..next]).copied().collect();
    iir::record("MoveTaskIIR_c", id, false, |w| {
        let w = w.u8(0).f32(here[0]).f32(here[1]).f32(here[2]).u8(4).u8(1).u8(0).u8(2);
        path_task(path_task(w, &[here, route[next]], false), &looped, true)
    })
}

/// PathMoveTask (1), as the original server wrote it: u8 wait flags 1, u16 path flags 0 (the
/// character's own speed), u32 n, the points, then the SplinePath: u8 loop, u32 point 0, u8 0,
/// f32 0, f32 first segment's length / 35 (as recorded, live and demo alike).
fn path_task(w: lmoxide_net::stream::Writer, points: &[[f32; 3]], looped: bool) -> lmoxide_net::stream::Writer {
    let w = w.u8(1).u8(1).u16(0).u32(points.len() as u32);
    let w = points.iter().flatten().fold(w, |w, v| w.f32(*v));
    let first = if points.len() > 1 { distance(points[0], points[1]) } else { 0.0 };
    w.u8(looped as u8).u32(0).u8(0).u32(0).f32(first / 35.0)
}

/// `StopIIR_c` as the original server stopped NPCs: position, rotation (yaw towards `dir`),
/// u8 0 (smooth correction, not a jump), u8 0.
fn stop_record(id: Id32, at: [f32; 3], dir: [f32; 2]) -> bytes::Bytes {
    let half = dir[0].atan2(dir[1]) / 2.0;
    iir::record("StopIIR_c", id, false, |w| {
        w.f32(at[0]).f32(at[1]).f32(at[2]).f32(0.0).f32(half.sin()).f32(0.0).f32(half.cos()).u8(0).u8(0)
    })
}

/// `DestructibleFullUpdateIIR_c` after the GameObject: ProtoGameCodeDynelIIR; the SimpleDynel
/// part (ProtoSimpleDynelIIR with name, template and CarsGroup 4; VisualData: resId, resType
/// (1010201 = the visual is a monsterdata, as the templates' MonsterData stat says), u32 n +
/// n × 5 u32 particles, 6 × f32 bounds, f32 scale, u32, u8); CoreDestructible (u32 health,
/// u32, f32, u32 side, ID32 locked to); Destructible (u32, f32, f32, u32, f32). Then Life/Health.
fn smashable_records(id: Id32, def: &SpawnDef, playfield: Pf, pos: [f32; 3], health: u32) -> Vec<bytes::Bytes> {
    use lmoxide_net::proto::{ProtoGameCodeDynelIir, ProtoQuat, ProtoSimpleDynelIir, ProtoVec3};
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some({
            // survey mode shows how it was turned (yaw only)
            let half = def.rot.filter(|_| crate::instance::rules(playfield).editor).unwrap_or(0.0) / 2.0;
            ProtoQuat { x: Some(0.0), y: Some(half.sin()), z: Some(0.0), w: Some(half.cos()) }
        }),
        playfield_instance: Some(playfield.key()),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(0),
        name: Some(def.name.clone()),
        template_item_id: Some(def.template),
        cars_group: Some(def.cars_group.unwrap_or(CARS_DESTRUCTIBLES)),
        is_closed: Some(true),
        ..Default::default()
    };
    // as the recorded crate (rec 1781): after the visual, bounds and scale, the Destructible part
    // u32 health, u32 maximum, f32 1.0, u32 0, ID32 0, u32 0, f32 0, f32 0, u32 0, f32 1.0. The
    // maximum matters: with 0 the client's destructible never broke (its script starts the smash
    // on Health 0, event 451, and checks the hardness).
    let record = lmoxide_net::iir::record("DestructibleFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(def.monster_data).u32(MONSTER_DATA_RDB_TYPE).u32(0);
        let w = [-0.6, 0.0, -0.6, 0.6, 1.2, 0.6].iter().fold(w, |w, v| w.f32(*v));
        // scale (n3VisualDynel asserts scale != 0), u32, u8
        let w = w.f32(1.0).u32(0).u8(0);
        let w = w.u32(health).u32(def.health).f32(1.0).u32(0);
        let w = Id32::default().write(w);
        w.u32(0).f32(0.0).f32(0.0).u32(0).f32(1.0)
    });
    vec![
        create_game_object(id, playfield, pos),
        record,
        int32_stat(id, STAT_LIFE, def.health),
        int32_stat(id, STAT_HEALTH, health),
        int32_stat(id, STAT_CARS_GROUP, def.cars_group.unwrap_or(CARS_DESTRUCTIBLES)),
    ]
}

/// RDB type of RDBMonsterData as the client registers it (resources live under 1010514).
const MONSTER_DATA_RDB_TYPE: u32 = 1010201;
const STAT_CARS_GROUP: u32 = 1050;
/// CarsGroup of objects nobody can target any more (smashed destructibles, recorded session).
const CARS_INACTIVE: u32 = 5;
/// `SimpleDynelVisualChangeIIR_c` state the original server sent when a destructible broke.
const SMASHED_VISUAL_STATE: u32 = 4;

/// `AttackResultIIR` as live reported a hit on a smashable (recorded wagon smash): identity =
/// the target, command queue id 0 (applied at once), u32 length + AttackResultIIRProto with one
/// entry {attacker, target, AttackResult {damage, damage_type 1000, result 0, levels 0}, the
/// hitting spell}. Live set killing_blow on enemy kills only, not on the smashing hit.
fn attack_result_record(attacker: Id32, target: Id32, spell: u32, damage: u32, killing_blow: bool) -> bytes::Bytes {
    use lmoxide_net::proto::{AttackResultIirProto, ProtoId32, attack_result_iir_proto::{AttackResult, AttackResultEntry}};
    use prost::Message;
    let id = |i: Id32| ProtoId32 { r#type: Some(i.kind), instance: Some(i.instance) };
    let msg = AttackResultIirProto {
        attack_result_entry: vec![AttackResultEntry {
            attacker: Some(id(attacker)),
            target: Some(id(target)),
            attack_result: Some(AttackResult {
                damage: Some(damage as f32),
                damage_type: Some(1000),
                result: Some(0),
                offensive_level: Some(0),
                defensive_level: Some(0),
                is_critical_hit: None,
                killing_blow: Some(killing_blow),
                redirect_damage_entry: vec![],
                attack_type: Some(0),
            }),
            template_id: Some(spell),
        }],
        command_queue_id: Some(0),
    };
    let body = msg.encode_to_vec();
    lmoxide_net::iir::record("AttackResultIIR", target, false, |w| w.u32(body.len() as u32).raw(&body))
}

/// `SimpleDynelVisualChangeIIR_c`: u32 state, u32 visual (0: keep), as in the recorded session.
fn visual_change_record(id: Id32, state: u32, visual: u32) -> bytes::Bytes {
    lmoxide_net::iir::record("SimpleDynelVisualChangeIIR_c", id, false, |w| w.u32(state).u32(visual))
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A point `keep` short of `to` on the way from `from`.
fn approach(from: [f32; 3], to: [f32; 3], keep: f32) -> [f32; 3] {
    let d = distance(from, to);
    if d <= keep {
        return from;
    }
    let t = (d - keep) / d;
    [from[0] + (to[0] - from[0]) * t, to[1], from[2] + (to[2] - from[2]) * t]
}

/// An enemy's power: what one of its hits (AbilityMod 1) does before the variance and the
/// player's defence, fitted to 352 clean live hits (live capture 2016; median live/predicted
/// 0.99, docs/TUNABLES.md "Enemy damage"): B(L) × R, with B(L) = max(L + 2, regular life of L /
/// 27) (L15: 49) and R by the enemy's life over a regular one's of its level: 0.3 up to 0.3×
/// (minions), 1 below 1.4×, 1.5 below 2.2×, else 2.25 (champions and bosses have 4–11× the
/// life but hit only ~2.25× as hard). The live formula itself is unknown.
pub fn enemy_damage(level: u32, health: u32) -> f32 {
    let base = (level as f32 + 2.0).max(regular_life(level) as f32 / ENEMY_DAMAGE_LIFE_DIVISOR);
    let factor = health as f32 / regular_life(level) as f32;
    let rank = match factor {
        f if f <= 0.3 => 0.3,
        f if f < 1.4 => 1.0,
        f if f < 2.2 => 1.5,
        _ => 2.25,
    };
    base * rank
}

/// `enemy_damage`'s divisor of the regular life (fitted: 11 of the 15 regular L15 templates hit
/// for 46–52 per AbilityMod, 1314 / 27 = 49).
const ENEMY_DAMAGE_LIFE_DIVISOR: f32 = 27.0;

/// One enemy hit: `power` × the spell's AbilityMod, ±10% (the client's damage variance, assumed
/// for enemies too).
pub fn enemy_hit(power: f32, ability_mod: f32) -> u32 {
    let variance = crate::gamedata::get().tunables.combat.variance.fraction() as f32;
    let roll = 1.0 + variance * (2.0 * crate::combat::random_unit() - 1.0);
    (power * ability_mod * roll).round().max(1.0) as u32
}

/// Enemy `id` hurts `player` with `spell` for `damage` (before the player's procs and
/// mitigation): the damage number and the player's new health. At 0 the client switches to the
/// next minifigure itself.
pub fn hurt_player(id: Id32, player: Id32, spell: u32, damage: u32) {
    mark_in_combat(player);
    // just after a minifigure swap the next one can't be hurt (465943)
    if crate::procs::immune(player) {
        return;
    }
    // an enemy weakened by an upgrade (Puff of Smoke, Oil Slick) does less
    let damage = ((damage as f32) * crate::procs::enemy_damage_factor(id)).round().max(1.0) as u32;
    // the player's defensive upgrade procs
    let damage = crate::procs::incoming(player, id, damage, crate::actions::is_channelling(player));
    let Some((minifigure, health)) = damage_player(player, damage) else { return };
    crate::procs::after_damage(player, health, player_max_health(player));
    let records = vec![combat_log_record(id, player, spell, damage), int32_stat(player, STAT_HEALTH, health)];
    // (through the player: a projectile can land after the enemy is gone)
    world::send_playfield(player, true, world::Out::Records(records));
    if health == 0 {
        world::send_to(player, world::Out::Smashed(minifigure));
    }
    crate::actions::caster_hit(player);
}

/// Keeps NPC `id` standing (casting, stunned) for `duration`.
pub fn set_busy(id: Id32, duration: Duration) {
    if let Some(npc) = NPCS.lock().unwrap().get_mut(&id) {
        let until = std::time::Instant::now() + duration;
        npc.busy_until = Some(npc.busy_until.map_or(until, |b| b.max(until)));
    }
}

/// Crowd control kinds (the client's diminishing return types).
pub const CC_SNARE: u32 = 802;
pub const CC_STUN: u32 = 804;

/// Whether `id` is a live enemy (players' crowd control acts on these).
pub fn is_enemy(id: Id32) -> bool {
    NPCS.lock().unwrap().get(&id).is_some_and(|n| n.def.kind == Kind::Enemy && n.health > 0)
}

/// AddDiminishingReturn on enemy `id`: a crowd control of `kind` counts for `window`, and each
/// one counting shortens the next of that kind by `step` (0.34: full, 66%, 32%, then immune).
pub fn diminish(id: Id32, kind: u32, window: Duration, step: f32) {
    if let Some(npc) = NPCS.lock().unwrap().get_mut(&id) {
        let now = std::time::Instant::now();
        let (list, s) = npc.diminishing.entry(kind).or_default();
        list.retain(|t| *t > now);
        list.push(now + window);
        *s = step;
    }
}

/// How much of a crowd control of `kind` enemy `id` takes now: 1 minus `step` for every one
/// still counting before this one (the newest is this one's own, registered just before it).
fn diminished(npc: &mut Npc, kind: u32) -> f32 {
    let now = std::time::Instant::now();
    let Some((list, step)) = npc.diminishing.get_mut(&kind) else { return 1.0 };
    list.retain(|t| *t > now);
    (1.0 - *step * list.len().saturating_sub(1) as f32).max(0.0)
}

/// `d` × `factor`, in whole milliseconds.
fn scaled(d: Duration, factor: f32) -> Duration {
    Duration::from_millis((d.as_secs_f64() * 1000.0 * f64::from(factor)).round() as u64)
}

/// The path speed multiplier of `npc` now (the strongest active snare).
fn snare_factor(npc: &mut Npc, now: std::time::Instant) -> f32 {
    npc.snares.retain(|(_, until)| *until > now);
    npc.snares.iter().map(|(m, _)| *m).fold(1.0, f32::min)
}

/// Makes `npc`'s current path go at the speed its snares allow from now on (the clients slow
/// it themselves when they get the snare's buff, AddSnareGameAction: the server keeps up).
fn apply_speed(npc: &mut Npc, now: std::time::Instant) {
    let factor = snare_factor(npc, now);
    if (factor - npc.speed_factor).abs() < f32::EPSILON {
        return;
    }
    if let Some(w) = npc.walk {
        let (here, _) = w.at(now);
        npc.walk = Some(Walk { from: here, to: w.to, since: now, speed: w.speed / npc.speed_factor.max(0.01) * factor });
    }
    npc.speed_factor = factor;
}

/// A player's ability slows enemy `id` to `multiplier` of its speed for `duration`
/// (diminished by recent snares). Returns the duration it got.
pub fn snare(id: Id32, multiplier: f32, duration: Duration) -> Option<Duration> {
    let now = std::time::Instant::now();
    let duration = {
        let mut npcs = NPCS.lock().unwrap();
        let npc = npcs.get_mut(&id).filter(|n| n.def.kind == Kind::Enemy && n.health > 0)?;
        let duration = scaled(duration, diminished(npc, CC_SNARE));
        if duration.is_zero() {
            return None;
        }
        npc.snares.push((multiplier.clamp(0.0, 1.0), now + duration));
        apply_speed(npc, now);
        duration
    };
    tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        if let Some(npc) = NPCS.lock().unwrap().get_mut(&id) {
            apply_speed(npc, std::time::Instant::now());
        }
    });
    Some(duration)
}

/// A player's ability stuns enemy `id` for `duration` (diminished by recent stuns): it stops
/// where it is and does nothing until then. Returns the duration it got.
pub fn stun(id: Id32, duration: Duration) -> Option<Duration> {
    let now = std::time::Instant::now();
    let (duration, stop) = {
        let mut npcs = NPCS.lock().unwrap();
        let npc = npcs.get_mut(&id).filter(|n| n.def.kind == Kind::Enemy && n.health > 0)?;
        let duration = scaled(duration, diminished(npc, CC_STUN));
        if duration.is_zero() {
            return None;
        }
        let until = now + duration;
        npc.busy_until = Some(npc.busy_until.map_or(until, |b| b.max(until)));
        npc.stunned_until = Some(until);
        // it stops where it is (facing the way it went)
        let stop = npc.walk.take().map(|w| {
            let (here, _) = w.at(now);
            *npc.pos.lock().unwrap() = here;
            let (dx, dz) = (w.to[0] - here[0], w.to[2] - here[2]);
            let d = dx.hypot(dz);
            stop_record(id, here, if d > 0.01 { [dx / d, dz / d] } else { [0.0, 1.0] })
        });
        (duration, stop)
    };
    if let Some(record) = stop {
        world::broadcast(id, vec![record]);
    }
    Some(duration)
}

/// A player's ability knocks enemy `id` `distance` m away from `from` (`towards`: pulls it
/// that far towards `from`, stopping short of it) at `speed` m/s; it stands still meanwhile.
/// Enemies that never leave their spot (`stationary`: cannons, pods, eggs, the Kraken) don't move.
pub fn knock(id: Id32, from: [f32; 3], distance: f32, towards: bool, speed: f32) -> bool {
    let now = std::time::Instant::now();
    let (here, to) = {
        let mut npcs = NPCS.lock().unwrap();
        let Some(npc) = npcs.get_mut(&id).filter(|n| n.def.kind == Kind::Enemy && n.health > 0 && !n.def.stationary) else { return false };
        let here = npc.walk.take().map_or_else(|| *npc.pos.lock().unwrap(), |w| w.at(now).0);
        let (dx, dz) = (here[0] - from[0], here[2] - from[2]);
        let d = dx.hypot(dz).max(0.1);
        let step = if towards { -distance.min((d - MELEE_RANGE).max(0.0)) } else { distance };
        let to = [here[0] + dx / d * step, here[1], here[2] + dz / d * step];
        *npc.pos.lock().unwrap() = to;
        let until = now + Duration::from_secs_f32(step.abs() / speed.max(1.0));
        npc.busy_until = Some(npc.busy_until.map_or(until, |b| b.max(until)));
        (here, to)
    };
    world::send_playfield(id, false, world::Out::Records(vec![crate::game::move_task_record(id, here, to)]));
    true
}

fn is_busy(id: Id32) -> bool {
    NPCS.lock().unwrap().get(&id).and_then(|n| n.busy_until).is_some_and(|t| t > std::time::Instant::now())
}

/// How far an enemy's spell reaches: its AttackRange, at least the melee distance (enemies
/// stand MELEE_RANGE away, and melee spells say 0.5–1 m).
fn reach_of(spell: u32) -> f32 {
    crate::npc_spells::range(spell).max(MELEE_RANGE + 1.0)
}

/// How close an enemy with `spells` walks up to its target: to melee range if it has a melee
/// attack, otherwise to 80% of its shortest ranged attack (kept inside the aggro range).
fn keep_distance(spells: &[u32]) -> f32 {
    let ranges: Vec<f32> = spells.iter().map(|s| crate::npc_spells::range(*s)).collect();
    if ranges.is_empty() || ranges.iter().any(|r| *r <= MELEE_RANGE + 1.0) {
        return MELEE_RANGE;
    }
    (ranges.iter().copied().fold(f32::MAX, f32::min) * 0.8).clamp(MELEE_RANGE, AGGRO_RANGE - 1.0)
}

/// The spells NPC `def` fights with: its monster's (the NPC templates' `skills`, skills.rs), else its own.
fn spells_for(def: &SpawnDef) -> Vec<u32> {
    if def.unarmed {
        return Vec::new();
    }
    crate::npc_spells::spells_of(def.monster_data).map_or_else(|| vec![def.spell], <[u32]>::to_vec)
}

/// How close players must come to an enemy that never leaves its spot: its own `reach`, else
/// (`stationary`) the longest reach of its attacks, at most the aggro range.
fn stand_reach(def: &SpawnDef) -> Option<f32> {
    def.reach.or_else(|| def.stationary.then(|| spells_for(def).into_iter().map(reach_of).fold(0.0, f32::max).min(AGGRO_RANGE)))
}

/// NPC `id` (at `here`) fights `target` (at `at`), a player or another NPC: when it isn't
/// casting and its last cast is ATTACK_INTERVAL ago, it casts the strongest (AIPowerrating) of
/// its spells that is off cooldown and reaches the target (ties: random). Returns whether it
/// cast.
fn engage(id: Id32, target: Id32, at: [f32; 3], here: [f32; 3]) -> bool {
    let now = std::time::Instant::now();
    let d = distance(here, at);
    let (spell, playfield, power, against_npc, aura) = {
        let mut npcs = NPCS.lock().unwrap();
        let Some(npc) = npcs.get_mut(&id) else { return false };
        if npc.busy_until.is_some_and(|t| t > now) || npc.last_attack.is_some_and(|t| t.elapsed() < ATTACK_INTERVAL) {
            return false;
        }
        // a heal, shield or totem first (enemy_abilities.rs), when there is one to cast
        if let Some(kit) = crate::enemy_abilities::kit(npc.def.monster_data) {
            let (playfield, faction) = (npc.playfield, npc.def.faction);
            let allies: Vec<crate::enemy_abilities::Ally> = npcs
                .iter()
                .filter(|(_, n)| n.playfield == playfield && n.def.kind == Kind::Enemy && n.presence.is_some() && n.health > 0 && n.def.faction == faction && n.owner.is_none())
                .map(|(o, n)| crate::enemy_abilities::Ally {
                    id: *o,
                    pos: n.now_at(now),
                    health: n.health,
                    max: n.def.health,
                    boss: rank_of(&n.def) >= BOSS_RANK,
                    shielded: n.shield.is_some_and(|(_, until)| until > now),
                })
                .filter(|a| distance(here, a.pos) <= 30.0)
                .collect();
            let npc = npcs.get_mut(&id).expect("looked up above");
            let heals_bosses = crate::gamedata::get().tunables.enemy_abilities.heals_bosses;
            let plan = crate::enemy_abilities::choose(kit, id, here, at, &allies, |key| npc.cooldowns.get(&key).is_none_or(|t| *t <= now), heals_bosses);
            if let Some(plan) = plan {
                let (key, cooldown) = plan.cooldown(kit);
                npc.cooldowns.insert(key, now + cooldown);
                npc.last_attack = Some(now);
                npc.busy_until = Some(now + plan.busy());
                let (playfield, power) = (npc.playfield, enemy_damage(npc.def.level, npc.def.health));
                drop(npcs);
                if is_player(target) {
                    mark_in_combat(target);
                }
                crate::enemy_abilities::run(id, playfield, power, kit, plan, target);
                return true;
            }
        }
        let npc = npcs.get_mut(&id).expect("looked up above");
        let spells = if npc.only_spells.is_empty() { spells_for(&npc.def) } else { npc.only_spells.clone() };
        let pick = spells
            .into_iter()
            .filter(|s| npc.cooldowns.get(s).is_none_or(|t| *t <= now) && d <= reach_of(*s))
            .map(|s| (s, crate::npc_spells::power(s) + crate::npc_spells::jitter()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(s, _)| s);
        let Some(spell) = pick else { return false };
        npc.last_attack = Some(now);
        npc.cooldowns.insert(spell, now + crate::npc_spells::cooldown(spell));
        let busy = crate::npc_spells::busy(spell);
        if !busy.is_zero() {
            npc.busy_until = Some(now + busy);
        }
        let against_npc = npcs.contains_key(&target);
        let npc = npcs.get_mut(&id).expect("looked up above");
        // a champion entering combat (or whose aura ran out) casts its element aura
        let aura = if against_npc { None } else { champion_aura(npc, now) };
        (spell, npc.playfield, enemy_damage(npc.def.level, npc.def.health), against_npc, aura)
    };
    if let Some(aura) = aura {
        info!(npc = ?id, aura, "champion aura");
        crate::npc_spells::cast(id, playfield, power, aura, target, at);
    }
    if against_npc {
        strike(id, target, spell, playfield);
    } else {
        // an attack keeps the player in combat even when it misses
        mark_in_combat(target);
        if crate::npc_spells::known(spell) {
            crate::npc_spells::cast(id, playfield, power, spell, target, at);
        } else {
            attack(id, target, spell, power);
        }
    }
    true
}

/// The aura champion `npc` casts now (`aura_until` set), if it is due: a champion (by health) of
/// PowerLevel (level x `power_per_level`) at least `aura_min_power_level` whose aura isn't on; the
/// zone's element's (a random one in a zone without), tunables `champions`.
fn champion_aura(npc: &mut Npc, now: std::time::Instant) -> Option<u32> {
    let c = &crate::gamedata::get().tunables.champions;
    let rank = rank_of(&npc.def);
    if npc.def.kind != Kind::Enemy || !(CHAMPION_RANK..BOSS_RANK).contains(&rank) || npc.def.level * c.power_per_level < c.aura_min_power_level {
        return None;
    }
    if npc.aura_until.is_some_and(|t| t > now) {
        return None;
    }
    let element = match crate::worlds::element(npc.playfield) {
        0 => 1 + (crate::npc_spells::jitter() * 5.0) as u32 % 5,
        e => e,
    };
    let name = lmoxide_content::model::Element::from_id(element).map(|e| format!("{e:?}").to_lowercase())?;
    let skill = crate::gamedata::get().skills.get(c.auras.get(&name)?)?;
    let spell = skill.client.as_ref()?.spell;
    let lasts = skill.client.as_ref()?.stats.get("TimeExist").and_then(|v| v.as_u64()).map_or(Duration::from_secs(59), |cs| Duration::from_millis(cs * 10));
    npc.aura_until = Some(now + lasts);
    Some(spell)
}

/// Whether NPC `id` is fighting: it attacked within `champions.out_of_combat` (NPCBreakOOCTime).
/// Its aura's pulses stop when it isn't; leaving combat also ends the aura.
pub fn fighting(id: Id32) -> bool {
    let ooc = crate::gamedata::get().tunables.champions.out_of_combat.to_std();
    let mut npcs = NPCS.lock().unwrap();
    let Some(npc) = npcs.get_mut(&id) else { return false };
    let on = npc.health > 0 && npc.last_attack.is_some_and(|t| t.elapsed() < ooc);
    if !on {
        npc.aura_until = None;
    }
    on
}

/// One NPC-against-NPC hit (a friendly fighter and an enemy): `target` loses about FIGHT_SHARE
/// of its maximum health, whatever the attacker's level, so that fights last.
fn fight_damage(target: Id32) -> u32 {
    let max = NPCS.lock().unwrap().get(&target).map_or(0, |n| n.def.health);
    let spread = 1.0 + FIGHT_SPREAD * (2.0 * crate::npc_spells::jitter() - 1.0);
    (max as f32 * FIGHT_SHARE * spread).round().max(1.0) as u32
}

/// NPC `id` casts `spell` at NPC `target`: the cast as players see an enemy's (a MagicCommand on
/// the caster aimed at the target), then at the spell's strike its projectile flies (if it
/// shoots one) and the hit lands (`hit`: damage number and the target's health). The spell's
/// other actions (areas, nanos) only ever touch players and aren't run.
fn strike(id: Id32, target: Id32, spell: u32, playfield: Pf) {
    let (_, records) = observed_cast(id, target, spell);
    world::send_playfield(id, false, world::Out::Records(records.to_vec()));
    let delay = crate::npc_spells::strike_delay(spell);
    let shot = crate::npc_spells::projectile_of(spell);
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let (Some(from), Some(to)) = (position(id), position(target)) else { return };
        if health_of(id).is_none() {
            return;
        }
        if let Some((item, speed)) = shot {
            crate::projectile::launch_visual(id, playfield, item, speed, from, to);
            tokio::time::sleep(Duration::from_secs_f32(distance(from, to) / speed.max(0.1))).await;
        }
        hit(id, target, spell, fight_damage(target));
    });
}

/// Friendly fighters: attack the nearest enemy near their post (FIGHTER_AGGRO from them, at most
/// FIGHTER_LEASH from home), otherwise walk back to their post.
fn fighters_tick(awake: &std::collections::HashSet<Pf>) {
    let fighters: Vec<(Id32, Pf, [f32; 3], f32)> = NPCS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| awake.contains(&n.playfield) && n.def.fights() && n.presence.is_some() && n.health > 0)
        .map(|(id, n)| (*id, n.playfield, n.def.pos, keep_distance(&spells_for(&n.def))))
        .collect();
    for (id, playfield, home, keep) in fighters {
        let Some(here) = current(id) else { continue };
        let enemy = NPCS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, n)| n.playfield == playfield && n.def.kind == Kind::Enemy && n.presence.is_some() && n.health > 0)
            .map(|(eid, n)| (*eid, *n.pos.lock().unwrap()))
            .filter(|(_, p)| distance(*p, here) < FIGHTER_AGGRO && distance(*p, home) < FIGHTER_LEASH)
            .min_by(|(_, a), (_, b)| distance(*a, here).total_cmp(&distance(*b, here)));
        match enemy {
            Some((eid, p)) => {
                let cast = engage(id, eid, p, here);
                if is_busy(id) {
                    steer(id, here, here, Some(p));
                    if cast {
                        face(id, here, p);
                    }
                } else {
                    steer(id, here, approach(here, p, keep), Some(p));
                }
            }
            None if is_busy(id) => steer(id, here, here, None),
            None => steer(id, here, home, None),
        }
    }
}

/// The live friendly fighters on a playfield, with their positions.
fn fighters_on(playfield: Pf) -> Vec<(Id32, [f32; 3])> {
    NPCS.lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.playfield == playfield && n.def.fights() && n.presence.is_some() && n.health > 0)
        .map(|(id, n)| (*id, *n.pos.lock().unwrap()))
        .collect()
}

/// A plain hit with a spell npc_spells.rs doesn't know (the default XX_Axe Chop): the cast (a
/// MagicCommand created on the NPC: BCC actions 16 queue, 4 success, 10 can-execute) and the
/// damage at once.
fn attack(id: Id32, player: Id32, spell: u32, power: f32) {
    let records: Vec<_> = [BCC_QUEUE, 4, 10].iter().map(|&a| cast_record(id, player, spell, a)).collect();
    world::send_playfield(id, false, world::Out::Records(records));
    hurt_player(id, player, spell, enemy_hit(power, 1.0));
}

/// Turn an enemy that stays put towards `target`.
fn face(id: Id32, here: [f32; 3], target: [f32; 3]) {
    let (dx, dz) = (target[0] - here[0], target[2] - here[2]);
    let d = dx.hypot(dz);
    if d > 0.01 {
        world::broadcast(id, vec![stop_record(id, here, [dx / d, dz / d])]);
    }
}

/// BCC action that queues a server-created command on the destination character.
const BCC_QUEUE: u8 = 16;
static NEXT_SERVER_COMMAND: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// A BehaviorControlCenterIIR_c carrying a MagicCommand cast by `caster` at `target`. The data
/// part copies what the client sends for its own casts, with the spell swapped in.
fn cast_record(caster: Id32, target: Id32, spell: u32, action: u8) -> bytes::Bytes {
    use std::sync::atomic::Ordering;
    let server_id = if action == BCC_QUEUE {
        NEXT_SERVER_COMMAND.fetch_add(1, Ordering::Relaxed)
    } else {
        NEXT_SERVER_COMMAND.load(Ordering::Relaxed) - 1
    };
    cast_record_with_id(caster, target, spell, action, server_id)
}

/// What the other players see of a player's cast of `spell` at `target`: the command queued,
/// succeeded and allowed to execute (BCC actions 16, 4, 10), as the original server sent NPC
/// casts to everyone; the caster's own client has its own command.
pub fn observed_cast(caster: Id32, target: Id32, spell: u32) -> (u32, [bytes::Bytes; 3]) {
    let id = NEXT_SERVER_COMMAND.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    (id, [BCC_QUEUE, 4, 10].map(|a| cast_record_with_id(caster, target, spell, a, id)))
}

/// Ends an observed cast (`observed_cast`'s command `id`) for the other players: BCC action 20
/// (finish channel: UpdateCommandStatus), as the caster's own release, then 7 (abort one:
/// CoreCommandHolder_c::AbortOneCommand, found by the server id), which takes the command off
/// the character; else a held charge keeps animating there until its own time runs out.
pub fn observed_end(caster: Id32, target: Id32, spell: u32, id: u32) -> Vec<bytes::Bytes> {
    vec![cast_record_with_id(caster, target, spell, 20, id), cast_record_with_id(caster, target, spell, 7, id)]
}

/// BCC action that aborts a command (CoreCommandHolder_c::AbortOneCommand, by server id): the
/// original server ended Captain Dreadleg's spin with it.
pub const BCC_ABORT: u8 = 7;

/// A BehaviorControlCenterIIR_c for the MagicCommand with server id `server_id` (see
/// `observed_cast`).
pub fn command_record(caster: Id32, target: Id32, spell: u32, action: u8, server_id: u32) -> bytes::Bytes {
    cast_record_with_id(caster, target, spell, action, server_id)
}

fn cast_record_with_id(caster: Id32, target: Id32, spell: u32, action: u8, server_id: u32) -> bytes::Bytes {
    let msg = lmoxide_net::proto::CommandMessage {
        bcc_action_id: Some(605),
        commandqueue_server_id: Some(server_id),
        flags: Some(0),
        execution_status: Some(0),
        target: Some(lmoxide_net::proto::ProtoId32 { r#type: Some(target.kind), instance: Some(target.instance) }),
        ..Default::default()
    };
    lmoxide_net::iir::record("BehaviorControlCenterIIR_c", caster, false, |w| {
        // MagicCommand_c::ReadSubClass: u64 spell, u8 posFlag (1: no position), u32, u32,
        // u32 activation, u8 canExecute, 3 x u8, ID32, u32, u32 flag
        let w = w.u8(action).u32(COMMAND_MAGIC).proto(&msg).u32(0).u32(spell).u8(1);
        let w = (0..3).fold(w, |w, _| w.u32(0)).u8(0x20).u8(0).u8(0).u8(0);
        w.u32(0).u32(0).u32(0xffff_ffff).u32(0)
    })
}

/// A live enemy for one AI tick: id, playfield, home, reach (stationary enemies), keep
/// distance, the player who hit it lately, its side among rival enemies.
type Fighter = (Id32, Pf, [f32; 3], Option<f32>, f32, Option<Id32>, Option<u32>);

/// NPCs and smashables per copy (the instance list, control.rs).
pub fn counts() -> HashMap<Pf, usize> {
    let mut counts = HashMap::new();
    for n in NPCS.lock().unwrap().values() {
        *counts.entry(n.playfield).or_default() += 1;
    }
    counts
}

/// The last AI tick's time (µs), and the longest since `TICK_MAX_SINCE` (unix s), for
/// `tick_stats`.
static TICK_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TICK_MAX_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TICK_MAX_SINCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long the last AI tick took and the longest in the last minute (ms).
pub fn tick_stats() -> (f32, f32) {
    use std::sync::atomic::Ordering::Relaxed;
    (TICK_US.load(Relaxed) as f32 / 1000.0, TICK_MAX_US.load(Relaxed) as f32 / 1000.0)
}

/// Times one AI tick (until dropped).
struct TickTimer(std::time::Instant);

impl Drop for TickTimer {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        let us = self.0.elapsed().as_micros() as u64;
        TICK_US.store(us, Relaxed);
        let now = crate::smash::now_ms() as u64 / 1000;
        if now >= TICK_MAX_SINCE.load(Relaxed) + 60 {
            TICK_MAX_SINCE.store(now, Relaxed);
            TICK_MAX_US.store(us, Relaxed);
        } else {
            TICK_MAX_US.fetch_max(us, Relaxed);
        }
    }
}

/// Chase the nearest player in range and fight it; walk home when there is none or it ran too far.
async fn ai() {
    let mut tick = tokio::time::interval(TICK);
    // playfields that emptied and when, until they are unloaded
    let mut emptied: HashMap<Pf, std::time::Instant> = HashMap::new();
    let mut awake: std::collections::HashSet<Pf> = Default::default();
    loop {
        tick.tick().await;
        let _timing = TickTimer(std::time::Instant::now());
        // only playfields with players run (others' fights went on with nobody watching), and
        // in the world editor's nothing moves or attacks
        let mut occupied = occupied_playfields();
        occupied.retain(|pf| !crate::instance::rules(*pf).editor);
        let now = std::time::Instant::now();
        for pf in awake.difference(&occupied) {
            emptied.insert(*pf, now);
        }
        emptied.retain(|pf, since| {
            let keep = !occupied.contains(pf) && since.elapsed() < UNLOAD_AFTER;
            if !keep && !occupied.contains(pf) {
                unload(*pf);
                crate::scripts::playfield_emptied(*pf);
                crate::triggers::playfield_emptied(*pf);
            }
            keep
        });
        awake = occupied;
        let awake = &awake;
        let live: Vec<Fighter> = NPCS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, n)| awake.contains(&n.playfield))
            .filter(|(_, n)| n.def.kind == Kind::Enemy && n.presence.is_some() && n.health > 0)
            .map(|(id, n)| {
                let threat = n.threat.filter(|(_, t)| t.elapsed() < THREAT_MEMORY).map(|(p, _)| p);
                (*id, n.playfield, n.def.pos, stand_reach(&n.def), keep_distance(&spells_for(&n.def)), threat, n.def.faction)
            })
            .collect();
        // rival enemies: (id, playfield, side, position)
        let rivals: Vec<(Id32, Pf, u32, [f32; 3])> = NPCS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, n)| awake.contains(&n.playfield) && n.def.kind == Kind::Enemy && n.presence.is_some() && n.health > 0)
            .filter_map(|(id, n)| Some((*id, n.playfield, n.def.faction?, *n.pos.lock().unwrap())))
            .collect();
        pets_tick();
        fighters_tick(awake);
        // players and friendly fighters per playfield, collected once per tick (most
        // playfields have none)
        let mut players: HashMap<Pf, Vec<(Id32, [f32; 3])>> = HashMap::new();
        let mut fighters: HashMap<Pf, Vec<(Id32, [f32; 3])>> = HashMap::new();
        for (_, playfield, _, _, _, _, _) in &live {
            players
                .entry(*playfield)
                .or_insert_with(|| world::positions(*playfield).into_iter().filter(|(other, _)| is_player(*other) && !calm(*other) && !listening(*playfield, *other)).collect());
            fighters.entry(*playfield).or_insert_with(|| fighters_on(*playfield));
        }
        for (id, playfield, home, reach, keep, threat, faction) in live {
            let Some(here) = current(id) else { continue };
            if let Some(reach) = reach {
                let target = players[&playfield]
                    .iter()
                    .copied()
                    .filter(|(_, p)| distance(*p, here) < reach)
                    .min_by(|(_, a), (_, b)| distance(*a, here).total_cmp(&distance(*b, here)));
                if let Some((player, p)) = target {
                    if engage(id, player, p, here) {
                        face(id, here, p);
                    }
                }
                continue;
            }
            let in_range = |(_, p): &(Id32, [f32; 3])| distance(*p, home) < LEASH_RANGE && distance(*p, here) < AGGRO_RANGE;
            let nearest = |list: &[(Id32, [f32; 3])]| {
                list.iter().copied().filter(in_range).min_by(|(_, a), (_, b)| distance(*a, here).total_cmp(&distance(*b, here)))
            };
            let player = nearest(&players[&playfield]);
            // a player in range who hit it lately stays its target; otherwise a friendly
            // fighter only when no player is closer
            let attacker = threat.and_then(|t| players[&playfield].iter().copied().find(|x| x.0 == t)).filter(in_range);
            // the nearest friendly fighter, or enemy of a rival side
            let rival = faction.and_then(|mine| {
                rivals
                    .iter()
                    .filter(|(_, pf, side, p)| *pf == playfield && *side != mine && distance(*p, here) < RIVAL_AGGRO && distance(*p, home) < LEASH_RANGE)
                    .map(|(rid, _, _, p)| (*rid, *p))
                    .min_by(|(_, a), (_, b)| distance(*a, here).total_cmp(&distance(*b, here)))
            });
            let foe = match (nearest(&fighters[&playfield]), rival) {
                (Some(f), Some(r)) => Some(if distance(r.1, here) < distance(f.1, here) { r } else { f }),
                (f, r) => f.or(r),
            };
            let target = attacker.or(match (player, foe) {
                (Some(p), Some(f)) if distance(f.1, here) < distance(p.1, here) => Some(f),
                (Some(p), _) => Some(p),
                (None, f) => f,
            });
            match target {
                Some((player, p)) => {
                    end_patrol(id);
                    let cast = engage(id, player, p, here);
                    if is_busy(id) {
                        // casting: it stops (facing the player) until the strike
                        steer(id, here, here, Some(p));
                        if cast {
                            face(id, here, p);
                        }
                    } else {
                        steer(id, here, approach(here, p, keep), Some(p));
                    }
                }
                // stunned or casting: it stays where it is
                None if is_busy(id) => steer(id, here, here, None),
                None if patrols(id) => patrol(id, here),
                None => steer(id, here, home, None),
            }
        }
        // friendly NPCs that patrol
        let walkers: Vec<Id32> = NPCS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, n)| awake.contains(&n.playfield) && n.def.kind == Kind::Npc && !n.def.patrol.is_empty() && n.presence.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in walkers {
            if let Some(here) = current(id) {
                patrol(id, here);
            }
        }
    }
}

/// Players enemies leave alone for now (`clear_hate`), until when.
static CALM: LazyLock<Mutex<HashMap<Id32, std::time::Instant>>> = LazyLock::new(Default::default);

/// Whether enemies leave `player` alone in `playfield` while a dialogue is open: in the tutorial
/// (the Scallywag Dreadleg summons waits for his lines to end; the HUD is hidden meanwhile).
fn listening(playfield: Pf, player: Id32) -> bool {
    playfield.model() == crate::tutorial::PLAYFIELD && crate::knubot::talking(player)
}

fn calm(player: Id32) -> bool {
    CALM.lock().unwrap().get(&player).is_some_and(|t| *t > std::time::Instant::now())
}

/// Enemies stop chasing `player` (a ClearHate area: Scry `ba_clearhatetowardsme`, triggers.rs):
/// their memory of its hits is gone and for `lasts` none picks it as a target, so they walk
/// home. How long live's script kept them off is unknown (docs/TUNABLES.md).
pub fn clear_hate(player: Id32, lasts: Duration) {
    let mut n = 0;
    for npc in NPCS.lock().unwrap().values_mut() {
        if npc.threat.is_some_and(|(p, _)| p == player) {
            npc.threat = None;
            n += 1;
        }
    }
    let mut calm = CALM.lock().unwrap();
    let now = std::time::Instant::now();
    calm.retain(|_, t| *t > now);
    calm.insert(player, now + lasts);
    info!(player = player.instance, forgot = n, ?lasts, "hate cleared");
}

/// The playfields players are on.
fn occupied_playfields() -> std::collections::HashSet<Pf> {
    // world::members first: is_player locks NPCS, which mustn't be taken under MEMBERS
    world::members().into_iter().filter(|(id, _)| is_player(*id)).map(|(_, pf)| pf).collect()
}

/// Puts the NPCs of a playfield nobody is on back as they were loaded: at home, full health,
/// not walking or fighting (pets and the dead, waiting to respawn, are left alone). Nobody sees
/// it happen; whoever comes next is sent them so (their spawn records read health and position).
fn unload(playfield: Pf) {
    let mut npcs = NPCS.lock().unwrap();
    let mut count = 0;
    for npc in npcs.values_mut().filter(|n| n.playfield == playfield && n.owner.is_none() && n.presence.is_some() && n.health > 0) {
        npc.health = npc.def.health;
        npc.walk = None;
        npc.patrol = None;
        npc.threat = None;
        npc.busy_until = None;
        npc.last_attack = None;
        npc.cooldowns.clear();
        *npc.pos.lock().unwrap() = npc.def.pos;
        count += 1;
    }
    info!(?playfield, count, "playfield empty: NPCs reset, asleep until a player comes");
}

/// Whether NPC `id` has a patrol route.
fn patrols(id: Id32) -> bool {
    NPCS.lock().unwrap().get(&id).is_some_and(|n| n.def.patrol.len() >= 2)
}

/// It stops patrolling (to fight): `patrol` starts again at the nearest route point.
fn end_patrol(id: Id32) {
    if let Some(npc) = NPCS.lock().unwrap().get_mut(&id) {
        if npc.patrol.take().is_some() {
            npc.walk = None;
        }
    }
}

/// An enemy at `pos` walking towards `to` (no world presence), for crowd control tests.
#[cfg(test)]
pub fn test_enemy(n: u32, pos: [f32; 3], to: [f32; 3]) -> Id32 {
    let mut def: SpawnDef = serde_json::from_value(serde_json::json!({ "name": "Test Enemy", "monster_data": 82410, "level": 1, "health": 100, "pos": pos })).unwrap();
    def.kind = Kind::Enemy;
    let id = Id32::new(crate::territory::CHARACTER_TYPE, 900_000 + n);
    let mut npc = Npc::new(def, Pf::main(9999), None, None);
    npc.walk = Some(Walk { from: pos, to, since: std::time::Instant::now(), speed: 4.0 });
    NPCS.lock().unwrap().insert(id, npc);
    id
}

/// A test enemy's crowd control state: (path speed multiplier, stunned or busy, position).
#[cfg(test)]
pub fn crowd_state(id: Id32) -> (f32, bool, [f32; 3]) {
    let busy = is_busy(id);
    let npcs = NPCS.lock().unwrap();
    let n = &npcs[&id];
    (n.speed_factor, busy, *n.pos.lock().unwrap())
}

#[cfg(test)]
pub fn remove_test_enemy(id: Id32) {
    NPCS.lock().unwrap().remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smash_attack_result_matches_recording() {
        // the tutorial wagon smashed by Video Game Guy (rec 5316), body after the header
        let rec = attack_result_record(Id32::new(50000, 1896), Id32::new(51321, 2599), 600329, 1, false);
        let body = "0000002e0a2a0a0708d0860310e80e120708f9900310a7141a120d0000803f10e807180020002800380048002089d2241000";
        let hex = crate::conn::hex(&rec);
        assert!(hex.ends_with(body), "{hex}");
    }

    fn test_spawn_def() -> SpawnDef {
        SpawnDef {
            variants: Vec::new(),
            count: 1,
            spread: None,
            requires: Vec::new(),
            cinematic: false,
            kind: Kind::Enemy,
            template: 0,
            name: "Enemy".into(),
            monster_data: 1,
            level: 1,
            health: 62,
            pos: [0.0; 3],
            spell: default_spell(),
            cars_group: None,
            npc_template: None,
            icon: None,
            reach: None,
        stationary: false,
        unarmed: false,
            anchor: None,
            fights: false,
            faction: None,
            patrol: Vec::new(),
            loot: None,
        power_level: None,
            source: None,
            rot: None,
        }
    }

    #[test]
    fn patrol_record_matches_live() {
        // a Pirate World Musketeer's patrol (live capture, PassBlob 15505): from where it is to
        // the route's second point, then the two-point route from there, looping
        let v = |x: u32, y: u32, z: u32| [f32::from_bits(x), f32::from_bits(y), f32::from_bits(z)];
        let here = v(0x42aefff3, 0x428c0180, 0x42b5f823);
        let route = [v(0x42bd6911, 0x428be70d, 0x42bcda17), v(0x42b245f0, 0x428c0000, 0x42b4c28d)];
        let hex = crate::conn::hex(&patrol_record(Id32::new(50000, 0x1ea2e6), here, &route, 1));
        let live = "96c467400000c350001ea2e6000042aefff3428c018042b5f82304010002010100000000000242aefff3428c018042b5f82342b245f0428c000042b4c28d000000000000000000003d4c8ebe010100000000000242b245f0428c000042b4c28d42bd6911428be70d42bcda17010000000000000000003e49c40a";
        // all but the two SplinePath segment floats (bytes 72 and 118 after the record's
        // length: length / 35 here, / 34.94 on live)
        let mask = |h: &str| format!("{}{}{}", &h[..72 * 2], &h[76 * 2..118 * 2], &h[122 * 2..]);
        assert_eq!(hex.len(), live.len());
        assert_eq!(mask(&hex), mask(live));
    }

    #[test]
    fn patrols_walk_their_route() {
        // the NPC heads for the nearest point, then along the route at PATROL_SPEED × walk
        let id = Id32::new(50000, 899_999);
        let route = vec![[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 0.0, 3.0]];
        let mut def = SpawnDef { patrol: route.clone(), ..test_spawn_def() };
        def.pos = [0.2, 0.0, 0.0];
        NPCS.lock().unwrap().insert(id, Npc::new(def.clone(), Pf::main(1), None, None));
        patrol(id, def.pos);
        assert_eq!(NPCS.lock().unwrap()[&id].patrol.map(|p| p.0), Some(0));
        // pretend it set out long enough ago to have walked past point 1
        let speed = PATROL_SPEED * 2.1;
        let back = Duration::from_secs_f32((0.2 + 3.0 + 1.0) / speed);
        if let Some(npc) = NPCS.lock().unwrap().get_mut(&id) {
            let w = npc.walk.as_mut().unwrap();
            w.since -= back;
        }
        patrol(id, def.pos);
        let npcs = NPCS.lock().unwrap();
        let npc = &npcs[&id];
        assert_eq!(npc.patrol.map(|p| p.0), Some(2));
        let at = *npc.pos.lock().unwrap();
        assert!(distance(at, [3.0, 0.0, 1.0]) < 0.05, "{at:?}");
        drop(npcs);
        NPCS.lock().unwrap().remove(&id);
        // copies placed elsewhere don't patrol
        assert!(Npc::new(def, Pf::main(1), None, Some(std::time::Instant::now())).def.patrol.is_empty());
    }

    #[test]
    fn enemy_damage_is_the_live_fit() {
        // L15 regulars: 49 per AbilityMod (live 46-52); champions and bosses 2.25x; minions 0.3x
        assert_eq!(enemy_damage(15, 1314).round(), 49.0);
        assert_eq!(enemy_damage(15, 14782), enemy_damage(15, 1314) * 2.25);
        assert_eq!(enemy_damage(15, 394), enemy_damage(15, 1314) * 0.3);
        // the recorded low levels: 3 at level 1, 4-5 at level 2
        assert_eq!(enemy_damage(1, 62), 3.0);
        assert_eq!(enemy_damage(2, 90), 4.0);
    }

    #[test]
    fn regular_life_is_the_live_servers() {
        // recorded levels, between them (5 and 7 recorded: 168, 282; 8 = a third of the way
        // to 10's 538) and past the last (74: 24298, 71: 21526)
        assert_eq!([1, 4, 15, 26].map(regular_life), [62, 132, 1314, 3854]);
        assert_eq!(regular_life(8), 367);
        assert_eq!(regular_life(77), 27070);
        let enemy = |level, health| SpawnDef { level, health, ..test_spawn_def() };
        // the live bosses rank as bosses, its champions as champions, Mantis Nymphs as regulars
        assert!(rank_of(&enemy(3, 855)) >= BOSS_RANK); // Monkey Kong, 7.5x
        assert!(rank_of(&enemy(15, 14782)) >= BOSS_RANK); // Monster Scientist, 11.25x
        assert!((CHAMPION_RANK..BOSS_RANK).contains(&rank_of(&enemy(15, 6652)))); // Gargantuan
        assert!(rank_of(&enemy(15, 657)) < CHAMPION_RANK);
    }

    #[test]
    fn npc_fights_spawn() {
        // the Isle of Yarr's beach: ghost pirates and the Jackets who fight them
        let table = crate::spawns::table();
        let yarr = &table["4200"];
        assert!(yarr.iter().all(|d| d.kind != Kind::Enemy || d.anchor.as_deref() != Some("Quest_Beach")));
        assert!(yarr.iter().any(|d| d.kind == Kind::Enemy && d.name == "Pirate Specter"));
        assert!(yarr.iter().any(|d| d.kind == Kind::Fighter));
    }

    #[test]
    fn every_fighter_has_enemies_to_fight() {
        // each friendly fighter's post is within reach of an enemy camp (it walks up to one)
        let table = crate::spawns::table();
        let mut fighters = 0;
        for (playfield, defs) in &table {
            for f in defs.iter().filter(|d| d.fights()) {
                fighters += 1;
                let near = defs.iter().filter(|e| e.kind == Kind::Enemy && distance(e.pos, f.pos) < FIGHTER_AGGRO.min(FIGHTER_LEASH)).count();
                assert!(near >= 2, "{playfield} {}: {near} enemies near", f.name);
            }
        }
        assert!(fighters >= 8, "{fighters}");
        // rival enemies meet somewhere on the Isle of Yarr
        let yarr = &table["4200"];
        let meet = yarr.iter().filter_map(|a| Some((a, a.faction?))).any(|(a, fa)| {
            yarr.iter().any(|b| b.faction.is_some_and(|fb| fb != fa) && distance(a.pos, b.pos) < RIVAL_AGGRO)
        });
        assert!(meet);
    }

    #[test]
    fn fight_damage_lasts() {
        // about 1/FIGHT_SHARE hits to defeat, at most a third more or less
        let hits = 1.0 / (FIGHT_SHARE * (1.0 + FIGHT_SPREAD));
        assert!(hits >= 15.0, "{hits}");
    }

    use super::test_enemy as walking_enemy;

    #[tokio::test]
    async fn snares_slow_the_path_and_wear_off() {
        let id = walking_enemy(1, [0.0; 3], [100.0, 0.0, 0.0]);
        assert_eq!(snare(id, 0.5, Duration::from_millis(200)), Some(Duration::from_millis(200)));
        let speed = |id| NPCS.lock().unwrap()[&id].walk.unwrap().speed;
        assert_eq!(speed(id), 2.0);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(speed(id), 4.0);
        NPCS.lock().unwrap().remove(&id);
    }

    #[test]
    fn stuns_stop_and_diminish() {
        let id = walking_enemy(2, [0.0; 3], [100.0, 0.0, 0.0]);
        // as the client data registers it before each stun: 10 s window, 0.34 a stun
        let mut got = Vec::new();
        for _ in 0..4 {
            diminish(id, CC_STUN, Duration::from_secs(10), 0.34);
            got.push(stun(id, Duration::from_secs(2)).map(|d| d.as_millis()));
        }
        assert_eq!(got, vec![Some(2000), Some(1320), Some(640), None]);
        assert!(NPCS.lock().unwrap()[&id].walk.is_none());
        assert!(is_busy(id));
        NPCS.lock().unwrap().remove(&id);
    }

    #[test]
    fn champions_cast_their_zone_aura_once_per_aura() {
        let now = std::time::Instant::now();
        let level = 5;
        let base = regular_life(level) as u32;
        let def = |health: u32| SpawnDef { level, health, ..test_spawn_def() };
        // a world with an element
        let (pf, element) = crate::gamedata::get().worlds.values().find_map(|w| Some((w.playfield, w.element?))).unwrap();
        let mut champion = Npc::new(def(base * 5), Pf::main(pf), None, None);
        let aura = champion_aura(&mut champion, now).expect("a champion's aura");
        let skill = &crate::gamedata::get().tunables.champions.auras[&format!("{element:?}").to_lowercase()];
        assert_eq!(Some(aura), crate::gamedata::get().skills[skill].client.as_ref().map(|c| c.spell));
        assert_eq!(champion_aura(&mut champion, now), None, "already on");
        // not a regular enemy, nor a boss, nor a champion below PowerLevel 300
        assert_eq!(champion_aura(&mut Npc::new(def(base), Pf::main(pf), None, None), now), None);
        assert_eq!(champion_aura(&mut Npc::new(def(base * 10), Pf::main(pf), None, None), now), None);
        let low = SpawnDef { level: 2, health: regular_life(2) as u32 * 5, ..test_spawn_def() };
        assert_eq!(champion_aura(&mut Npc::new(low, Pf::main(pf), None, None), now), None);
    }

    #[test]
    fn knockbacks_push_and_pulls_pull() {
        let id = walking_enemy(3, [10.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
        let at = |id| *NPCS.lock().unwrap()[&id].pos.lock().unwrap();
        assert!(knock(id, [0.0; 3], 5.0, false, 25.0));
        assert!((at(id)[0] - 15.0).abs() < 1e-4, "{:?}", at(id));
        assert!(knock(id, [0.0; 3], 20.0, true, 25.0));
        // pulled up to melee range, not through the caster
        assert!((at(id)[0] - MELEE_RANGE).abs() < 1e-4, "{:?}", at(id));
        NPCS.lock().unwrap().remove(&id);
    }

    #[test]
    fn inactive_minifigures_regenerate() {
        let player = Id32::new(crate::territory::CHARACTER_TYPE, 1_900_000_001);
        let (a, b) = (41_757, 41_840);
        set_active_minifigure(player, a, 1000, 0.0);
        set_player_health(player, 100);
        set_active_minifigure(player, b, 2000, 0.0);
        set_player_health(player, 50);
        set_smashed(player, 99);
        assert_eq!(set_active_minifigure(player, a, 1000, 0.0), 100);
        assert_eq!(player_health(player), Some(100));
        PLAYERS.lock().unwrap().get_mut(&player).unwrap().health.insert(b, 50);
        for _ in 0..3 {
            regen_tick(&Default::default(), 0.05, 0.005);
        }
        let p = PLAYERS.lock().unwrap().remove(&player).unwrap();
        // 5% of each one's own maximum a tick, out of combat
        assert_eq!(p.health[&a], 100 + 3 * 50);
        assert_eq!(p.health[&b], 50 + 3 * 100);
        assert_eq!(p.health[&99], 0, "smashed stays down");
    }

    #[test]
    fn stationary_enemies_stay_put() {
        let id = walking_enemy(4, [10.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
        NPCS.lock().unwrap().get_mut(&id).unwrap().def.stationary = true;
        assert!(!knock(id, [0.0; 3], 5.0, false, 25.0));
        assert!(!knock(id, [0.0; 3], 5.0, true, 25.0));
        assert_eq!(*NPCS.lock().unwrap()[&id].pos.lock().unwrap(), [10.0, 0.0, 0.0]);
        assert!(!is_busy(id));
        NPCS.lock().unwrap().remove(&id);
    }
}
