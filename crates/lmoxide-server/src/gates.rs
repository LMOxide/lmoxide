//! LEGO gates (`module_wall_4x4_gate_lego_myth_interactable`) that block until something opens
//! them, then play their open motion (the interactables' blend setup, mirrors.rs `TURN_PLUS`):
//!
//! - Light gates (content/worlds/*/puzzles.json `light_gate`, world_data.rs): Medusa's Lair's gates
//!   with "mirror light" spots facing them. Each spot gets a mirror buildie ("XX_boss medusa
//!   mirror buildie"); once built it beams light at the gate, and with every spot of a gate lit
//!   the gate opens (the user's memory of live).
//! - Arena exit gates: a gate standing at an arena's exit door visual (Medusa's) opens with that
//!   exit (encounter.rs).
//! - Room gates: the Gauntlet's plate room (plates.rs), Tiki room (tiki.rs) and mirror room
//!   (mirrors.rs).
//! - The Underworld's front gate (`underworld_gate_a_myth_interactable`, keygate.rs): the
//!   pedestal's crystal opens it.
//! - Boss gates (`BOSS_GATES`): the jail's gate behind the Jail Warden opens when he falls.
//!
//! Other gates stay open scenery (scenery.rs). Built mirrors stay built until the server
//! restarts, as the other buildies.

use crate::instance::Pf;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
};

use bytes::Bytes;
use lmoxide_net::{iir, types::Id32};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::{scenery::SIMPLE_DYNEL_TYPE, world};

const GATE: &str = "module_wall_4x4_gate_lego_myth_interactable";
/// The Underworld's front gate (keygate.rs): no collision of its own, its wall piece blocks.
const UNDERWORLD_GATE: &str = "underworld_gate_a_myth_interactable";
/// The gate model's collision (static visual 2260 is the same gate with a CollisionMesh2; the
/// interactable's own mesh has none).
const GATE_COLLISION: u32 = 2260;
/// The open motion (blend setup 474600 `anim` = 1), as the mirrors' (the Underworld's
/// collapsing floor's too).
pub const OPEN: u32 = 474576;
/// Medusa's mirror buildie: item template and look ("mirror_medusa_b_buildie").
const MIRROR_TEMPLATE: u32 = 489061;
const MIRROR_LOOK: u32 = 489796;
/// How near an arena exit visual a gate must stand to open with it.
const EXIT_DISTANCE: f32 = 3.0;
/// Instances of the beam dynels (clear of mirrors.rs's).
const BEAM_INSTANCES: u32 = 0x00c1_0000;
/// Gates an enemy holds shut: (playfield, gate guid, the enemy whose defeat opens it). The live
/// capture's jail (4610): when the Jail Warden fell, the server played the gate's (576082) open
/// motion (`OPEN`) and cleared the collision of its "door 1 collision" piece (576750); live put
/// fx_generic_shield_5x3_a (351981) there, this server the gate walls' `SHIELD_COLLISION` (as
/// every gate's wall piece here).
const BOSS_GATES: [(u32, u32, &str); 1] = [(4610, 576_082, "Jail Warden")];

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
        serde_json::from_value(serde_json::json!(*crate::world_data::LIGHT_GATES)).expect("light gates");
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
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
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
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
    let mut m = HashMap::new();
    for w in worlds.values() {
        let pieces: Vec<Piece> = w
            .get("simple_dynels")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|d| serde_json::from_value(d.clone()).ok()).collect())
            .unwrap_or_default();
        for wall in pieces.iter().filter(|p| p.name.starts_with("door") && p.name.ends_with("collision")) {
            let gate = pieces.iter().find(|g| (g.name == GATE || g.name == UNDERWORLD_GATE) && ((g.pos[0] - wall.pos[0]).powi(2) + (g.pos[2] - wall.pos[2]).powi(2)).sqrt() < 1.0);
            if let Some(gate) = gate {
                m.insert(wall.guid, gate.guid);
            }
        }
    }
    m
});

/// Whether a simple dynel is a gate's wall piece (scenery.rs spawns it).
pub fn is_wall(guid: u32) -> bool {
    WALLS.contains_key(&guid)
}

