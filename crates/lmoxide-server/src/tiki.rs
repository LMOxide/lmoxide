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

use crate::instance::Pf;
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
/// The skull's fireball: projectile item (its hit radius: content/items `radius`), speed,
/// reach and damage (nano 519853).
const FIREBALL: u32 = 519852;
const FIREBALL_SPEED: f32 = 10.0;
/// How far the fireballs fly: the line patterns (the user: right), the spiral (further, it
/// looked too short).
const REACH: f32 = 12.0;
const SPIRAL_REACH: f32 = 18.0;
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
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
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

/// Per copy of the Gauntlet: the statue buildies (in marker order), and the room's invisible
/// dynels: the skull (forcefield and fireballs) and one glow per statue.
static STATUES: LazyLock<Mutex<HashMap<Pf, Vec<Id32>>>> = LazyLock::new(Default::default);
static PRESENCES: LazyLock<Mutex<HashMap<Pf, Vec<world::Presence>>>> = LazyLock::new(Default::default);

fn skull_id(pf: Pf) -> Id32 {
    Id32::new(SIMPLE_DYNEL_TYPE, crate::instance::fixture(pf, INSTANCES))
}

fn glow_id(pf: Pf, i: usize) -> Id32 {
    Id32::new(SIMPLE_DYNEL_TYPE, crate::instance::fixture(pf, INSTANCES + 1 + i as u32))
}

/// Whether every statue of copy `pf` is built (the gate opens).
pub fn solved(pf: Pf) -> bool {
    let statues = STATUES.lock().unwrap().get(&pf).cloned().unwrap_or_default();
    !statues.is_empty() && statues.iter().all(|b| crate::buildie::built(*b))
}

/// Runs the skull's fire (the room itself comes with each copy, `spawn_world`).
pub fn start() {
    info!(statues = ROOM_DATA.statues.len(), "tiki room");
    tokio::spawn(fire());
}

/// Puts the statues, the skull and the glows into copy `pf` of the Gauntlet.
pub fn spawn_world(pf: Pf) {
    if pf.model() != PLAYFIELD {
        return;
    }
    let room = &*ROOM_DATA;
    let statues: Vec<Id32> = room
        .statues
        .iter()
        .map(|statue| {
            let pos = [statue.pos[0], statue.pos[1] - STATUE_MARKER_HEIGHT, statue.pos[2]];
            crate::buildie::add("XX_Statue_Buildie", STATUE_TEMPLATE, STATUE_LOOK, pf, pos, statue.rot)
        })
        .collect();
    STATUES.lock().unwrap().insert(pf, statues);
    join(pf, skull_id(pf), room.holder, [0.0, 0.0, 0.0, 1.0], move || (!solved(pf)).then_some(FORCEFIELD));
    for (i, statue) in room.statues.iter().enumerate() {
        join(pf, glow_id(pf, i), statue.pos, statue.rot, move || {
            let built = STATUES.lock().unwrap().get(&pf).and_then(|s| s.get(i).copied()).is_some_and(crate::buildie::built);
            built.then_some(STATUE_GLOW)
        });
    }
}

/// Takes copy `pf`'s room out (its statue buildies go with the copy's, buildie.rs).
pub fn despawn_world(pf: Pf) {
    STATUES.lock().unwrap().remove(&pf);
    let gone = PRESENCES.lock().unwrap().remove(&pf);
    drop(gone);
}

/// An invisible dynel showing `particle()` (looping) while it gives one.
fn join(pf: Pf, id: Id32, pos: [f32; 3], rot: [f32; 4], particle: impl Fn() -> Option<u32> + Send + Sync + 'static) {
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    let presence = world::join(
        id,
        pf,
        Arc::new(Mutex::new(pos)),
        move || {
            let mut records = crate::scenery::invisible_records(id, pf, pos, rot);
            records.extend(particle().map(|p| particle_record(id, Some(p))));
            records
        },
        outbox,
    );
    PRESENCES.lock().unwrap().entry(pf).or_default().push(presence);
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
    let found = STATUES.lock().unwrap().iter().find_map(|(pf, s)| Some((*pf, s.iter().position(|b| *b == buildie)?)));
    let Some((pf, i)) = found else { return };
    info!(playfield = ?pf, statue = i + 1, "tiki statue built");
    world::send_playfield(glow_id(pf, i), false, world::Out::Records(vec![particle_record(glow_id(pf, i), Some(STATUE_GLOW))]));
    if solved(pf) {
        info!(playfield = ?pf, "tiki room solved: the skull's forcefield falls");
        world::send_playfield(skull_id(pf), false, world::Out::Records(vec![particle_record(skull_id(pf), None)]));
        crate::gates::refresh(pf, GATE);
    }
}

