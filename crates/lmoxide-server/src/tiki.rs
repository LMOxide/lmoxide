//! The Gauntlet's Tiki room (6300, "Quest_TikiRoom"): a skull in an orange forcefield ("Tiki Room
//! Skull Forcefield Holder") shoots fire in patterns while four golden statues (look
//! `statue_hero_a_buildie`, confirmed by the user; at the "statue N particle" markers) wait to be
//! built;
//! with all four built the forcefield and the fire stop and the room's east gate opens (the
//! user's memory of live, 2026-10-25).
//!
//! The patterns, in turn: three fireballs ("XX_Skull Fireball Projectile", hit 250) out of one
//! side (its "Skull_Spell_<dir>_1-3" markers; north, east, south, west by turns), one out of
//! every side at once, and a spiral of fireballs sweeping round; each after a ground warning. Pieces, items and particles are client data; the timing, the order and the
//! warning's look are inferred (docs/TUNABLES.md "Gauntlet"). Built statues stay built, as all
//! buildies, until the server restarts.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use lmoxide_net::{iir, types::Id32};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::{scenery::SIMPLE_DYNEL_TYPE, world};

pub const PLAYFIELD: u32 = 6300;
/// The gate the statues open (the Tiki room's east gate).
pub const GATE: u32 = 506102;
const HOLDER: &str = "Tiki Room Skull Forcefield Holder";
/// The statues' item template: "XX_Statue_Buildie" (506103) has no stats in the client data, and
/// the client won't build it; the recorded generic buildie's (1600 build points) instead.
const STATUE_TEMPLATE: u32 = 446283;
/// `statue_hero_a_buildie` (the user confirmed it; `gauntlet_elements_dino_buildie` 529120 showed
/// a grey statue the client wouldn't let anyone build).
const STATUE_LOOK: u32 = 152320;
/// The statue buildies stand this far below their particle markers (the room's floor).
const STATUE_MARKER_HEIGHT: f32 = 2.0;
/// Particles: the forcefield (red-orange bubble), a built statue's glow, the warning.
const FORCEFIELD: u32 = 412053;
const STATUE_GLOW: u32 = 532195;
/// Ground fire along the coming fireballs' lines (effect package of the monkeys' ground fire).
const WARNING: u32 = 241037;
/// The skull's fireball: projectile item, speed, reach, hit radius and damage (nano 519853).
const FIREBALL: u32 = 519852;
const FIREBALL_SPEED: f32 = 10.0;
const REACH: f32 = 12.0;
const HIT_RADIUS: f32 = 1.2;
const DAMAGE: u32 = 250;
/// A volley every `VOLLEY` while someone is in the room, the warning `WARN` before it.
const VOLLEY: Duration = Duration::from_secs(4);
const WARN: Duration = Duration::from_millis(1200);
/// The room (x, z) and its floor band, from the Quest_TikiRoom area.
const ROOM: ([f32; 2], [f32; 2], [f32; 2]) = ([251.0, 287.0], [160.0, 185.0], [46.0, 54.0]);
/// Instances of the room's own invisible dynels (clear of mirrors.rs's and gates.rs's).
const INSTANCES: u32 = 0x00c2_0000;

#[derive(Deserialize, Clone)]
struct Piece {
    name: String,
    pos: [f32; 3],
    rot: [f32; 4],
}

struct Room {
    holder: [f32; 3],
    statues: Vec<Piece>,
    /// direction (N, E, S, W) -> (unit vector x, z, the three spell origins)
    spells: Vec<([f32; 2], Vec<[f32; 3]>)>,
}

static ROOM_DATA: LazyLock<Room> = LazyLock::new(|| {
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/world_content.json")).expect("world_content.json");
    let pieces: Vec<Piece> = worlds
        .get(&PLAYFIELD.to_string())
        .and_then(|w| w.get("simple_dynels"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|d| serde_json::from_value(d.clone()).ok()).collect())
        .unwrap_or_default();
    let holder = pieces.iter().find(|p| p.name == HOLDER).map_or([275.9, 49.6, 172.9], |p| p.pos);
    let mut statues: Vec<Piece> = pieces.iter().filter(|p| p.name.starts_with("statue ") && p.name.ends_with(" particle")).cloned().collect();
    statues.sort_by(|a, b| a.name.cmp(&b.name));
    let spells = [("N", [0.0, 1.0]), ("E", [1.0, 0.0]), ("S", [0.0, -1.0]), ("W", [-1.0, 0.0])]
        .iter()
        .map(|&(d, v)| (v, (1..=3).filter_map(|i| pieces.iter().find(|p| p.name == format!("Skull_Spell_{d}_{i}")).map(|p| p.pos)).collect()))
        .collect();
    Room { holder, statues, spells }
});

