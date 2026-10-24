//! Server-controlled NPCs (enemies).
//!
//! LMO's spawn tables and monster templates were server-side, so spawns come from
//! data/spawns.json (playfield -> name, monsterdata look, level, health, position). An enemy is
//! an ordinary character dynel (type 50000; instances <= 16,000,000 count as NPCs) whose
//! ProtoCoreCharacterIIR has `stats.cars_group = 3` (Enemies) and a non-zero
//! `npc_stats.template_id`: the client's CARS rules then let players target and attack it while
//! its Health is non-zero. Everything an NPC does is driven from here.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use lmoxide_net::{iir, proto::proto_core_character_iir::NpcStats, types::Id32};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    game::{
        BCC_STATE_LIMBO, COMMAND_MAGIC, STAT_LIFE, create_game_object, CharSpec, STAT_BCC_STATE, STAT_HEALTH, combat_log_record, int32_stat, remote_spawn_records},
    world,
};

/// CarsGroup of hostile NPCs (target group "Enemies" in the client's CARS rules).
const CARS_ENEMIES: u32 = 3;
/// CarsGroup of friendly NPCs players can use (talk to).
const CARS_FRIENDS: u32 = 2;
/// NPC dynel instances start here (players are accounts, so keep clear of small numbers).
static NEXT_INSTANCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(900_000);
const PET_FOLLOW: f32 = 2.0;
const PET_DAMAGE: u32 = 60;
/// Pets that shoot keep this distance from their target.
const PET_SHOOT_RANGE: f32 = 8.0;
const AGGRO_RANGE: f32 = 8.0;
const LEASH_RANGE: f32 = 25.0;
/// How close an enemy walks up to its target.
const MELEE_RANGE: f32 = 1.8;
const RESPAWN: Duration = Duration::from_secs(20);
const DESTRUCTIBLE_TYPE: u32 = 51321;
/// CarsGroup of breakable objects (target group "Destructibles").
const CARS_DESTRUCTIBLES: u32 = 4;
const TICK: Duration = Duration::from_millis(250);
/// A walking NPC gets a new path when its goal moved this far from the current path's end.
const REPATH: f32 = 1.5;
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
const ATTACK_INTERVAL: Duration = Duration::from_millis(2500);
/// Share of player damage enemies absorb (the original server's, measured in the recorded
/// session: ~20% for normal enemies, ~40% for bosses).
const ENEMY_MITIGATION: f32 = 0.2;
const BOSS_MITIGATION: f32 = 0.4;
/// Cap on a player's total damage mitigation (upgrades plus buffs).
const MAX_MITIGATION: f32 = 0.9;
/// Enemies with more health than this count as bosses.
const BOSS_HEALTH: u32 = 500;

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
    last_damaged: Option<std::time::Instant>,
}

/// Out of combat (no damage for this long) health regenerates.
const REGEN_DELAY: Duration = Duration::from_secs(5);
const REGEN_TICK: Duration = Duration::from_secs(1);
const REGEN_SHARE: f32 = 0.05;

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
    let h = p.health.entry(template).or_insert(max);
    *h = (*h).min(max);
    *h
}

