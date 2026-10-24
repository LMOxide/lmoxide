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

use lmo_net::{proto::proto_core_character_iir::NpcStats, types::Id32};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    game::{
        BCC_STATE_LIMBO, COMMAND_MAGIC, STAT_LIFE, create_game_object, CharSpec, STAT_BCC_STATE, STAT_HEALTH, combat_log_record, int32_stat, move_task_record, remote_spawn_records},
    world,
};

/// CarsGroup of hostile NPCs (target group "Enemies" in the client's CARS rules).
const CARS_ENEMIES: u32 = 3;
/// NPC dynel instances start here (players are accounts, so keep clear of small numbers).
const FIRST_NPC_INSTANCE: u32 = 900_000;
const AGGRO_RANGE: f32 = 8.0;
const LEASH_RANGE: f32 = 25.0;
/// How close an enemy walks up to its target.
const MELEE_RANGE: f32 = 1.8;
const RESPAWN: Duration = Duration::from_secs(20);
const DESTRUCTIBLE_TYPE: u32 = 51321;
/// CarsGroup of breakable objects (target group "Destructibles").
const CARS_DESTRUCTIBLES: u32 = 4;
/// Stars dropped by a defeated enemy and a smashed object.
const ENEMY_STARS: usize = 10;
const SMASHABLE_STARS: usize = 3;
const TICK: Duration = Duration::from_millis(750);
const ATTACK_INTERVAL: Duration = Duration::from_millis(2500);
const NPC_DAMAGE: u32 = 8;
/// Full health of a minifigure.
pub const PLAYER_HEALTH: u32 = 100;

/// Players' combat state: each minifigure has its own health.
#[derive(Default)]
struct PlayerCombat {
    active: u32,
    health: HashMap<u32, u32>,
    last_damaged: Option<std::time::Instant>,
}

/// Out of combat (no damage for this long) health regenerates.
const REGEN_DELAY: Duration = Duration::from_secs(5);
const REGEN_TICK: Duration = Duration::from_secs(1);
const REGEN_PER_TICK: u32 = 5;

static PLAYERS: LazyLock<Mutex<HashMap<Id32, PlayerCombat>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Makes `template` the player's active minifigure and returns its health.
pub fn set_active_minifigure(player: Id32, template: u32) -> u32 {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.entry(player).or_default();
    p.active = template;
    *p.health.entry(template).or_insert(PLAYER_HEALTH)
}

pub fn player_left(player: Id32) {
    PLAYERS.lock().unwrap().remove(&player);
}

/// Heals the player's active minifigure (up to full); returns its new health.
pub fn heal_player(player: Id32, amount: u32) -> u32 {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.entry(player).or_default();
    let h = p.health.entry(p.active).or_insert(PLAYER_HEALTH);
    *h = (*h + amount).min(PLAYER_HEALTH);
    *h
}

/// Damages the player's active minifigure; returns its new health (`None` if unknown).
fn damage_player(player: Id32, damage: u32) -> Option<u32> {
    let mut players = PLAYERS.lock().unwrap();
    let p = players.get_mut(&player)?;
    p.last_damaged = Some(std::time::Instant::now());
    let h = p.health.entry(p.active).or_insert(PLAYER_HEALTH);
    *h = h.saturating_sub(damage);
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
}

fn default_spell() -> u32 {
    2029
}

struct Npc {
    def: SpawnDef,
    playfield: u32,
    health: u32,
    pos: world::Position,
    presence: Option<world::Presence>,
    last_attack: Option<std::time::Instant>,
}

static NPCS: LazyLock<Mutex<HashMap<Id32, Npc>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Where a live NPC is.
pub fn position(id: Id32) -> Option<[f32; 3]> {
    NPCS.lock().unwrap().get(&id).filter(|n| n.presence.is_some()).map(|n| *n.pos.lock().unwrap())
}

pub fn is_npc(id: Id32) -> bool {
    NPCS.lock().unwrap().contains_key(&id)
}