/// Statue buildies (in marker order) and the room's invisible dynels: the skull (forcefield
/// and fireballs) and one glow per statue.
static STATUES: Mutex<Vec<Id32>> = Mutex::new(Vec::new());
static PRESENCES: Mutex<Vec<world::Presence>> = Mutex::new(Vec::new());

fn skull_id() -> Id32 {
    Id32::new(SIMPLE_DYNEL_TYPE, INSTANCES)
}

fn glow_id(i: usize) -> Id32 {
    Id32::new(SIMPLE_DYNEL_TYPE, INSTANCES + 1 + i as u32)
}

/// Whether every statue is built (the gate opens).
pub fn solved() -> bool {
    let statues = STATUES.lock().unwrap();
    !statues.is_empty() && statues.iter().all(|b| crate::buildie::built(*b))
}

/// Puts the statues, the skull and the glows in, and runs the skull's fire.
pub fn start() {
    let room = &*ROOM_DATA;
    for statue in &room.statues {
        let pos = [statue.pos[0], statue.pos[1] - STATUE_MARKER_HEIGHT, statue.pos[2]];
        let id = crate::buildie::add("XX_Statue_Buildie", STATUE_TEMPLATE, STATUE_LOOK, PLAYFIELD, pos, statue.rot);
        STATUES.lock().unwrap().push(id);
    }
    join(skull_id(), room.holder, [0.0, 0.0, 0.0, 1.0], || (!solved()).then_some(FORCEFIELD));
    for (i, statue) in room.statues.iter().enumerate() {
        join(glow_id(i), statue.pos, statue.rot, move || {
            let built = STATUES.lock().unwrap().get(i).is_some_and(|b| crate::buildie::built(*b));
            built.then_some(STATUE_GLOW)
        });
    }
    info!(statues = room.statues.len(), "tiki room");
    tokio::spawn(fire());
}

/// An invisible dynel showing `particle()` (looping) while it gives one.
fn join(id: Id32, pos: [f32; 3], rot: [f32; 4], particle: impl Fn() -> Option<u32> + Send + Sync + 'static) {
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    let presence = world::join(
        id,
        PLAYFIELD,
        Arc::new(Mutex::new(pos)),
        move || {
            let mut records = crate::scenery::invisible_records(id, PLAYFIELD, pos, rot);
            records.extend(particle().map(|p| particle_record(id, Some(p))));
            records
        },
        outbox,
    );
    PRESENCES.lock().unwrap().push(presence);
}

/// `SimpleDynelVisualChangeIIR_c`: state 0 starts a looping particle (attractor 35, as live), 2
/// stops the first one.
fn particle_record(id: Id32, particle: Option<u32>) -> bytes::Bytes {
    iir::record("SimpleDynelVisualChangeIIR_c", id, false, |w| match particle {
        Some(p) => w.u32(0).u32(p).i32(35).f32(0.0).f32(0.0).f32(0.0).f32(0.0),
        None => w.u32(2).u32(0),
    })
}

/// A buildie was built (buildie.rs): a statue's glow comes on, and with all four the skull's
/// forcefield goes and the gate opens.
pub fn built(buildie: Id32) {
    let Some(i) = STATUES.lock().unwrap().iter().position(|b| *b == buildie) else { return };
    info!(statue = i + 1, "tiki statue built");
    world::send_playfield(glow_id(i), false, world::Out::Records(vec![particle_record(glow_id(i), Some(STATUE_GLOW))]));
    if solved() {
        info!("tiki room solved: the skull's forcefield falls");
        world::send_playfield(skull_id(), false, world::Out::Records(vec![particle_record(skull_id(), None)]));
        crate::gates::refresh(GATE);
    }
}

/// Players in the room now.
fn in_room() -> Vec<(Id32, world::Position)> {
    world::position_handles(PLAYFIELD)
        .into_iter()
        .filter(|(id, pos)| {
            let p = *pos.lock().unwrap();
            crate::npc::is_player(*id) && (ROOM.0[0]..=ROOM.0[1]).contains(&p[0]) && (ROOM.1[0]..=ROOM.1[1]).contains(&p[2]) && (ROOM.2[0]..=ROOM.2[1]).contains(&p[1])
        })
        .collect()
}