/// Players in copy `pf`'s room now.
fn in_room(pf: Pf) -> Vec<(Id32, world::Position)> {
    world::position_handles(pf)
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
/// How far out of the skull the spiral's fireballs start (the side markers stand ~0.8 m out).
const SPIRAL_START: f32 = 1.2;

/// The skull's fire: a pattern every `VOLLEY` in each copy whose room is occupied and unsolved
/// (not the world editor's, where nothing attacks).
async fn fire() {
    let mut tick = tokio::time::interval(VOLLEY);
    // per copy: (pattern turn, side turn)
    let mut turns: HashMap<Pf, (usize, usize)> = HashMap::new();
    loop {
        tick.tick().await;
        let copies = crate::worlds::copies_of(PLAYFIELD);
        turns.retain(|pf, _| copies.contains(pf));
        for pf in copies {
            if crate::instance::rules(pf).editor || solved(pf) || in_room(pf).is_empty() {
                continue;
            }
            let (turn, side) = turns.entry(pf).or_default();
            let pattern = PATTERNS[*turn % PATTERNS.len()];
            *turn += 1;
            let room = &*ROOM_DATA;
            let shots: Vec<([f32; 3], [f32; 2])> = match pattern {
                Pattern::Side => {
                    let (dir, origins) = &room.spells[*side % room.spells.len()];
                    *side += 1;
                    origins.iter().map(|o| (*o, *dir)).collect()
                }
                Pattern::Cross => room.spells.iter().filter_map(|(dir, origins)| origins.get(1).map(|o| (*o, *dir))).collect(),
                Pattern::Spiral => Vec::new(),
            };
            tokio::spawn(volley(pf, pattern, shots));
        }
    }
}

/// One pattern in copy `pf`: the ground warning, then the fireballs.
async fn volley(pf: Pf, pattern: Pattern, shots: Vec<([f32; 3], [f32; 2])>) {
    let room = &*ROOM_DATA;
    // the warning: ground fire along each line
    let warnings: Vec<[f32; 3]> = match pattern {
        // (none before a spiral: ground fire round the skull looked like fireballs bursting on it)
        Pattern::Spiral => Vec::new(),
        _ => shots
            .iter()
            .flat_map(|(o, d)| (1..=3).map(move |k| [o[0] + d[0] * REACH * k as f32 / 3.0, o[1] - 0.7, o[2] + d[1] * REACH * k as f32 / 3.0]))
            .collect(),
    };
    let records = warnings.into_iter().map(|at| crate::effects::effect_package_at(skull_id(pf), WARNING, at)).collect();
    world::send_playfield(skull_id(pf), false, world::Out::Records(records));
    tokio::time::sleep(WARN).await;
    match pattern {
        Pattern::Spiral => {
            for k in 0..SPIRAL_SHOTS {
                let a = k as f32 * SPIRAL_ANGLE;
                // from just outside the skull (from its centre the fireball burst on it)
                let from = [room.holder[0] + a.sin() * SPIRAL_START, room.holder[1] - 0.9, room.holder[2] + a.cos() * SPIRAL_START];
                shoot(pf, from, [a.sin(), a.cos()], SPIRAL_REACH);
                tokio::time::sleep(SPIRAL_STEP).await;
            }
        }
        _ => {
            for (origin, dir) in shots {
                shoot(pf, origin, dir, REACH);
            }
        }
    }
}

/// One fireball from `origin` along `dir` (x, z): the first player in the room it touches on
/// its way is hit (`projectile::launch_at_players`).
fn shoot(pf: Pf, origin: [f32; 3], dir: [f32; 2], reach: f32) {
    let to = [origin[0] + dir[0] * reach, origin[1], origin[2] + dir[1] * reach];
    let on_hit: crate::projectile::OnHit = std::sync::Arc::new(move |player, _| {
        if let Some(player) = player.filter(|p| in_room(pf).iter().any(|(id, _)| id == p)) {
            crate::npc::hurt_player(skull_id(pf), player, FIREBALL, DAMAGE);
        }
    });
    crate::projectile::launch_at_players(skull_id(pf), pf, 0, FIREBALL, FIREBALL_SPEED, 0, origin, to, reach, None, on_hit);
}
