//! LEGO gates (`module_wall_4x4_gate_lego_myth_interactable`) that block until something opens
//! them, then play their open motion (the interactables' blend setup, mirrors.rs `TURN_PLUS`):
//!
//! - Light gates (data/light_gates.json, tools/re/extract_light_gates.py): Medusa's Lair's gates
//!   with "mirror light" spots facing them. Each spot gets a mirror buildie ("XX_boss medusa
//!   mirror buildie"); once built it beams light at the gate, and with every spot of a gate lit
//!   the gate opens (the user's memory of live).
//! - Arena exit gates: a gate standing at an arena's exit door visual (Medusa's) opens with that
//!   exit (encounter.rs).
//! - Room gates: the Gauntlet's plate room (plates.rs) and mirror room (mirrors.rs).
//!
//! Other gates stay open scenery (scenery.rs). Built mirrors stay built until the server
//! restarts, as the other buildies.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use bytes::Bytes;
use lmoxide_net::{iir, types::Id32};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::{scenery::SIMPLE_DYNEL_TYPE, world};

const GATE: &str = "module_wall_4x4_gate_lego_myth_interactable";
/// The gate model's collision (static visual 2260 is the same gate with a CollisionMesh2; the
/// interactable's own mesh has none).
const GATE_COLLISION: u32 = 2260;
/// The open motion (blend setup 474600 `anim` = 1), as the mirrors'.
const OPEN: u32 = 474576;
/// Medusa's mirror buildie: item template and look ("mirror_medusa_b_buildie").
const MIRROR_TEMPLATE: u32 = 489061;
const MIRROR_LOOK: u32 = 489796;
/// How near an arena exit visual a gate must stand to open with it.
const EXIT_DISTANCE: f32 = 3.0;
/// Instances of the beam dynels (clear of mirrors.rs's).
const BEAM_INSTANCES: u32 = 0x00c1_0000;

#[derive(Deserialize)]
struct Spot {
    guid: u32,
    pos: [f32; 3],
    light: [f32; 3],
    rot: [f32; 4],
    length: f32,
}

#[derive(Deserialize)]
struct LightGate {
    gate: u32,
    mirrors: Vec<Spot>,
}

static DATA: LazyLock<HashMap<u32, Vec<LightGate>>> = LazyLock::new(|| {
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/light_gates.json")).expect("light_gates.json");
    raw.into_iter()
        .filter_map(|(pf, v)| Some((pf.parse().ok()?, serde_json::from_value(v).expect("light_gates.json playfield"))))
        .collect()
});

/// Exit gate -> the arena exit door visual beside it.
static EXIT_GATES: LazyLock<HashMap<u32, u32>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct Piece {
        name: String,
        pos: [f32; 3],
        guid: u32,
    }
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/world_content.json")).expect("world_content.json");
    let mut m = HashMap::new();
    for w in worlds.values() {
        let pieces: Vec<Piece> = w
            .get("simple_dynels")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|d| serde_json::from_value(d.clone()).ok()).collect())
            .unwrap_or_default();
        for gate in pieces.iter().filter(|p| p.name == GATE) {
            let exit = pieces.iter().find(|p| {
                crate::encounter::is_exit_door(p.guid) && ((p.pos[0] - gate.pos[0]).powi(2) + (p.pos[2] - gate.pos[2]).powi(2)).sqrt() <= EXIT_DISTANCE
            });
            if let Some(exit) = exit {
                m.insert(gate.guid, exit.guid);
            }
        }
    }
    m
});

/// A gate's "door collision" piece (an invisible marker at the gate, turned along the wall) ->
/// the gate. While the gate is shut it carries the arena shields' wall (encounter.rs
/// `SHIELD_COLLISION`, 6 m wide): the gate model's own collision left gaps to squeeze through.
static WALLS: LazyLock<HashMap<u32, u32>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct Piece {
        name: String,
        pos: [f32; 3],
        guid: u32,
    }
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/world_content.json")).expect("world_content.json");
    let mut m = HashMap::new();
    for w in worlds.values() {
        let pieces: Vec<Piece> = w
            .get("simple_dynels")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|d| serde_json::from_value(d.clone()).ok()).collect())
            .unwrap_or_default();
        for wall in pieces.iter().filter(|p| p.name.starts_with("door") && p.name.ends_with("collision")) {
            let gate = pieces.iter().find(|g| g.name == GATE && ((g.pos[0] - wall.pos[0]).powi(2) + (g.pos[2] - wall.pos[2]).powi(2)).sqrt() < 1.0);
            if let Some(gate) = gate {
                m.insert(wall.guid, gate.guid);
            }
        }
    }
    m
});

/// Whether a simple dynel is a gate's wall piece (scenery.rs spawns it); `Some(blocks now)`.
pub fn wall_blocks(guid: u32) -> Option<bool> {
    let gate = *WALLS.get(&guid)?;
    gate_open(gate).map(|open| !open)
}