/// The skull's patterns, in turn (the user's memory of live: lines, four lines, a spiral).
#[derive(Clone, Copy)]
enum Pattern {
    /// three lines out of one side (north, east, south, west by turns)
    Side,
    /// one line out of each side at once
    Cross,
    /// a fireball every `SPIRAL_STEP`, each `SPIRAL_ANGLE` further round
    Spiral,
}

const PATTERNS: [Pattern; 3] = [Pattern::Side, Pattern::Cross, Pattern::Spiral];
const SPIRAL_SHOTS: usize = 16;
const SPIRAL_STEP: Duration = Duration::from_millis(150);
const SPIRAL_ANGLE: f32 = std::f32::consts::FRAC_PI_4;

/// The skull's fire: a pattern every `VOLLEY` while the room is occupied and unsolved.
async fn fire() {
    let room = &*ROOM_DATA;
    let mut tick = tokio::time::interval(VOLLEY);
    let (mut turn, mut side) = (0, 0);
    loop {
        tick.tick().await;
        if solved() || in_room().is_empty() {
            continue;
        }
        let pattern = PATTERNS[turn % PATTERNS.len()];
        turn += 1;
        let shots: Vec<([f32; 3], [f32; 2])> = match pattern {
            Pattern::Side => {
                let (dir, origins) = &room.spells[side % room.spells.len()];
                side += 1;
                origins.iter().map(|o| (*o, *dir)).collect()
            }
            Pattern::Cross => room.spells.iter().filter_map(|(dir, origins)| origins.get(1).map(|o| (*o, *dir))).collect(),
            Pattern::Spiral => Vec::new(),
        };
        // the warning: ground fire along each line (round the skull before a spiral)
        let warnings: Vec<[f32; 3]> = match pattern {
            Pattern::Spiral => (0..8)
                .map(|k| {
                    let a = k as f32 * SPIRAL_ANGLE;
                    [room.holder[0] + a.sin() * 2.5, room.holder[1] - 1.5, room.holder[2] + a.cos() * 2.5]
                })
                .collect(),
            _ => shots
                .iter()
                .flat_map(|(o, d)| (1..=3).map(move |k| [o[0] + d[0] * REACH * k as f32 / 3.0, o[1] - 0.7, o[2] + d[1] * REACH * k as f32 / 3.0]))
                .collect(),
        };
        let records = warnings.into_iter().map(|at| crate::effects::effect_package_at(skull_id(), WARNING, at)).collect();
        world::send_playfield(skull_id(), false, world::Out::Records(records));
        tokio::time::sleep(WARN).await;
        match pattern {
            Pattern::Spiral => {
                let from = [room.holder[0], room.holder[1] - 0.9, room.holder[2]];
                for k in 0..SPIRAL_SHOTS {
                    let a = k as f32 * SPIRAL_ANGLE;
                    shoot(from, [a.sin(), a.cos()]);
                    tokio::time::sleep(SPIRAL_STEP).await;
                }
            }
            _ => {
                for (origin, dir) in shots {
                    shoot(origin, dir);
                }
            }
        }
    }
}

/// One fireball from `origin` along `dir` (x, z): whoever stands in its line when it passes is
/// hit.
fn shoot(origin: [f32; 3], dir: [f32; 2]) {
    let to = [origin[0] + dir[0] * REACH, origin[1], origin[2] + dir[1] * REACH];
    let (shot, flight) = crate::projectile::launch_enemy(skull_id(), PLAYFIELD, 0, FIREBALL, 1, FIREBALL_SPEED, 0, origin, to, None);
    tokio::spawn(async move {
        let mut hit = None;
        for (player, pos) in in_room() {
            let p = *pos.lock().unwrap();
            let (dx, dz) = (p[0] - origin[0], p[2] - origin[2]);
            let along = dx * dir[0] + dz * dir[1];
            let off = (dx * dir[1] - dz * dir[0]).abs();
            if (0.0..=REACH).contains(&along) && off <= HIT_RADIUS && hit.as_ref().is_none_or(|(_, a, _)| along < *a) {
                hit = Some((player, along, p));
            }
        }
        match hit {
            Some((player, along, at)) => {
                tokio::time::sleep(Duration::from_secs_f32(along / FIREBALL_SPEED)).await;
                crate::npc::hurt_player(skull_id(), player, FIREBALL, DAMAGE);
                crate::projectile::enemy_impact(skull_id(), shot, Some((player, at)));
            }
            None => {
                tokio::time::sleep(flight).await;
                crate::projectile::enemy_impact(skull_id(), shot, None);
            }
        }
    });
}