/// Spawns every NPC in data/spawns.json and starts their AI.
pub fn start() {
    let table: HashMap<String, Vec<SpawnDef>> =
        serde_json::from_str(include_str!("../../../data/spawns.json")).expect("spawns.json");
    let mut instance = FIRST_NPC_INSTANCE;
    for (playfield, defs) in table {
        let Ok(playfield) = playfield.parse::<u32>() else { continue };
        for def in defs {
            let kind = if def.kind == Kind::Smashable { DESTRUCTIBLE_TYPE } else { crate::territory::CHARACTER_TYPE };
            let id = Id32::new(kind, instance);
            instance += 1;
            let pos = Arc::new(Mutex::new(def.pos));
            NPCS.lock().unwrap().insert(id, Npc { health: def.health, def, playfield, pos, presence: None, last_attack: None });
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
        cars_group: CARS_ENEMIES,
        npc: Some(NpcStats { template_id: Some(def.monster_data), ..Default::default() }),
    }
}

/// Puts the NPC into the world (at full health, at home).
fn appear(id: Id32) {
    let mut npcs = NPCS.lock().unwrap();
    let Some(npc) = npcs.get_mut(&id) else { return };
    npc.health = npc.def.health;
    *npc.pos.lock().unwrap() = npc.def.pos;
    let (def, playfield, pos) = (npc.def.clone(), npc.playfield, npc.pos.clone());
    let spawn_pos = pos.clone();
    let spawn = move || {
        let health = NPCS.lock().unwrap().get(&id).map_or(def.health, |n| n.health);
        let pos = *spawn_pos.lock().unwrap();
        match def.kind {
            Kind::Enemy => remote_spawn_records(id, &spec(&def, playfield, pos, health)),
            Kind::Npc => remote_spawn_records(id, &CharSpec { cars_group: 0, ..spec(&def, playfield, pos, health) }),
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

/// `attacker` hits NPC `target` with `spell` for `damage`. Returns whether it died, or `None`
/// if `target` isn't a live NPC. A defeated NPC drops loot for the attacker.
pub fn hit(attacker: Id32, target: Id32, spell: u32, damage: u32) -> Option<bool> {
    let mut npcs = NPCS.lock().unwrap();
    let npc = npcs.get_mut(&target)?;
    if npc.presence.is_none() || npc.health == 0 || npc.def.kind == Kind::Npc {
        return None;
    }
    let dealt = damage.min(npc.health);
    npc.health -= dealt;
    let health = npc.health;
    info!(?attacker, npc = %npc.def.name, damage, health, "hit");
    let kind = npc.def.kind;
    let mut records = vec![int32_stat(target, STAT_HEALTH, health)];
    if kind == Kind::Enemy {
        records.insert(0, combat_log_record(attacker, target, spell, dealt));
        if health == 0 {
            // Limbo: the client plays the death and stops targeting it
            records.push(int32_stat(target, STAT_BCC_STATE, BCC_STATE_LIMBO));
        }
    }
    // (Health 0 on a destructible plays its smash.)
    world::send_playfield(target, true, world::Out::Records(records));
    if health == 0 {
        let stars = if kind == Kind::Enemy { ENEMY_STARS } else { SMASHABLE_STARS };
        let mut items = vec![crate::loot::STAR_ITEM; stars];
        if kind == Kind::Enemy && rand_percent() < 35 {
            items.push(crate::loot::HEALTH_ORB_ITEM);
        }
        crate::loot::drop_items(attacker, npc.playfield, *npc.pos.lock().unwrap(), target, items);
        // Dropping the presence despawns it for everyone (UpdateMessage op 1).
        let presence = npc.presence.take();
        drop(npcs);
        tokio::spawn(async move {
            // let the death play before the body disappears
            tokio::time::sleep(Duration::from_secs(3)).await;
            drop(presence);
            tokio::time::sleep(RESPAWN).await;
            appear(target);
        });
    }
    Some(health == 0)
}

/// Hits every live NPC on `playfield` within `radius` of `center`.
pub fn hit_area(attacker: Id32, playfield: u32, center: [f32; 3], radius: f32, spell: u32, damage: u32) -> u32 {
    let targets: Vec<Id32> = NPCS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, n)| n.playfield == playfield && n.presence.is_some() && n.health > 0 && n.def.kind != Kind::Npc)
        .filter(|(_, n)| distance(*n.pos.lock().unwrap(), center) <= radius)
        .map(|(id, _)| *id)
        .collect();
    targets.into_iter().filter(|t| hit(attacker, *t, spell, damage) == Some(true)).count() as u32
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
                let h = p.health.entry(p.active).or_insert(PLAYER_HEALTH);
                (*h > 0 && *h < PLAYER_HEALTH).then(|| {
                    *h = (*h + REGEN_PER_TICK).min(PLAYER_HEALTH);
                    (*id, *h)
                })
            })
            .collect();
        for (id, health) in healed {
            world::send_to(id, world::Out::Records(vec![int32_stat(id, STAT_HEALTH, health)]));
        }
    }
}