pub fn player_left(player: Id32) {
    PLAYERS.lock().unwrap().remove(&player);
    crate::procs::left(player);
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

/// Heals the player's active minifigure (up to full); returns its new health.
pub fn heal_player(player: Id32, amount: u32) -> u32 {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.entry(player).or_default();
    let max = p.max;
    let h = p.health.entry(p.active).or_insert(max);
    *h = (*h + amount).min(max);
    *h
}

/// Damages the player's active minifigure; returns its new health (`None` if unknown).
fn damage_player(player: Id32, damage: u32) -> Option<u32> {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.get_mut(&player)?;
    p.last_damaged = Some(std::time::Instant::now());
    let (max, mitigation) = (p.max, (p.mitigation + p.buff_mitigation.values().sum::<f32>()).min(MAX_MITIGATION));
    let h = p.health.entry(p.active).or_insert(max);
    let dealt = ((damage as f32) * (1.0 - mitigation)).round().max(1.0) as u32;
    *h = h.saturating_sub(dealt);
    Some(*h)
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
}

/// data/world_content.json (tools/re/extract_world_content.py): per playfield, the enemies,
/// NPCs and smashables the original server spawned (from the recorded demo session in the client
/// folder for 1300/4600/4610) or proposed ones at the world's AI and resurrection points.
#[derive(Deserialize)]
struct WorldContent {
    #[serde(default)]
    enemies: Vec<SpawnDef>,
    #[serde(default)]
    npcs: Vec<SpawnDef>,
    #[serde(default)]
    smashables: Vec<SpawnDef>,
}

fn default_spell() -> u32 {
    2029
}

struct Npc {
    def: SpawnDef,
    /// Pets: the summoning player, when the pet vanishes, the summon spell and what its attack
    /// shoots.
    owner: Option<(Id32, std::time::Instant)>,
    summon: Option<(u32, Option<PetProjectile>)>,
    playfield: u32,
    health: u32,
    pos: world::Position,
    presence: Option<world::Presence>,
    last_attack: Option<std::time::Instant>,
    /// The path the clients are moving it along.
    walk: Option<Walk>,
    /// Spawned by a test scenario: doesn't respawn, and is gone at that time.
    temporary: Option<std::time::Instant>,
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
        Kind::Enemy => ENEMY_SPEEDS,
        _ => crate::game::PLAYER_SPEEDS,
    }
}

fn run_speed(def: &SpawnDef) -> f32 {
    speeds(def).iter().find(|(stat, _)| *stat == RUN_SPEED).map_or(4.0, |&(_, v)| v)
}

static NPCS: LazyLock<Mutex<HashMap<Id32, Npc>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// NPCs for test commands: enemies of a rank, "boss" (≥ 8 × a regular enemy's health of their
/// level), "champion" (≥ 4 ×) or "enemy" (all), or "npc" (friendly NPCs) or "smashable":
/// (label, playfield, home position), by playfield.
pub fn points(rank: &str) -> Vec<(String, u32, [f32; 3])> {
    let mut v: Vec<_> = NPCS
        .lock()
        .unwrap()
        .values()
        .filter(|n| n.owner.is_none())
        .filter(|n| {
            let regular = 60 + 15 * n.def.level;
            match rank {
                "boss" => n.def.kind == Kind::Enemy && n.def.health >= regular * 8,
                "champion" => n.def.kind == Kind::Enemy && n.def.health >= regular * 4 && n.def.health < regular * 8,
                "npc" => n.def.kind == Kind::Npc,
                "smashable" => n.def.kind == Kind::Smashable,
                _ => n.def.kind == Kind::Enemy,
            }
        })
        .map(|n| (format!("{} (level {}, {} health)", n.def.name, n.def.level, n.def.health), n.playfield, n.def.pos))
        .collect();
    v.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    v
}

/// A friendly (story or usable) NPC's name and playfield.
pub fn friendly(id: Id32) -> Option<(String, u32)> {
    NPCS.lock().unwrap().get(&id).filter(|n| n.def.kind == Kind::Npc).map(|n| (n.def.name.clone(), n.playfield))
}

/// A spawned NPC named `name` on `playfield`.
pub fn find(name: &str, playfield: u32) -> Option<Id32> {
    NPCS.lock().unwrap().iter().find(|(_, n)| n.playfield == playfield && n.def.name == name && n.presence.is_some()).map(|(id, _)| *id)
}

/// Where a live NPC is.
pub fn position(id: Id32) -> Option<[f32; 3]> {
    NPCS.lock().unwrap().get(&id).filter(|n| n.presence.is_some()).map(|n| *n.pos.lock().unwrap())
}

