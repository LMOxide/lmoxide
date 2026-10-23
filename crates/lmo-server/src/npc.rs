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
        BCC_STATE_LIMBO, CharSpec, STAT_BCC_STATE, STAT_HEALTH, combat_log_record, int32_stat, move_task_record, remote_spawn_records},
    world,
};

/// CarsGroup of hostile NPCs (target group "Enemies" in the client's CARS rules).
const CARS_ENEMIES: u32 = 3;
/// NPC dynel instances start here (players are accounts, so keep clear of small numbers).
const FIRST_NPC_INSTANCE: u32 = 900_000;
const AGGRO_RANGE: f32 = 12.0;
const LEASH_RANGE: f32 = 25.0;
/// How close an enemy walks up to its target.
const MELEE_RANGE: f32 = 1.8;
const RESPAWN: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Deserialize)]
pub struct SpawnDef {
    pub name: String,
    pub monster_data: u32,
    pub level: u32,
    pub health: u32,
    pub pos: [f32; 3],
}

struct Npc {
    def: SpawnDef,
    playfield: u32,
    health: u32,
    pos: world::Position,
    presence: Option<world::Presence>,
}

static NPCS: LazyLock<Mutex<HashMap<Id32, Npc>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

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
            let id = Id32::new(crate::territory::CHARACTER_TYPE, instance);
            instance += 1;
            let pos = Arc::new(Mutex::new(def.pos));
            NPCS.lock().unwrap().insert(id, Npc { health: def.health, def, playfield, pos, presence: None });
            appear(id);
        }
    }
    info!(count = NPCS.lock().unwrap().len(), "NPCs spawned");
    tokio::spawn(ai());
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
        remote_spawn_records(id, &spec(&def, playfield, *spawn_pos.lock().unwrap(), health))
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
/// if `target` isn't a live NPC.
pub fn hit(attacker: Id32, target: Id32, spell: u32, damage: u32) -> Option<bool> {
    let mut npcs = NPCS.lock().unwrap();
    let npc = npcs.get_mut(&target)?;
    if npc.presence.is_none() || npc.health == 0 {
        return None;
    }
    let dealt = damage.min(npc.health);
    npc.health -= dealt;
    let health = npc.health;
    info!(?attacker, npc = %npc.def.name, damage, health, "hit");
    let mut records = vec![combat_log_record(attacker, target, spell, dealt), int32_stat(target, STAT_HEALTH, health)];
    if health == 0 {
        // Limbo: the client plays the death and stops targeting it
        records.push(int32_stat(target, STAT_BCC_STATE, BCC_STATE_LIMBO));
    }
    world::send_playfield(target, true, world::Out::Records(records));
    if health == 0 {
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
        .filter(|(_, n)| n.playfield == playfield && n.presence.is_some() && n.health > 0)
        .filter(|(_, n)| distance(*n.pos.lock().unwrap(), center) <= radius)
        .map(|(id, _)| *id)
        .collect();
    targets.into_iter().filter(|t| hit(attacker, *t, spell, damage) == Some(true)).count() as u32
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

/// Chase the nearest player in range; walk home when there is none or it ran too far.
async fn ai() {
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        let live: Vec<(Id32, u32, [f32; 3], world::Position)> = NPCS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, n)| n.presence.is_some() && n.health > 0)
            .map(|(id, n)| (*id, n.playfield, n.def.pos, n.pos.clone()))
            .collect();
        for (id, playfield, home, pos) in live {
            let here = *pos.lock().unwrap();
            let target = world::positions(playfield)
                .into_iter()
                .filter(|(other, p)| !is_npc(*other) && distance(*p, home) < LEASH_RANGE)
                .map(|(_, p)| p)
                .filter(|p| distance(*p, here) < AGGRO_RANGE)
                .min_by(|a, b| distance(*a, here).total_cmp(&distance(*b, here)));
            let goal = match target {
                Some(p) => approach(here, p, MELEE_RANGE),
                None => home,
            };
            if distance(goal, here) > 0.3 {
                *pos.lock().unwrap() = goal;
                world::broadcast(id, vec![move_task_record(id, here, goal)]);
            }
        }
    }
}
