//! The Gauntlet's shooting room (6300, "Quest_ShootingRoom"): two troll ballistas ("ballista N
//! low" / "ballista N high", the same ballista aimed low or high) with a lever each
//! (`lego_lever_a_interactable`). Target dummies pop up at the ballistas' aim points ("ballista N
//! low/high particle"); pulling a lever fires its ballista ("XX_Ballista Arrow Projectile") at
//! the point it aims at now, and a dummy there falls. Four hits open the room's east gate
//! (gates.rs). The user's memory of live (2026-10-25: levers fire the ballistas at targets that
//! pop up); the pieces are client data, the dummies ("XX_Combat Dummy"), the aiming cycle, the
//! timing and the count are inferred (docs/TUNABLES.md "Gauntlet"). Reset once nobody is left in
//! the Gauntlet.

use crate::instance::Pf;
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use lmoxide_net::types::Id32;
use serde::Deserialize;
use tracing::info;

use crate::world;

pub const PLAYFIELD: u32 = 6300;
/// The gate the hits open (the shooting room's east gate).
pub const GATE: u32 = 516809;
const HITS_NEEDED: usize = 4;
/// A ballista swings between low and high aim every `AIM_PERIOD` (the two apart by half).
const AIM_PERIOD: Duration = Duration::from_secs(3);
/// A dummy pops up every `TARGET_PERIOD` for `TARGET_LIFE` while someone is in the room.
const TARGET_PERIOD: Duration = Duration::from_secs(4);
const TARGET_LIFE: Duration = Duration::from_secs(6);
const DUMMY_TEMPLATE: u32 = 320512;
const DUMMY_LOOK: u32 = 320511;
/// The arrow: projectile item and speed; the hit lands when it arrives.
const ARROW: u32 = 506676;
const ARROW_SPEED: f32 = 25.0;
/// The room (x, z): from the west gate to the east gate.
const ROOM: ([f32; 2], [f32; 2]) = ([288.0, 320.0], [160.0, 195.0]);
const RESET_CHECK: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Piece {
    name: String,
    pos: [f32; 3],
    guid: u32,
}

/// A ballista: its lever, its low and high pieces and their aim points.
struct Ballista {
    lever: u32,
    low: u32,
    high: u32,
    pos: [f32; 3],
    aims: [[f32; 3]; 2],
}

static BALLISTAS: LazyLock<Vec<Ballista>> = LazyLock::new(|| {
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
    let pieces: Vec<Piece> = worlds
        .get(&PLAYFIELD.to_string())
        .and_then(|w| w.get("simple_dynels"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|d| serde_json::from_value(d.clone()).ok()).collect())
        .unwrap_or_default();
    let find = |name: String| pieces.iter().find(|p| p.name == name);
    (1..=2)
        .filter_map(|n| {
            let low = find(format!("ballista {n} low"))?;
            let high = find(format!("ballista {n} high"))?;
            let lever = pieces
                .iter()
                .filter(|p| p.name == "lego_lever_a_interactable")
                .min_by(|a, b| dist(a.pos, low.pos).total_cmp(&dist(b.pos, low.pos)))?;
            let aims = [find(format!("ballista {n} low particle"))?.pos, find(format!("ballista {n} high particle"))?.pos];
            Some(Ballista { lever: lever.guid, low: low.guid, high: high.guid, pos: low.pos, aims })
        })
        .collect()
});

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[derive(Default)]
struct State {
    /// per ballista: aiming high now
    high: Vec<bool>,
    /// the dummy up now: (ballista, high, id)
    dummy: Option<(usize, bool, Id32)>,
    hits: usize,
    /// which aim point the next dummy takes
    turn: usize,
}

/// Each copy's room (by the copy of the Gauntlet).
static STATE: LazyLock<Mutex<HashMap<Pf, State>>> = LazyLock::new(Default::default);

/// Runs `f` on copy `pf`'s room state.
fn with<T>(pf: Pf, f: impl FnOnce(&mut State) -> T) -> T {
    let mut states = STATE.lock().unwrap();
    let state = states.entry(pf).or_insert_with(|| State { high: vec![false; BALLISTAS.len()], ..Default::default() });
    f(state)
}

/// Whether scenery.rs spawns this piece (the ballistas; the levers it spawns anyway).
pub fn is_ballista(playfield: u32, name: &str) -> bool {
    playfield == PLAYFIELD && name.starts_with("ballista ") && !name.ends_with("particle")
}

/// Whether a simple dynel is a ballista's lever (usable).
pub fn is_lever(guid: u32) -> bool {
    BALLISTAS.iter().any(|b| b.lever == guid)
}

/// Whether a ballista piece is hidden now in copy `pf` (the aim it doesn't have).
pub fn hidden(pf: Pf, guid: u32) -> Option<bool> {
    let (i, b) = BALLISTAS.iter().enumerate().find(|(_, b)| guid == b.low || guid == b.high)?;
    let high = with(pf, |s| s.high.get(i).copied().unwrap_or(false));
    Some(if guid == b.low { high } else { !high })
}

/// Whether the room's gate is open in copy `pf`.
pub fn solved(pf: Pf) -> bool {
    with(pf, |s| s.hits >= HITS_NEEDED)
}

/// The copies of the Gauntlet whose room someone is in (not the world editor's).
fn occupied() -> Vec<Pf> {
    crate::worlds::copies_of(PLAYFIELD)
        .into_iter()
        .filter(|pf| !crate::instance::rules(*pf).editor)
        .filter(|pf| {
            world::positions(*pf).iter().any(|(id, p)| {
                crate::npc::is_player(*id) && (ROOM.0[0]..=ROOM.0[1]).contains(&p[0]) && (ROOM.1[0]..=ROOM.1[1]).contains(&p[2])
            })
        })
        .collect()
}

