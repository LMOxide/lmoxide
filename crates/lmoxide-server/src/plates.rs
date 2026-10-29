//! The Gauntlet's plate room (6300): 30 floor plates (`ruined_fort_floor_plate_2x2_b_medieval`,
//! each with a `_c_` twin at the same spot, the lit look); stepping on a plate lights it, and
//! with every plate lit the room's gate opens (gates.rs). The user's memory of live
//! (2026-10-25: "walk on all tiles to turn them green"); the plates and the room's areas
//! (Quest_PressurePlateRoom, BA_TikiRoom_Plate1-30, 2.5 m apart) are client data, the rule is
//! inferred. One state for the playfield, reset once nobody is left (as mirrors.rs).

use crate::instance::Pf;
use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::Duration,
};

use serde::Deserialize;
use tracing::info;

use crate::world;

pub const PLAYFIELD: u32 = 6300;
const UNLIT: &str = "ruined_fort_floor_plate_2x2_b_medieval";
const LIT: &str = "ruined_fort_floor_plate_2x2_c_medieval";
/// The gate the plates open (the plate room's east gate).
pub const GATE: u32 = 517465;
/// Half a plate's side (the plates are 2.5 m tiles) and how far above it the player may be.
const HALF: f32 = 1.25;
const ABOVE: f32 = 2.0;
const RESET_CHECK: Duration = Duration::from_secs(30);
/// A click-to-move path is checked every this many metres, at about the run speed (jumppad.rs).
const STEP: f32 = 0.5;
const WALK_SPEED: f32 = 6.0;

#[derive(Deserialize)]
struct Piece {
    name: String,
    pos: [f32; 3],
    guid: u32,
}

/// The plates: (unlit guid, lit guid, position).
static PLATES: LazyLock<Vec<(u32, u32, [f32; 3])>> = LazyLock::new(|| {
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
    let pieces: Vec<Piece> = worlds
        .get(&PLAYFIELD.to_string())
        .and_then(|w| w.get("simple_dynels"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|d| serde_json::from_value(d.clone()).ok()).collect())
        .unwrap_or_default();
    pieces
        .iter()
        .filter(|p| p.name == UNLIT)
        .filter_map(|u| {
            let lit = pieces.iter().find(|l| l.name == LIT && (l.pos[0] - u.pos[0]).abs() < 0.1 && (l.pos[2] - u.pos[2]).abs() < 0.1)?;
            Some((u.guid, lit.guid, u.pos))
        })
        .collect()
});

/// The plates lit (by unlit guid), per copy of the Gauntlet.
static LIT_NOW: LazyLock<Mutex<HashMap<Pf, HashSet<u32>>>> = LazyLock::new(Default::default);

/// Whether scenery.rs spawns this piece (the plates of the plate room).
pub fn is_plate(playfield: u32, name: &str) -> bool {
    playfield == PLAYFIELD && (name == UNLIT || name == LIT)
}

/// Whether a plate piece is hidden now in copy `pf`: the lit look until its plate is lit, the
/// unlit after.
pub fn hidden(pf: Pf, guid: u32) -> Option<bool> {
    let lit_now = LIT_NOW.lock().unwrap();
    let lit = |u: u32| lit_now.get(&pf).is_some_and(|l| l.contains(&u));
    PLATES.iter().find_map(|&(u, l, _)| {
        if guid == u {
            Some(lit(u))
        } else if guid == l {
            Some(!lit(u))
        } else {
            None
        }
    })
}

/// Whether the plates' gate is open in copy `pf` (every plate lit).
pub fn solved(pf: Pf) -> bool {
    !PLATES.is_empty() && LIT_NOW.lock().unwrap().get(&pf).is_some_and(|l| l.len() == PLATES.len())
}

/// A player moved to `pos` (keyboard), or walks a click-to-move path from `from` to `pos`
/// (only its ends are reported): the plates under them light, along the path as they get there.
pub fn moved(playfield: Pf, from: Option<[f32; 3]>, pos: [f32; 3]) {
    if playfield.model() != PLAYFIELD || crate::instance::rules(playfield).editor {
        return;
    }
    let Some(from) = from else {
        step(playfield, pos);
        return;
    };
    let length = ((pos[0] - from[0]).powi(2) + (pos[2] - from[2]).powi(2)).sqrt();
    let steps = (length / STEP).ceil().max(1.0) as usize;
    tokio::spawn(async move {
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            tokio::time::sleep(Duration::from_secs_f32(length / steps as f32 / WALK_SPEED)).await;
            step(playfield, [from[0] + (pos[0] - from[0]) * t, from[1] + (pos[1] - from[1]) * t, from[2] + (pos[2] - from[2]) * t]);
        }
    });
}

/// Lights the plate at `pos` in copy `pf`, if any.
fn step(pf: Pf, pos: [f32; 3]) {
    let Some(&(unlit, lit, _)) = PLATES
        .iter()
        .find(|(_, _, p)| (pos[0] - p[0]).abs() <= HALF && (pos[2] - p[2]).abs() <= HALF && pos[1] >= p[1] - 0.5 && pos[1] <= p[1] + ABOVE)
    else {
        return;
    };
    let (new, done) = {
        let mut lit_now = LIT_NOW.lock().unwrap();
        let lit_now = lit_now.entry(pf).or_default();
        let new = lit_now.insert(unlit);
        (new, new && lit_now.len() == PLATES.len())
    };
    if !new {
        return;
    }
    let piece = |g| crate::scenery::piece_id(pf, g);
    let records = vec![crate::encounter::door_record(piece(unlit), true), crate::encounter::door_record(piece(lit), false)];
    world::send_playfield(piece(unlit), false, world::Out::Records(records));
    if done {
        info!(playfield = ?pf, "plate room solved");
        crate::gates::refresh(pf, GATE);
    }
}

/// Test command: lights every plate of copy `pf` (or none).
pub fn set_all(pf: Pf, lit: bool) {
    {
        let mut lit_now = LIT_NOW.lock().unwrap();
        let lit_now = lit_now.entry(pf).or_default();
        lit_now.clear();
        if lit {
            lit_now.extend(PLATES.iter().map(|p| p.0));
        }
    }
    let piece = |g| crate::scenery::piece_id(pf, g);
    let records: Vec<_> =
        PLATES.iter().flat_map(|&(unlit, l, _)| [crate::encounter::door_record(piece(unlit), lit), crate::encounter::door_record(piece(l), !lit)]).collect();
    if let Some(&(first, ..)) = PLATES.first() {
        world::send_playfield(piece(first), false, world::Out::Records(records));
    }
    crate::gates::refresh(pf, GATE);
}

/// Puts the plates back once nobody is left in a copy of the Gauntlet.
pub fn start() {
    tokio::spawn(async {
        let mut tick = tokio::time::interval(RESET_CHECK);
        loop {
            tick.tick().await;
            for pf in crate::worlds::copies_of(PLAYFIELD) {
                let empty = !world::positions(pf).iter().any(|(id, _)| crate::npc::is_player(*id));
                if empty && LIT_NOW.lock().unwrap().remove(&pf).is_some_and(|l| !l.is_empty()) {
                    info!(playfield = ?pf, "plate room reset");
                }
            }
        }
    });
}