pub fn is_npc(id: Id32) -> bool {
    NPCS.lock().unwrap().contains_key(&id)
}

/// Spawns every NPC in data/spawns.json and starts their AI.
pub fn start() {
    let mut table: HashMap<String, Vec<SpawnDef>> =
        serde_json::from_str(include_str!("../../../data/spawns.json")).expect("spawns.json");
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/world_content.json")).expect("world_content.json");
    for (playfield, world) in worlds {
        if !world.is_object() {
            continue; // "_comment"
        }
        let content = match serde_json::from_value::<WorldContent>(world) {
            Ok(c) => c,
            Err(e) => {
                warn!(%playfield, "world_content.json: playfield skipped: {e}");
                continue;
            }
        };
        let defs = table.entry(playfield).or_default();
        defs.extend(content.enemies.into_iter().map(|d| SpawnDef { kind: Kind::Enemy, ..d }));
        defs.extend(content.npcs.into_iter().map(|d| SpawnDef { kind: Kind::Npc, ..d }));
        defs.extend(content.smashables.into_iter().map(|d| SpawnDef { kind: Kind::Smashable, ..d }));
    }
    for (playfield, defs) in table {
        let Ok(playfield) = playfield.parse::<u32>() else { continue };
        for def in defs {
            let kind = if def.kind == Kind::Smashable { DESTRUCTIBLE_TYPE } else { crate::territory::CHARACTER_TYPE };
            let id = Id32::new(kind, NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
            let pos = Arc::new(Mutex::new(def.pos));
            NPCS.lock().unwrap().insert(id, Npc { health: def.health, def, playfield, pos, presence: None, last_attack: None, owner: None, summon: None, walk: None, temporary: None });
            appear(id);
        }
    }
    info!(count = NPCS.lock().unwrap().len(), "NPCs spawned");
    tokio::spawn(ai());
    tokio::spawn(regenerate());
}

fn spec(def: &SpawnDef, playfield: u32, pos: [f32; 3], health: u32) -> CharSpec<'_> {
    CharSpec {
        name: &def.name,
        playfield,
        pos,
        monster_data: def.monster_data,
        level: def.level,
        health,
        max_health: def.health,
        cars_group: def.cars_group.unwrap_or(CARS_ENEMIES),
        npc: Some(NpcStats { template_id: Some(def.npc_template.unwrap_or(def.monster_data)), ..Default::default() }),
        speeds: speeds(def),
    }
}