fn show_aim(pf: Pf, i: usize, high: bool) {
    let b = &BALLISTAS[i];
    let (low_id, high_id) = (crate::scenery::piece_id(pf, b.low), crate::scenery::piece_id(pf, b.high));
    let records = vec![crate::encounter::door_record(low_id, high), crate::encounter::door_record(high_id, !high)];
    world::send_playfield(low_id, false, world::Out::Records(records));
}

/// Runs the aiming and the dummies in each copy of the room someone is in.
pub fn start() {
    for i in 0..BALLISTAS.len() {
        tokio::spawn(async move {
            tokio::time::sleep(AIM_PERIOD / 2 * i as u32).await;
            let mut tick = tokio::time::interval(AIM_PERIOD);
            loop {
                tick.tick().await;
                for pf in occupied().into_iter().filter(|pf| !solved(*pf)) {
                    let high = with(pf, |s| {
                        s.high[i] = !s.high[i];
                        s.high[i]
                    });
                    show_aim(pf, i, high);
                }
            }
        });
    }
    tokio::spawn(async {
        let mut tick = tokio::time::interval(TARGET_PERIOD);
        loop {
            tick.tick().await;
            if BALLISTAS.is_empty() {
                continue;
            }
            for pf in occupied().into_iter().filter(|pf| !solved(*pf)) {
                let Some(turn) = with(pf, |s| s.dummy.is_none().then(|| {
                    s.turn += 1;
                    s.turn - 1
                })) else {
                    continue;
                };
                // each aim point in turn: ballista 1 low, 2 high, 1 high, 2 low, ...
                let (i, high) = (turn % BALLISTAS.len(), (turn / BALLISTAS.len() + turn) % 2 == 1);
                let def = crate::npc::SpawnDef {
                variants: Vec::new(),
                    kind: crate::npc::Kind::Smashable,
                    template: DUMMY_TEMPLATE,
                    name: "XX_Combat Dummy".into(),
                    monster_data: DUMMY_LOOK,
                    level: 1,
                    health: 1,
                    pos: BALLISTAS[i].aims[usize::from(high)],
                    spell: 0,
                    cars_group: Some(5),
                    npc_template: None,
                    reach: None,
                    anchor: None,
                    fights: false,
                    faction: None,
                    patrol: Vec::new(),
                    loot: None,
                    source: None,
                    rot: None,
                };
                let id = crate::npc::spawn_temporary(def, pf, TARGET_LIFE);
                info!(playfield = ?pf, ballista = i + 1, high, "dummy up");
                with(pf, |s| s.dummy = Some((i, high, id)));
                tokio::spawn(async move {
                    tokio::time::sleep(TARGET_LIFE).await;
                    with(pf, |s| {
                        if s.dummy.is_some_and(|d| d.2 == id) {
                            s.dummy = None;
                        }
                    });
                });
            }
        }
    });
    tokio::spawn(async {
        let mut tick = tokio::time::interval(RESET_CHECK);
        loop {
            tick.tick().await;
            for pf in crate::worlds::copies_of(PLAYFIELD) {
                let empty = !world::positions(pf).iter().any(|(id, _)| crate::npc::is_player(*id));
                if empty && with(pf, |s| std::mem::take(&mut s.hits)) > 0 {
                    info!(playfield = ?pf, "shooting room reset");
                    crate::gates::refresh(pf, GATE);
                }
            }
        }
    });
}

/// `player` pulled lever `guid`: the lever and the ballista move, its arrow flies at the point
/// it aims at, and a dummy there falls (in the player's copy of the room).
pub fn pull(player: Id32, guid: u32) {
    let Some(i) = BALLISTAS.iter().position(|b| b.lever == guid) else { return };
    let Some(pf) = world::whereabouts(player).map(|(pf, _)| pf).filter(|pf| pf.model() == PLAYFIELD) else { return };
    let b = &BALLISTAS[i];
    let high = with(pf, |s| s.high[i]);
    let lever = crate::scenery::piece_id(pf, b.lever);
    let ballista = crate::scenery::piece_id(pf, if high { b.high } else { b.low });
    let open = |id| crate::effects::effect_package(id, crate::mirrors::TURN_PLUS);
    world::send_playfield(lever, false, world::Out::Records(vec![open(lever), open(ballista)]));
    let to = b.aims[usize::from(high)];
    let from = [b.pos[0], b.pos[1] + 0.5, b.pos[2]];
    let (shot, flight) = crate::projectile::launch_enemy(ballista, pf, 0, ARROW, 1, ARROW_SPEED, 0, from, to, None);
    info!(?player, ballista = i + 1, high, "ballista fired");
    tokio::spawn(async move {
        tokio::time::sleep(flight).await;
        let target = with(pf, |s| match s.dummy {
            Some((d, h, id)) if d == i && h == high => {
                s.dummy = None;
                s.hits += 1;
                Some((id, s.hits))
            }
            _ => None,
        });
        crate::projectile::enemy_impact(ballista, shot, None);
        if let Some((id, hits)) = target {
            info!(hits, "ballista hit a dummy");
            crate::npc::hit(player, id, 0, 100);
            if hits == HITS_NEEDED {
                info!(playfield = ?pf, "shooting room solved");
                crate::gates::refresh(pf, GATE);
            }
        }
        let close = |id| crate::effects::effect_package(id, crate::mirrors::TURN_MINUS);
        world::send_playfield(lever, false, world::Out::Records(vec![close(lever), close(ballista)]));
    });
}