/// Mirror buildie -> (playfield, gate, spot, beam dynel).
static MIRRORS: LazyLock<Mutex<HashMap<Id32, (u32, u32, &'static Spot, Id32)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static PRESENCES: Mutex<Vec<world::Presence>> = Mutex::new(Vec::new());

/// Puts the mirror buildies and their (dark) beam dynels into Medusa's Lair.
pub fn start() {
    let mut n = 0;
    for (&pf, gates) in DATA.iter() {
        for gate in gates {
            for spot in &gate.mirrors {
                let buildie = crate::buildie::add("XX_boss medusa mirror buildie", MIRROR_TEMPLATE, MIRROR_LOOK, pf, spot.pos, spot.rot);
                let beam = Id32::new(SIMPLE_DYNEL_TYPE, BEAM_INSTANCES + n);
                n += 1;
                MIRRORS.lock().unwrap().insert(buildie, (pf, gate.gate, spot, beam));
                let (outbox, mut inbox) = mpsc::unbounded_channel();
                tokio::spawn(async move { while inbox.recv().await.is_some() {} });
                let presence = world::join(
                    beam,
                    pf,
                    Arc::new(Mutex::new(spot.light)),
                    move || {
                        let mut records = crate::scenery::invisible_records(beam, pf, spot.light, spot.rot);
                        if crate::buildie::built(buildie) {
                            records.push(crate::mirrors::beam_particle(beam, spot.length));
                        }
                        records
                    },
                    outbox,
                );
                PRESENCES.lock().unwrap().push(presence);
            }
        }
    }
    info!(mirrors = n, exit_gates = EXIT_GATES.len(), "light gates");
}

/// A buildie was built: a mirror's beam comes on, and its gate opens once all its mirrors shine.
pub fn built(buildie: Id32) {
    let Some(&(pf, gate, spot, beam)) = MIRRORS.lock().unwrap().get(&buildie) else { return };
    info!(playfield = pf, gate, spot = spot.guid, "mirror built: light on the gate");
    world::send_playfield(beam, false, world::Out::Records(vec![crate::mirrors::beam_particle(beam, spot.length)]));
    if gate_open(gate) == Some(true) {
        info!(playfield = pf, gate, "light gate opens");
        send_state(gate, true);
    }
}

/// Shows a gate open or shut: its motion and collision, and its wall piece's.
fn send_state(gate: u32, open: bool) {
    let id = Id32::new(SIMPLE_DYNEL_TYPE, gate);
    world::send_playfield(id, false, world::Out::Records(open_records(id, open)));
    for (&wall, _) in WALLS.iter().filter(|(_, g)| **g == gate) {
        let id = Id32::new(SIMPLE_DYNEL_TYPE, wall);
        world::send_playfield(id, false, world::Out::Records(vec![crate::encounter::collision_record(id, !open)]));
    }
}

/// Whether a gate this module runs is open now (`None`: not one of them).
fn gate_open(guid: u32) -> Option<bool> {
    // room puzzles: the Gauntlet's plate room (plates.rs) and mirror room (mirrors.rs)
    if guid == crate::plates::GATE {
        return Some(crate::plates::solved());
    }
    if let Some(open) = crate::mirrors::gate_open(guid) {
        return Some(open);
    }
    if let Some(&exit) = EXIT_GATES.get(&guid) {
        return Some(crate::encounter::door_hidden(exit) == Some(true));
    }
    let mirrors = MIRRORS.lock().unwrap();
    let spots: Vec<Id32> = mirrors.iter().filter(|(_, m)| m.1 == guid).map(|(b, _)| *b).collect();
    drop(mirrors);
    (!spots.is_empty()).then(|| spots.iter().all(|b| crate::buildie::built(*b)))
}

/// Whether a gate blocks now (scenery.rs spawns it with the gate's collision).
pub fn blocks(guid: u32) -> bool {
    gate_open(guid) == Some(false)
}

pub fn collision() -> u32 {
    GATE_COLLISION
}

/// What a gate's spawn adds: its open pose, if it's open.
pub fn pose_records(guid: u32) -> Vec<Bytes> {
    match gate_open(guid) {
        Some(true) => vec![crate::effects::effect_package(Id32::new(SIMPLE_DYNEL_TYPE, guid), OPEN)],
        _ => Vec::new(),
    }
}

/// The open motion (or the closed pose) and the collision to match.
fn open_records(id: Id32, open: bool) -> Vec<Bytes> {
    let motion = crate::effects::effect_package(id, if open { OPEN } else { crate::mirrors::TURN_MINUS });
    let collision = iir::record("SimpleDynelVisualChangeIIR_c", id, false, |w| w.u32(4).u32(if open { 0 } else { GATE_COLLISION }));
    vec![motion, collision]
}

/// A room puzzle was solved or reset: its gate opens or shuts to match.
pub fn refresh(guid: u32) {
    if let Some(open) = gate_open(guid) {
        send_state(guid, open);
    }
}

/// An arena's doors changed (encounter.rs): its exit gates open or shut with them.
pub fn arena_doors_changed(doors: &[u32]) {
    for (&gate, exit) in EXIT_GATES.iter().filter(|(_, exit)| doors.contains(exit)) {
        send_state(gate, crate::encounter::door_hidden(*exit) == Some(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_have_their_walls() {
        // Medusa's Lair's four gates and the Gauntlet's five
        for gate in [476102, 474105, 476024, 474103, 506181, 506102, 517927, 516809, 517465] {
            assert!(WALLS.values().any(|g| *g == gate), "gate {gate} has no wall piece");
        }
    }
}