/// Puts the NPC into the world (at full health, at home).
fn appear(id: Id32) {
    let mut npcs = NPCS.lock().unwrap();
    let Some(npc) = npcs.get_mut(&id) else { return };
    npc.health = npc.def.health;
    npc.walk = None;
    *npc.pos.lock().unwrap() = npc.def.pos;
    let (def, playfield, pos) = (npc.def.clone(), npc.playfield, npc.pos.clone());
    let spawn_pos = pos.clone();
    let spawn = move || {
        let health = NPCS.lock().unwrap().get(&id).map_or(def.health, |n| n.health);
        let pos = *spawn_pos.lock().unwrap();
        match def.kind {
            Kind::Enemy => remote_spawn_records(id, &spec(&def, playfield, pos, health)),
            Kind::Pet => remote_spawn_records(id, &CharSpec { cars_group: 0, ..spec(&def, playfield, pos, health) }),
            Kind::Npc => remote_spawn_records(
                id,
                &CharSpec { cars_group: def.cars_group.unwrap_or(CARS_FRIENDS), ..spec(&def, playfield, pos, health) },
            ),
            Kind::Smashable => smashable_records(id, &def, playfield, pos, health),
        }
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

static SUMMONS: LazyLock<HashMap<String, serde_json::Value>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../data/summons.json")).expect("summons.json"));

/// Test scenarios (devcmd `!test`): `count` copies of the enemy named `name` (any spawn of it
/// gives the look, level, health and attack) in a ring around `center` on `playfield`, for
/// `lifetime`. They fight like the others but don't respawn. Returns how many appeared.
pub fn spawn_test_enemies(name: &str, count: u32, playfield: u32, center: [f32; 3], lifetime: Duration) -> u32 {
    let wanted = name.to_ascii_lowercase();
    let Some(def) = NPCS
        .lock()
        .unwrap()
        .values()
        .find(|n| n.def.kind == Kind::Enemy && n.owner.is_none() && n.def.name.to_ascii_lowercase() == wanted)
        .map(|n| n.def.clone())
    else {
        return 0;
    };
    let until = std::time::Instant::now() + lifetime;
    let mut ids = Vec::new();
    for i in 0..count {
        let angle = i as f32 / count.max(1) as f32 * std::f32::consts::TAU;
        let pos = [center[0] + TEST_SPAWN_RING * angle.cos(), center[1], center[2] + TEST_SPAWN_RING * angle.sin()];
        let id = Id32::new(crate::territory::CHARACTER_TYPE, NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let def = SpawnDef { pos, ..def.clone() };
        let npc = Npc {
            health: def.health,
            def,
            playfield,
            pos: Arc::new(Mutex::new(pos)),
            presence: None,
            last_attack: None,
            owner: None,
            summon: None,
            walk: None,
            temporary: Some(until),
        };
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

/// How far from the player test enemies appear.
const TEST_SPAWN_RING: f32 = 7.0;

/// Whether `spell` summons a pet (data/summons.json).
pub fn is_summon(spell: u32) -> bool {
    SUMMONS.contains_key(&spell.to_string())
}

/// `owner` casts summon `spell` at `pos`: a pet that follows them and fights for a while.
pub fn summon(owner: Id32, playfield: u32, spell: u32, pos: [f32; 3]) {
    let Some(def) = SUMMONS.get(&spell.to_string()).and_then(|v| serde_json::from_value::<SummonDef>(v.clone()).ok()) else {
        return;
    };
    let id = Id32::new(crate::territory::CHARACTER_TYPE, NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let spawn = SpawnDef {
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
    let pos = Arc::new(Mutex::new(spawn.pos));
    NPCS.lock().unwrap().insert(
        id,
        Npc {
            health: spawn.health,
            def: spawn,
            playfield,
            pos,
            presence: None,
            last_attack: None,
            owner: Some((owner, until)),
            summon: Some((spell, def.projectile)),
            walk: None,
            temporary: None,
        },
    );
    appear(id);
}

/// `attacker` hits NPC `target` with `spell` for `damage`. Returns whether it died, or `None`
/// if `target` isn't a live NPC. A defeated NPC drops loot for the attacker.
pub fn hit(attacker: Id32, target: Id32, spell: u32, damage: u32) -> Option<bool> {
    let mut npcs = NPCS.lock().unwrap();
    // (not `is_player`: NPCS is locked here)
    let by_player = attacker.kind == crate::territory::CHARACTER_TYPE && !npcs.contains_key(&attacker);
    let npc = npcs.get_mut(&target)?;
    if npc.presence.is_none() || npc.health == 0 || matches!(npc.def.kind, Kind::Npc | Kind::Pet) {
        return None;
    }
    let mitigation = if npc.def.kind == Kind::Enemy {
        if npc.def.health > BOSS_HEALTH { BOSS_MITIGATION } else { ENEMY_MITIGATION }
    } else {
        0.0
    };
    // the attacking player's upgrade procs (damage bonuses)
    let damage = if by_player { crate::procs::outgoing(attacker, spell, damage) } else { damage };
    let dealt = (((damage as f32) * (1.0 - mitigation)).round().max(1.0) as u32).min(npc.health);
    npc.health -= dealt;
    let health = npc.health;
    // a knockback upgrade pushes a surviving enemy away from the attacker
    if by_player
        && health > 0
        && npc.def.kind == Kind::Enemy
        && let Some(distance) = crate::procs::knockback(attacker)
        && let Some(from) = world::position_of(attacker)
    {
        let here = npc.walk.take().map_or_else(|| *npc.pos.lock().unwrap(), |w| w.at(std::time::Instant::now()).0);
        let (dx, dz) = (here[0] - from[0], here[2] - from[2]);
        let d = dx.hypot(dz).max(0.1);
        let to = [here[0] + dx / d * distance, here[1], here[2] + dz / d * distance];
        *npc.pos.lock().unwrap() = to;
        world::send_playfield(target, false, world::Out::Records(vec![crate::game::move_task_record(target, here, to)]));
    }
    info!(?attacker, npc = %npc.def.name, damage, health, "hit");
    let kind = npc.def.kind;
    let mut records = vec![if kind == Kind::Smashable {
        crate::game::stat_record(target, STAT_HEALTH, health)
    } else {
        int32_stat(target, STAT_HEALTH, health)
    }];
    records.insert(0, combat_log_record(attacker, target, spell, dealt));
    if kind == Kind::Enemy && health == 0 {
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
        // champions and bosses by their health over a regular enemy's of that level (proposed
        // content: 4x and 10x; recorded bosses have more)
        let regular = 60 + 15 * npc.def.level;
        let source = match kind {
            Kind::Smashable => crate::loot::Source::Smashable,
            _ if npc.def.health >= regular * 8 => crate::loot::Source::Boss,
            _ if npc.def.health >= regular * 4 => crate::loot::Source::Champion,
            _ => crate::loot::Source::Enemy,
        };
        crate::loot::drop_loot(attacker, npc.playfield, *npc.pos.lock().unwrap(), target, source);
        if kind == Kind::Enemy && by_player {
            crate::procs::killed(attacker, target, spell, npc.playfield, *npc.pos.lock().unwrap());
        }
        // achievements (and XP) of the player who defeated it
        world::send_to(
            attacker,
            world::Out::Progress(crate::achievements::Event::Defeated {
                name: npc.def.name.clone(),
                smashable: kind == Kind::Smashable,
                boss: source == crate::loot::Source::Boss,
                champion: source == crate::loot::Source::Champion,
                level: npc.def.level,
                playfield: npc.playfield,
            }),
        );
        // Dropping the presence despawns it for everyone (UpdateMessage op 1).
        let presence = npc.presence.take();
        let temporary = npc.temporary.is_some();
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
                tokio::time::sleep(RESPAWN).await;
                drop(presence);
            } else {
                // let the death play before the body disappears
                tokio::time::sleep(Duration::from_secs(3)).await;
                drop(presence);
                tokio::time::sleep(RESPAWN).await;
            }
            appear(target);
        });
    }
    Some(health == 0)
}

/// Hits every live NPC on `playfield` within `radius` of `center`.
pub fn hit_area(attacker: Id32, playfield: u32, center: [f32; 3], radius: f32, spell: u32, damage: u32) -> u32 {
    hit_where(attacker, playfield, spell, damage, |p| distance(p, center) <= radius)
}

/// Hits every live NPC on `playfield` within `radius` of the line from `from` to `to` (a column,
/// e.g. a dash's path).
pub fn hit_column(attacker: Id32, playfield: u32, from: [f32; 3], to: [f32; 3], radius: f32, spell: u32, damage: u32) -> u32 {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    let len2 = (dx * dx + dz * dz).max(0.0001);
    hit_where(attacker, playfield, spell, damage, |p| {
        let t = (((p[0] - from[0]) * dx + (p[2] - from[2]) * dz) / len2).clamp(0.0, 1.0);
        distance(p, [from[0] + dx * t, p[1], from[2] + dz * t]) <= radius
    })
}

fn hit_where(attacker: Id32, playfield: u32, spell: u32, damage: u32, inside: impl Fn([f32; 3]) -> bool) -> u32 {
    let targets = targets_where(playfield, inside);
    targets.into_iter().filter(|(t, _)| hit(attacker, *t, spell, damage) == Some(true)).count() as u32
}

/// The live enemies and smashables on `playfield` whose position is `inside`, with their positions.
pub fn targets_where(playfield: u32, inside: impl Fn([f32; 3]) -> bool) -> Vec<(Id32, [f32; 3])> {
    NPCS.lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.playfield == playfield && n.presence.is_some() && n.health > 0)
        .filter(|(_, n)| matches!(n.def.kind, Kind::Enemy | Kind::Smashable))
        .map(|(id, n)| (*id, *n.pos.lock().unwrap()))
        .filter(|(_, p)| inside(*p))
        .collect()
}

/// Heals players' active minifigures that haven't been hurt for a while (a defeated, 0-health
/// minifigure stays down until healed otherwise).
async fn regenerate() {
    let mut tick = tokio::time::interval(REGEN_TICK);
    loop {
        tick.tick().await;
        let healed: Vec<(Id32, u32)> = PLAYERS
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|(_, p)| p.last_damaged.is_none_or(|t| t.elapsed() >= REGEN_DELAY))
            .filter_map(|(id, p)| {
                let max = p.max;
                let h = p.health.entry(p.active).or_insert(max);
                (*h > 0 && *h < max).then(|| {
                    // out-of-combat regeneration: 5% of maximum health per second
                    *h = (*h + (max as f32 * REGEN_SHARE).ceil() as u32).min(max);
                    (*id, *h)
                })
            })
            .collect();
        for (id, health) in healed {
            world::send_to(id, world::Out::Records(vec![int32_stat(id, STAT_HEALTH, health)]));
        }
    }
}

/// Pets: vanish when their time is up, otherwise attack an enemy near them or follow the owner.
fn pets_tick() {
    let now = std::time::Instant::now();
    type Pet = (Id32, Id32, std::time::Instant, u32, u32, Option<PetProjectile>);
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
                        hit(owner, eid, spell, PET_DAMAGE);
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
            npc.walk = Some(Walk { from: here, to: goal, since: now, speed: run_speed(&npc.def) });
            Some(path_record(id, here, goal))
        } else {
            None
        }
    };
    if let Some(record) = record {
        world::broadcast(id, vec![record]);
    }
}