/// Whether a gate's wall piece blocks now in copy `pf` (`None`: not a wall piece).
pub fn wall_blocks(pf: Pf, guid: u32) -> Option<bool> {
    let gate = *WALLS.get(&guid)?;
    gate_open(pf, gate).map(|open| !open)
}

/// The boss gates opened in each copy (copy, gate), until the copy is dropped.
static OPENED: LazyLock<Mutex<HashSet<(Pf, u32)>>> = LazyLock::new(Default::default);

/// Whether the enemy named `name` holds a gate shut on `playfield` (ambushes don't copy him).
pub fn holds_gate(playfield: Pf, name: &str) -> bool {
    BOSS_GATES.iter().any(|g| g.0 == playfield.model() && g.2 == name)
}

/// An enemy named `name` was defeated in copy `pf` (npc.rs): the gates it held open.
pub fn defeated(pf: Pf, name: &str) {
    for &(_, gate, _) in BOSS_GATES.iter().filter(|g| g.0 == pf.model() && g.2 == name) {
        if OPENED.lock().unwrap().insert((pf, gate)) {
            info!(playfield = ?pf, gate, boss = name, "gate opens: its boss fell");
            send_state(pf, gate, true);
        }
    }
}

/// Mirror buildie -> (copy, gate, spot, beam dynel).
static MIRRORS: LazyLock<Mutex<HashMap<Id32, (Pf, u32, &'static Spot, Id32)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static PRESENCES: LazyLock<Mutex<HashMap<Pf, Vec<world::Presence>>>> = LazyLock::new(Default::default);

pub fn start() {
    info!(mirrors = DATA.values().flatten().map(|g| g.mirrors.len()).sum::<usize>(), exit_gates = EXIT_GATES.len(), "light gates");
}

/// Puts the mirror buildies and their (dark) beam dynels into copy `pf` (Medusa's Lair).
pub fn spawn_world(pf: Pf) {
    let mut n = 0;
    for gate in DATA.get(&pf.model()).into_iter().flatten() {
        for spot in &gate.mirrors {
            let buildie = crate::buildie::add("XX_boss medusa mirror buildie", MIRROR_TEMPLATE, MIRROR_LOOK, pf, spot.pos, spot.rot);
            let beam = Id32::new(SIMPLE_DYNEL_TYPE, crate::instance::fixture(pf, BEAM_INSTANCES + n));
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
            PRESENCES.lock().unwrap().entry(pf).or_default().push(presence);
        }
    }
}

/// Takes copy `pf`'s beams out (its buildies go with the copy's, buildie.rs) and shuts its boss
/// gates again.
pub fn despawn_world(pf: Pf) {
    OPENED.lock().unwrap().retain(|(p, _)| *p != pf);
    MIRRORS.lock().unwrap().retain(|_, m| m.0 != pf);
    let gone = PRESENCES.lock().unwrap().remove(&pf);
    drop(gone);
}

/// A buildie was built: a mirror's beam comes on, and its gate opens once all its mirrors shine.
pub fn built(buildie: Id32) {
    let Some(&(pf, gate, spot, beam)) = MIRRORS.lock().unwrap().get(&buildie) else { return };
    info!(playfield = ?pf, gate, spot = spot.guid, "mirror built: light on the gate");
    world::send_playfield(beam, false, world::Out::Records(vec![crate::mirrors::beam_particle(beam, spot.length)]));
    if gate_open(pf, gate) == Some(true) {
        info!(playfield = ?pf, gate, "light gate opens");
        send_state(pf, gate, true);
    }
}

/// Shows a gate open or shut: its motion and collision, and its wall piece's.
fn send_state(pf: Pf, gate: u32, open: bool) {
    let id = crate::scenery::piece_id(pf, gate);
    world::send_playfield(id, false, world::Out::Records(open_records(id, gate, open)));
    for (&wall, _) in WALLS.iter().filter(|(_, g)| **g == gate) {
        let id = crate::scenery::piece_id(pf, wall);
        world::send_playfield(id, false, world::Out::Records(vec![crate::encounter::collision_record(id, !open)]));
    }
}

/// Whether a gate this module runs is open now in copy `pf` (`None`: not one of them).
fn gate_open(pf: Pf, guid: u32) -> Option<bool> {
    // room puzzles: the Gauntlet's plate room (plates.rs) and mirror room (mirrors.rs)
    if guid == crate::plates::GATE {
        return Some(crate::plates::solved(pf));
    }
    if guid == crate::tiki::GATE {
        return Some(crate::tiki::solved(pf));
    }
    if guid == crate::ballista::GATE {
        return Some(crate::ballista::solved(pf));
    }
    if guid == crate::keygate::GATE && pf.model() == crate::keygate::PLAYFIELD {
        return Some(crate::keygate::solved(pf));
    }
    if let Some(open) = crate::mirrors::gate_open(pf, guid) {
        return Some(open);
    }
    if BOSS_GATES.iter().any(|g| g.0 == pf.model() && g.1 == guid) {
        return Some(OPENED.lock().unwrap().contains(&(pf, guid)));
    }
    if let Some(&exit) = EXIT_GATES.get(&guid) {
        return Some(crate::encounter::door_hidden(pf, exit) == Some(true));
    }
    let mirrors = MIRRORS.lock().unwrap();
    let spots: Vec<Id32> = mirrors.iter().filter(|(_, m)| m.0 == pf && m.1 == guid).map(|(b, _)| *b).collect();
    drop(mirrors);
    (!spots.is_empty()).then(|| spots.iter().all(|b| crate::buildie::built(*b)))
}

/// Whether a gate blocks now in copy `pf` (scenery.rs spawns it with the gate's collision).
pub fn blocks(pf: Pf, guid: u32) -> bool {
    gate_open(pf, guid) == Some(false)
}

/// The collision gate `guid` has while it blocks (the Underworld's gate none: its wall piece
/// blocks).
pub fn collision(guid: u32) -> u32 {
    if guid == crate::keygate::GATE { 0 } else { GATE_COLLISION }
}

/// What a gate's spawn adds: its open pose, if it's open.
pub fn pose_records(pf: Pf, guid: u32) -> Vec<Bytes> {
    match gate_open(pf, guid) {
        Some(true) => vec![crate::effects::effect_package(crate::scenery::piece_id(pf, guid), OPEN)],
        _ => Vec::new(),
    }
}

/// The open motion (or the closed pose) and the collision to match.
fn open_records(id: Id32, guid: u32, open: bool) -> Vec<Bytes> {
    let motion = crate::effects::effect_package(id, if open { OPEN } else { crate::mirrors::TURN_MINUS });
    let collision = iir::record("SimpleDynelVisualChangeIIR_c", id, false, |w| w.u32(4).u32(if open { 0 } else { collision(guid) }));
    vec![motion, collision]
}

/// A room puzzle in copy `pf` was solved or reset: its gate opens or shuts to match.
pub fn refresh(pf: Pf, guid: u32) {
    if let Some(open) = gate_open(pf, guid) {
        send_state(pf, guid, open);
    }
}

/// An arena's doors changed in copy `pf` (encounter.rs): its exit gates open or shut with them.
pub fn arena_doors_changed(pf: Pf, doors: &[u32]) {
    for (&gate, exit) in EXIT_GATES.iter().filter(|(_, exit)| doors.contains(exit)) {
        send_state(pf, gate, crate::encounter::door_hidden(pf, *exit) == Some(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_have_their_walls() {
        // Medusa's Lair's four gates and the Gauntlet's five
        // and the Underworld's
        // and the jail's (behind the Jail Warden)
        for gate in [476102, 474105, 476024, 474103, 506181, 506102, 517927, 516809, 517465, crate::keygate::GATE, 576_082] {
            assert!(WALLS.values().any(|g| *g == gate), "gate {gate} has no wall piece");
        }
    }

    #[test]
    fn jail_gate_opens_when_the_warden_falls() {
        let pf = Pf::main(4610);
        let wall = WALLS.iter().find(|(_, g)| **g == 576_082).map(|(w, _)| *w).expect("jail gate wall");
        assert_eq!(wall, 576_750);
        assert!(blocks(pf, 576_082));
        assert_eq!(wall_blocks(pf, wall), Some(true));
        OPENED.lock().unwrap().insert((pf, 576_082));
        assert!(!blocks(pf, 576_082));
        assert_eq!(wall_blocks(pf, wall), Some(false));
        assert_eq!(pose_records(pf, 576_082).len(), 1);
        despawn_world(pf);
        assert!(blocks(pf, 576_082));
    }
}
