//! Medusa mirror puzzles (Armory 2310, Gauntlet 6300): data/mirror_puzzles.json
//! (tools/re/extract_mirror_puzzles.py). Each mirror turns between two positions when used; in
//! each it sends the light on to another mirror, a target or a wall. Lighting every target
//! solves the puzzle: the Armory's pedestal opens the barrier beside it, the Thunderbolt arena's
//! exit (encounter.rs), the Gauntlet's two gate markers open its gate (scenery.rs).
//!
//! The mirror graph is client data (the "+"/"-" markers beside each mirror); the starting
//! positions and the rewards were server script and are inferred (docs/TUNABLES.md "Mirror
//! puzzles"). The light itself isn't shown past the first mirror (the room's beam is level
//! geometry; neither a projectile from a simple dynel nor a particle on one shows). A turned
//! mirror stands a quarter turn round. One state per playfield, shared by everyone there; it
//! goes back to the start once nobody is left (a new instance, as live gave each group).

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::Duration,
};

use lmoxide_net::types::Id32;
use serde::Deserialize;
use tracing::info;

use crate::{scenery::SIMPLE_DYNEL_TYPE, world};

/// How often an empty playfield's puzzle is put back to the start.
const RESET_CHECK: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Outs {
    plus: Option<String>,
    minus: Option<String>,
}

#[derive(Deserialize)]
struct Mirror {
    guid: u32,
    name: String,
    out: Outs,
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

/// A piece's rotation now: a mirror at "+" stands a quarter turn from its placement ("-"); the
/// client neither plays a turn for `is_closed` nor takes a new rotation in a repeated full
/// update, so a turned mirror leaves and comes back (scenery::respawn).
pub fn rotation(guid: u32, placed: [f32; 4]) -> [f32; 4] {
    if closed(guid) != Some(false) {
        return placed;
    }
    // placed * (a quarter turn about the vertical axis)
    let (s, c) = std::f32::consts::FRAC_PI_4.sin_cos();
    let [x, y, z, w] = placed;
    [x * c - z * s, y * c + w * s, z * c + x * s, w * c - y * s]
}

/// Whether a puzzle gate is open now (its puzzle solved): hidden, as the arena doors open
/// (`is_closed` doesn't change how a gate looks).
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
    let (plus, newly_solved) = {
        let mut states = STATE.lock().unwrap();
        let s = states.entry(pf).or_default();
        if !s.plus.remove(&guid) {
            s.plus.insert(guid);
        }
        let reached = reached(puzzle, &s.plus);
        let newly_solved = !s.solved && puzzle.targets.iter().all(|t| reached.contains(t.name.as_str()));
        s.solved |= newly_solved;
        (s.plus.contains(&guid), newly_solved)
    };
    info!(?player, playfield = pf, mirror = %m.name, plus, "mirror turned");
    crate::scenery::respawn(guid);
    if newly_solved {
        info!(playfield = pf, "mirror puzzle solved");
        on_solved(pf, puzzle);
    }
}

/// The targets the light reaches, following it from every root mirror to mirror.
fn reached<'a>(puzzle: &'a Puzzle, plus: &HashSet<u32>) -> HashSet<&'a str> {
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
    reached
}

fn on_solved(pf: u32, puzzle: &Puzzle) {
    if let Some(gate) = puzzle.opens.gate {
        let id = Id32::new(SIMPLE_DYNEL_TYPE, gate);
        world::send_playfield(id, false, world::Out::Records(vec![crate::encounter::door_record(id, true)]));
    }
    if puzzle.opens.arena_exit.is_some() {
        crate::encounter::refresh_doors(pf);
    }
}

/// Puts the puzzles of empty playfields back to the start, now and then.
pub fn start() {
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
    STATE.lock().unwrap().insert(playfield, State { plus, solved: solve });
    // spawned again: turned as they are now
    for m in &puzzle.mirrors {
        crate::scenery::respawn(m.guid);
    }
    if solve {
        on_solved(playfield, puzzle);
    } else {
        if let Some(gate) = puzzle.opens.gate {
            let id = Id32::new(SIMPLE_DYNEL_TYPE, gate);
            world::send_playfield(id, false, world::Out::Records(vec![crate::encounter::door_record(id, false)]));
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
            let lit = reached(puzzle, plus);
            puzzle.targets.iter().all(|t| lit.contains(t.name.as_str()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_puzzle_has_a_solution_and_starts_unsolved() {
        for (pf, puzzle) in DATA.iter() {
            let lit = reached(puzzle, &HashSet::new());
            assert!(!puzzle.targets.iter().all(|t| lit.contains(t.name.as_str())), "{pf} starts solved");
            assert!(solution(puzzle).is_some(), "{pf} has no solution");
        }
    }
}