/// `MoveTaskIIR_c` as the original server sent NPC paths: u8 0, start; ListMoveTask (4): u8 wait
/// flags 1, u8 current 0, u8 count 1; PathMoveTask (1): u8 wait flags 1, u16 path flags 0 (the
/// character's own speed), u32 2, start, end, spline block (u8 0, u32 0, u8 0, u32 0, f32
/// length / 35, as in the recording).
fn path_record(id: Id32, from: [f32; 3], to: [f32; 3]) -> bytes::Bytes {
    iir::record("MoveTaskIIR_c", id, false, |w| {
        let w = w.u8(0).f32(from[0]).f32(from[1]).f32(from[2]).u8(4).u8(1).u8(0).u8(1);
        let w = w.u8(1).u8(1).u16(0).u32(2);
        let w = [from, to].iter().flatten().fold(w, |w, v| w.f32(*v));
        w.u8(0).u32(0).u8(0).u32(0).f32(distance(from, to) / 35.0)
    })
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
fn smashable_records(id: Id32, def: &SpawnDef, playfield: u32, pos: [f32; 3], health: u32) -> Vec<bytes::Bytes> {
    use lmoxide_net::proto::{ProtoGameCodeDynelIir, ProtoQuat, ProtoSimpleDynelIir, ProtoVec3};
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        name: Some(def.name.clone()),
        template_item_id: Some(def.template),
        cars_group: Some(def.cars_group.unwrap_or(CARS_DESTRUCTIBLES)),
        ..Default::default()
    };
    let record = lmoxide_net::iir::record("DestructibleFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(def.monster_data).u32(MONSTER_DATA_RDB_TYPE).u32(0);
        let w = [-0.6, 0.0, -0.6, 0.6, 1.2, 0.6].iter().fold(w, |w, v| w.f32(*v));
        // scale (n3VisualDynel asserts scale != 0), u32, u8
        let w = w.f32(1.0).u32(0).u8(0);
        let w = w.u32(health).u32(0).f32(0.0).u32(0);
        let w = Id32::default().write(w);
        w.u32(0).f32(0.0).f32(0.0).u32(0).f32(0.0)
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

/// NPC `id` swings at `player` if its attack is ready: the cast (a MagicCommand created on the
/// NPC: BCC actions 16 queue, 4 success, 10 can-execute), the damage number and the player's
/// new health. At 0 the client switches to the next minifigure itself.
fn attack(id: Id32, player: Id32) {
    let (spell, level) = {
        let mut npcs = NPCS.lock().unwrap();
        let Some(npc) = npcs.get_mut(&id) else { return };
        if npc.last_attack.is_some_and(|t| t.elapsed() < ATTACK_INTERVAL) {
            return;
        }
        npc.last_attack = Some(std::time::Instant::now());
        (npc.def.spell, npc.def.level)
    };
    // enemy power ~ 1.5 × level + 1.5 (fit to the recorded session's hits)
    let damage = (1.5 * level as f32 + 1.5).round() as u32;
    // the player's defensive upgrade procs
    let damage = crate::procs::incoming(player, damage, crate::actions::is_channelling(player));
    let Some(health) = damage_player(player, damage) else { return };
    crate::procs::after_damage(player, health, player_max_health(player));
    let mut records: Vec<_> = [BCC_QUEUE, 4, 10].iter().map(|&a| cast_record(id, player, spell, a)).collect();
    records.push(combat_log_record(id, player, spell, damage));
    records.push(int32_stat(player, STAT_HEALTH, health));
    world::send_playfield(id, false, world::Out::Records(records));
    crate::actions::caster_hit(player);
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
pub fn observed_cast(caster: Id32, target: Id32, spell: u32) -> [bytes::Bytes; 3] {
    let id = NEXT_SERVER_COMMAND.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    [BCC_QUEUE, 4, 10].map(|a| cast_record_with_id(caster, target, spell, a, id))
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

/// Chase the nearest player in range; walk home when there is none or it ran too far.
async fn ai() {
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        let live: Vec<(Id32, u32, [f32; 3])> = NPCS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, n)| n.def.kind == Kind::Enemy && n.presence.is_some() && n.health > 0)
            .map(|(id, n)| (*id, n.playfield, n.def.pos))
            .collect();
        pets_tick();
        // players per playfield, collected once per tick (most playfields have none)
        let mut players: HashMap<u32, Vec<(Id32, [f32; 3])>> = HashMap::new();
        for (_, playfield, _) in &live {
            players
                .entry(*playfield)
                .or_insert_with(|| world::positions(*playfield).into_iter().filter(|(other, _)| !is_npc(*other)).collect());
        }
        for (id, playfield, home) in live {
            let Some(here) = current(id) else { continue };
            let target = players[&playfield]
                .iter()
                .copied()
                .filter(|(_, p)| distance(*p, home) < LEASH_RANGE)
                .filter(|(_, p)| distance(*p, here) < AGGRO_RANGE)
                .min_by(|(_, a), (_, b)| distance(*a, here).total_cmp(&distance(*b, here)));
            match target {
                Some((player, p)) => {
                    if distance(p, here) <= MELEE_RANGE + 1.0 {
                        attack(id, player);
                    }
                    steer(id, here, approach(here, p, MELEE_RANGE), Some(p));
                }
                None => steer(id, here, home, None),
            }
        }
    }
}
