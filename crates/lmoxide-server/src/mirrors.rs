//! Medusa mirror puzzles (Armory 2310, Gauntlet 6300): data/mirror_puzzles.json
//! (tools/re/extract_mirror_puzzles.py). Each mirror turns between two positions when used; in
//! each it sends the light on to another mirror, a target or a wall. Lighting every target
//! solves the puzzle: the Armory's pedestal opens the barrier beside it, the Thunderbolt arena's
//! exit (encounter.rs), the Gauntlet's two gate markers open its gate (scenery.rs).
//!
//! The mirror graph is client data (the "+"/"-" markers beside each mirror); the starting
//! positions and the rewards were server script and are inferred (docs/TUNABLES.md "Mirror
//! puzzles"). A mirror turns by its animation (`TURN_PLUS`/`TURN_MINUS`); the light shows as beam
//! particles from mirror to mirror (`BEAMS`). One state per playfield, shared by everyone there;
//! it goes back to the start once nobody is left (a new instance, as live gave each group).

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_net::types::Id32;
use serde::Deserialize;
use tracing::info;

use crate::{scenery::SIMPLE_DYNEL_TYPE, world};

/// How often an empty playfield's puzzle is put back to the start.
const RESET_CHECK: Duration = Duration::from_secs(30);
/// The light: `particle_generic_lightbeam_horizontal_<n>m` (RDB 1010207, n = 1..7; an "n m" beam
/// runs about 2n m along the dynel's facing, as the rooms' own beams do), shown on invisible
/// dynels at the "+"/"-" markers, as live put particles on its XX_Invisible dynels.
const BEAMS: [u32; 7] = [495960, 495959, 495958, 495957, 495956, 495961, 495962];
/// The beam dynels' instances: one per mirror and position, clear of the playfield guids.
const BEAM_INSTANCES: u32 = 0x00c0_0000;

#[derive(Deserialize)]
struct Outs {
    plus: Option<String>,
    minus: Option<String>,
}

/// Where the light leaves a mirror in one position: the marker's place and facing, and how far
/// it goes.
#[derive(Deserialize)]
struct Beam {
    pos: [f32; 3],
    rot: [f32; 4],
    length: f32,
}

#[derive(Deserialize)]
struct Beams {
    plus: Beam,
    minus: Beam,
}

#[derive(Deserialize)]
struct Mirror {
    guid: u32,
    name: String,
    out: Outs,
    beam: Beams,
}

#[derive(Deserialize)]
struct Target {
    name: String,
}

#[derive(Deserialize, Default)]
struct Opens {
    #[serde(default)]
    arena_exit: Option<String>,
    #[serde(default)]
    gate: Option<u32>,
}

#[derive(Deserialize)]
struct Puzzle {
    mirrors: Vec<Mirror>,
    roots: Vec<String>,
    targets: Vec<Target>,
    #[serde(default)]
    opens: Opens,
}

static DATA: LazyLock<HashMap<u32, Puzzle>> = LazyLock::new(|| {
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/mirror_puzzles.json")).expect("mirror_puzzles.json");
    raw.into_iter()
        .filter_map(|(pf, v)| Some((pf.parse().ok()?, serde_json::from_value(v).expect("mirror_puzzles.json puzzle"))))
        .collect()
});

/// A playfield's puzzle: the mirrors turned to "+" (all start at "-") and whether it's solved.
#[derive(Default, PartialEq)]
struct State {
    plus: HashSet<u32>,
    solved: bool,
}

static STATE: LazyLock<Mutex<HashMap<u32, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn find(guid: u32) -> Option<(u32, &'static Mirror)> {
    DATA.iter().find_map(|(&pf, p)| p.mirrors.iter().find(|m| m.guid == guid).map(|m| (pf, m)))
}

/// Whether a simple dynel is a puzzle mirror (spawned usable by scenery.rs).
pub fn is_mirror(guid: u32) -> bool {
    find(guid).is_some()
}

/// A mirror's `is_closed` now: "-" closed, "+" open (the client keeps the same pose either way).
pub fn closed(guid: u32) -> Option<bool> {
    let (pf, m) = find(guid)?;
    Some(!STATE.lock().unwrap().get(&pf).is_some_and(|s| s.plus.contains(&m.guid)))
}

/// The mirror's pose effect (blend setup 474600's `anim` parameter, effect packages in RDB
/// 1000622): 474576 sets `anim` 1, which plays `open` and rests in `anim_1` ("+"), 474575 sets
/// it 0 (`close`, `anim_0`, "-"). Levers and gates share the blend setup.
const TURN_PLUS: u32 = 474576;
pub const TURN_MINUS: u32 = 474575;

/// The records that put a mirror in its pose now (for its spawn: a mirror at "+" turns there).
pub fn pose_records(guid: u32) -> Vec<Bytes> {
    match closed(guid) {
        Some(false) => vec![crate::effects::effect_package(Id32::new(SIMPLE_DYNEL_TYPE, guid), TURN_PLUS)],
        _ => Vec::new(),
    }
}