/// A cheap pseudo-random number in 0..100 (loot rolls).
fn rand_percent() -> u32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    nanos.wrapping_mul(2_654_435_761) % 100
}

/// `DestructibleFullUpdateIIR_c` after the GameObject: ProtoGameCodeDynelIIR; the SimpleDynel
/// part (ProtoSimpleDynelIIR with name, template and CarsGroup 4; VisualData: resId, resType
/// (1010201 = the visual is a monsterdata, as the templates' MonsterData stat says), u32 n +
/// n × 5 u32 particles, 6 × f32 bounds, f32 scale, u32, u8); CoreDestructible (u32 health,
/// u32, f32, u32 side, ID32 locked to); Destructible (u32, f32, f32, u32, f32). Then Life/Health.
fn smashable_records(id: Id32, def: &SpawnDef, playfield: u32, pos: [f32; 3], health: u32) -> Vec<bytes::Bytes> {
    use lmo_net::proto::{ProtoGameCodeDynelIir, ProtoQuat, ProtoSimpleDynelIir, ProtoVec3};
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        name: Some(def.name.clone()),
        template_item_id: Some(def.template),
        cars_group: Some(CARS_DESTRUCTIBLES),
        ..Default::default()
    };
    let record = lmo_net::iir::record("DestructibleFullUpdateIIR_c", id, false, |w| {
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
        int32_stat(id, STAT_CARS_GROUP, CARS_DESTRUCTIBLES),
    ]
}

/// RDB type of RDBMonsterData as the client registers it (resources live under 1010514).
const MONSTER_DATA_RDB_TYPE: u32 = 1010201;
const STAT_CARS_GROUP: u32 = 1050;

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
    let spell = {
        let mut npcs = NPCS.lock().unwrap();
        let Some(npc) = npcs.get_mut(&id) else { return };
        if npc.last_attack.is_some_and(|t| t.elapsed() < ATTACK_INTERVAL) {
            return;
        }
        npc.last_attack = Some(std::time::Instant::now());
        npc.def.spell
    };
    let Some(health) = damage_player(player, NPC_DAMAGE) else { return };
    let mut records: Vec<_> = [BCC_QUEUE, 4, 10].iter().map(|&a| cast_record(id, player, spell, a)).collect();
    records.push(combat_log_record(id, player, spell, NPC_DAMAGE));
    records.push(int32_stat(player, STAT_HEALTH, health));
    world::send_playfield(id, false, world::Out::Records(records));
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
    let msg = lmo_net::proto::CommandMessage {
        bcc_action_id: Some(605),
        commandqueue_server_id: Some(server_id),
        flags: Some(0),
        execution_status: Some(0),
        target: Some(lmo_net::proto::ProtoId32 { r#type: Some(target.kind), instance: Some(target.instance) }),
        ..Default::default()
    };
    lmo_net::iir::record("BehaviorControlCenterIIR_c", caster, false, |w| {
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
        let live: Vec<(Id32, u32, [f32; 3], world::Position)> = NPCS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, n)| n.def.kind == Kind::Enemy && n.presence.is_some() && n.health > 0)
            .map(|(id, n)| (*id, n.playfield, n.def.pos, n.pos.clone()))
            .collect();
        for (id, playfield, home, pos) in live {
            let here = *pos.lock().unwrap();
            let target = world::positions(playfield)
                .into_iter()
                .filter(|(other, p)| !is_npc(*other) && distance(*p, home) < LEASH_RANGE)
                .filter(|(_, p)| distance(*p, here) < AGGRO_RANGE)
                .min_by(|(_, a), (_, b)| distance(*a, here).total_cmp(&distance(*b, here)));
            let goal = match target {
                Some((player, p)) => {
                    if distance(p, here) <= MELEE_RANGE + 1.0 {
                        attack(id, player);
                    }
                    approach(here, p, MELEE_RANGE)
                }
                None => home,
            };
            if distance(goal, here) > 0.3 {
                *pos.lock().unwrap() = goal;
                world::broadcast(id, vec![move_task_record(id, here, goal)]);
            }
        }
    }
}