fn plus_now(pf: u32, guid: u32) -> bool {
    STATE.lock().unwrap().get(&pf).is_some_and(|s| s.plus.contains(&guid))
}

/// Whether a puzzle's gate is open now (its puzzle solved; gates.rs runs it).
pub fn gate_open(guid: u32) -> Option<bool> {
    let (&pf, _) = DATA.iter().find(|(_, p)| p.opens.gate == Some(guid))?;
    Some(solved(pf))
}

/// Whether a solved mirror puzzle opens the exit of arena `name` on `playfield`.
pub fn opens_exit(playfield: u32, name: &str) -> bool {
    DATA.get(&playfield).is_some_and(|p| p.opens.arena_exit.as_deref() == Some(name)) && solved(playfield)
}

fn solved(playfield: u32) -> bool {
    STATE.lock().unwrap().get(&playfield).is_some_and(|s| s.solved)
}

/// `player` used mirror `guid`: it turns, the light follows, and lighting every target solves
/// the puzzle.
pub fn turn(player: Id32, guid: u32) {
    let Some((pf, m)) = find(guid) else { return };
    let puzzle = &DATA[&pf];
    let before = lit_beams(pf);
    let (plus, newly_solved) = {
        let mut states = STATE.lock().unwrap();
        let s = states.entry(pf).or_default();
        if !s.plus.remove(&guid) {
            s.plus.insert(guid);
        }
        let (reached, _) = reached(puzzle, &s.plus);
        let newly_solved = !s.solved && puzzle.targets.iter().all(|t| reached.contains(t.name.as_str()));
        s.solved |= newly_solved;
        (s.plus.contains(&guid), newly_solved)
    };
    info!(?player, playfield = pf, mirror = %m.name, plus, "mirror turned");
    let id = Id32::new(SIMPLE_DYNEL_TYPE, guid);
    let effect = crate::effects::effect_package(id, if plus { TURN_PLUS } else { TURN_MINUS });
    world::send_playfield(id, false, world::Out::Records(vec![effect]));
    show_beams(pf, &before);
    if newly_solved {
        info!(playfield = pf, "mirror puzzle solved");
        on_solved(pf, puzzle);
    }
}

/// The targets the light reaches and the mirrors it falls on, following it from every root
/// mirror to mirror.
fn reached<'a>(puzzle: &'a Puzzle, plus: &HashSet<u32>) -> (HashSet<&'a str>, HashSet<u32>) {
    let mirror = |name: &str| puzzle.mirrors.iter().find(|m| m.name == name);
    let (mut reached, mut seen) = (HashSet::new(), HashSet::new());
    for root in &puzzle.roots {
        let mut at = mirror(root);
        while let Some(m) = at {
            if !seen.insert(m.guid) {
                break;
            }
            let next = if plus.contains(&m.guid) { &m.out.plus } else { &m.out.minus };
            at = None;
            match next.as_deref() {
                Some(n) if let Some(t) = puzzle.targets.iter().find(|t| t.name == n) => {
                    reached.insert(t.name.as_str());
                }
                Some(n) => at = mirror(n),
                None => {}
            }
        }
    }
    (reached, seen)
}

/// The beams shown now on `pf`: (mirror, "+") for every mirror the light falls on.
fn lit_beams(pf: u32) -> HashSet<(u32, bool)> {
    let plus = STATE.lock().unwrap().get(&pf).map(|s| s.plus.clone()).unwrap_or_default();
    reached(&DATA[&pf], &plus).1.into_iter().map(|g| (g, plus.contains(&g))).collect()
}

/// Starts the beams that came on and stops those that went out since `before`.
fn show_beams(pf: u32, before: &HashSet<(u32, bool)>) {
    let now = lit_beams(pf);
    for &(guid, plus) in now.difference(before) {
        let id = beam_id(guid, plus);
        world::send_playfield(id, false, world::Out::Records(vec![beam_record(id, guid, plus)]));
    }
    for &(guid, plus) in before.difference(&now) {
        let id = beam_id(guid, plus);
        world::send_playfield(id, false, world::Out::Records(vec![beam_stop_record(id)]));
    }
}

/// Mirror and position -> its beam dynel.
static BEAM_IDS: LazyLock<HashMap<(u32, bool), Id32>> = LazyLock::new(|| {
    let mut guids: Vec<u32> = DATA.values().flat_map(|p| p.mirrors.iter().map(|m| m.guid)).collect();
    guids.sort();
    guids
        .into_iter()
        .enumerate()
        .flat_map(|(i, g)| [false, true].map(|plus| ((g, plus), Id32::new(SIMPLE_DYNEL_TYPE, BEAM_INSTANCES + 2 * i as u32 + u32::from(plus)))))
        .collect()
});

fn beam_id(guid: u32, plus: bool) -> Id32 {
    BEAM_IDS[&(guid, plus)]
}

fn beam_of(guid: u32, plus: bool) -> &'static Beam {
    let (_, m) = find(guid).expect("puzzle mirror");
    if plus { &m.beam.plus } else { &m.beam.minus }
}

/// `SimpleDynelVisualChangeIIR_c` state 0: a looping particle (u32 particle, i32 attractor 35 as
/// live always sent, f32 yaw, offset x, y, z in cm), the beam as long as the light goes.
fn beam_record(id: Id32, guid: u32, plus: bool) -> Bytes {
    beam_particle(id, beam_of(guid, plus).length)
}

/// The light beam particle that spans `length` m along dynel `id`'s facing (gates.rs too).
pub fn beam_particle(id: Id32, length: f32) -> Bytes {
    let n = (length / 2.0).round().clamp(1.0, BEAMS.len() as f32) as usize;
    lmoxide_net::iir::record("SimpleDynelVisualChangeIIR_c", id, false, |w| w.u32(0).u32(BEAMS[n - 1]).i32(35).f32(0.0).f32(0.0).f32(0.0).f32(0.0))
}

/// State 2: stops the dynel's first particle.
fn beam_stop_record(id: Id32) -> Bytes {
    lmoxide_net::iir::record("SimpleDynelVisualChangeIIR_c", id, false, |w| w.u32(2).u32(0))
}

static BEAM_PRESENCES: Mutex<Vec<world::Presence>> = Mutex::new(Vec::new());

/// Puts the beam dynels into their worlds (each shows its beam while the light takes it).
fn spawn_beams() {
    let mut presences = BEAM_PRESENCES.lock().unwrap();
    for (&pf, puzzle) in DATA.iter() {
        for m in &puzzle.mirrors {
            for plus in [false, true] {
                let (id, guid, beam) = (beam_id(m.guid, plus), m.guid, beam_of(m.guid, plus));
                let (outbox, mut inbox) = tokio::sync::mpsc::unbounded_channel();
                tokio::spawn(async move { while inbox.recv().await.is_some() {} });
                let pos = std::sync::Arc::new(Mutex::new(beam.pos));
                presences.push(world::join(
                    id,
                    pf,
                    pos,
                    move || {
                        let mut records = crate::scenery::invisible_records(id, pf, beam.pos, beam.rot);
                        if lit_beams(pf).contains(&(guid, plus)) {
                            records.push(beam_record(id, guid, plus));
                        }
                        records
                    },
                    outbox,
                ));
            }
        }
    }
}

fn on_solved(pf: u32, puzzle: &Puzzle) {
    if let Some(gate) = puzzle.opens.gate {
        crate::gates::refresh(gate);
    }
    if puzzle.opens.arena_exit.is_some() {
        crate::encounter::refresh_doors(pf);
    }
}

/// Puts the puzzles of empty playfields back to the start, now and then.
pub fn start() {
    spawn_beams();
    tokio::spawn(async {
        let mut tick = tokio::time::interval(RESET_CHECK);
        loop {
            tick.tick().await;
            for &pf in DATA.keys() {
                let empty = !world::positions(pf).iter().any(|(id, _)| crate::npc::is_player(*id));
                let reset = empty && STATE.lock().unwrap().remove(&pf).is_some_and(|s| s != State::default());
                if reset {
                    info!(playfield = pf, "mirror puzzle reset");
                }
            }
        }
    });
}

/// Test command: turns the playfield's mirrors to the solution (or back to the start).
pub fn set_solved(playfield: u32, solve: bool) -> bool {
    let Some(puzzle) = DATA.get(&playfield) else { return false };
    let plus = if solve {
        let Some(plus) = solution(puzzle) else { return false };
        plus
    } else {
        HashSet::new()
    };
    let before = lit_beams(playfield);
    STATE.lock().unwrap().insert(playfield, State { plus, solved: solve });
    show_beams(playfield, &before);
    for m in &puzzle.mirrors {
        let id = Id32::new(SIMPLE_DYNEL_TYPE, m.guid);
        let effect = crate::effects::effect_package(id, if plus_now(playfield, m.guid) { TURN_PLUS } else { TURN_MINUS });
        world::send_playfield(id, false, world::Out::Records(vec![effect]));
    }
    if solve {
        on_solved(playfield, puzzle);
    } else {
        if let Some(gate) = puzzle.opens.gate {
            crate::gates::refresh(gate);
        }
        crate::encounter::refresh_doors(playfield);
    }
    true
}

/// The mirrors to turn to "+" so the light reaches every target: tried in every combination
/// (2^12 at most).
fn solution(puzzle: &Puzzle) -> Option<HashSet<u32>> {
    (0u32..1 << puzzle.mirrors.len())
        .map(|bits| puzzle.mirrors.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, m)| m.guid).collect::<HashSet<u32>>())
        .find(|plus| {
            let lit = reached(puzzle, plus).0;
            puzzle.targets.iter().all(|t| lit.contains(t.name.as_str()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_puzzle_has_a_solution_and_starts_unsolved() {
        for (pf, puzzle) in DATA.iter() {
            let lit = reached(puzzle, &HashSet::new()).0;
            assert!(!puzzle.targets.iter().all(|t| lit.contains(t.name.as_str())), "{pf} starts solved");
            assert!(solution(puzzle).is_some(), "{pf} has no solution");
        }
    }
}
