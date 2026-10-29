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

use crate::{scenery::SIMPLE_DYNEL_TYPE, world};

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
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State { high: vec![false; BALLISTAS.len()], ..Default::default() }));

/// Whether scenery.rs spawns this piece (the ballistas; the levers it spawns anyway).
pub fn is_ballista(playfield: u32, name: &str) -> bool {
    playfield == PLAYFIELD && name.starts_with("ballista ") && !name.ends_with("particle")
}

/// Whether a simple dynel is a ballista's lever (usable).
pub fn is_lever(guid: u32) -> bool {
    BALLISTAS.iter().any(|b| b.lever == guid)
}

/// Whether a ballista piece is hidden now (the aim it doesn't have).
pub fn hidden(guid: u32) -> Option<bool> {
    let state = STATE.lock().unwrap();
    BALLISTAS.iter().enumerate().find_map(|(i, b)| {
        let high = state.high.get(i).copied().unwrap_or(false);
        if guid == b.low {
            Some(high)
        } else if guid == b.high {
            Some(!high)
        } else {
            None
        }
    })
}

/// Whether the room's gate is open.
pub fn solved() -> bool {
    STATE.lock().unwrap().hits >= HITS_NEEDED
}

fn occupied() -> bool {
    world::positions(Pf::main(PLAYFIELD)).iter().any(|(id, p)| {
        crate::npc::is_player(*id) && (ROOM.0[0]..=ROOM.0[1]).contains(&p[0]) && (ROOM.1[0]..=ROOM.1[1]).contains(&p[2])
    })
}

fn show_aim(i: usize, high: bool) {
    let b = &BALLISTAS[i];
    let records = vec![
        crate::encounter::door_record(Id32::new(SIMPLE_DYNEL_TYPE, b.low), high),
        crate::encounter::door_record(Id32::new(SIMPLE_DYNEL_TYPE, b.high), !high),
    ];
    world::send_playfield(Id32::new(SIMPLE_DYNEL_TYPE, b.low), false, world::Out::Records(records));
}

/// Runs the aiming and the dummies.
pub fn start() {
    for i in 0..BALLISTAS.len() {
        tokio::spawn(async move {
            tokio::time::sleep(AIM_PERIOD / 2 * i as u32).await;
            let mut tick = tokio::time::interval(AIM_PERIOD);
            loop {
                tick.tick().await;
                if solved() || !occupied() {
                    continue;
                }
                let high = {
                    let mut state = STATE.lock().unwrap();
                    state.high[i] = !state.high[i];
                    state.high[i]
                };
                show_aim(i, high);
            }
        });
    }
    tokio::spawn(async {
        let mut tick = tokio::time::interval(TARGET_PERIOD);
        let mut turn = 0usize;
        loop {
            tick.tick().await;
            if solved() || !occupied() || STATE.lock().unwrap().dummy.is_some() || BALLISTAS.is_empty() {
                continue;
            }
            // each aim point in turn: ballista 1 low, 2 high, 1 high, 2 low, ...
            let (i, high) = (turn % BALLISTAS.len(), (turn / BALLISTAS.len() + turn) % 2 == 1);
            turn += 1;
            let def = crate::npc::SpawnDef {
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
            let id = crate::npc::spawn_temporary(def, Pf::main(PLAYFIELD), TARGET_LIFE);
            info!(ballista = i + 1, high, "dummy up");
            STATE.lock().unwrap().dummy = Some((i, high, id));
            tokio::spawn(async move {
                tokio::time::sleep(TARGET_LIFE).await;
                let mut state = STATE.lock().unwrap();
                if state.dummy.is_some_and(|d| d.2 == id) {
                    state.dummy = None;
                }
            });
        }
    });
    tokio::spawn(async {
        let mut tick = tokio::time::interval(RESET_CHECK);
        loop {
            tick.tick().await;
            let empty = !world::positions(Pf::main(PLAYFIELD)).iter().any(|(id, _)| crate::npc::is_player(*id));
            let mut state = STATE.lock().unwrap();
            if empty && state.hits > 0 {
                state.hits = 0;
                info!("shooting room reset");
                drop(state);
                crate::gates::refresh(GATE);
            }
        }
    });
}

/// `player` pulled lever `guid`: the lever and the ballista move, its arrow flies at the point
/// it aims at, and a dummy there falls.
pub fn pull(player: Id32, guid: u32) {
    let Some(i) = BALLISTAS.iter().position(|b| b.lever == guid) else { return };
    let b = &BALLISTAS[i];
    let high = STATE.lock().unwrap().high[i];
    let lever = Id32::new(SIMPLE_DYNEL_TYPE, b.lever);
    let ballista = Id32::new(SIMPLE_DYNEL_TYPE, if high { b.high } else { b.low });
    let open = |id| crate::effects::effect_package(id, crate::mirrors::TURN_PLUS);
    world::send_playfield(lever, false, world::Out::Records(vec![open(lever), open(ballista)]));
    let to = b.aims[usize::from(high)];
    let from = [b.pos[0], b.pos[1] + 0.5, b.pos[2]];
    let (shot, flight) = crate::projectile::launch_enemy(ballista, Pf::main(PLAYFIELD), 0, ARROW, 1, ARROW_SPEED, 0, from, to, None);
    info!(?player, ballista = i + 1, high, "ballista fired");
    tokio::spawn(async move {
        tokio::time::sleep(flight).await;
        let target = {
            let mut state = STATE.lock().unwrap();
            match state.dummy {
                Some((d, h, id)) if d == i && h == high => {
                    state.dummy = None;
                    state.hits += 1;
                    Some((id, state.hits))
                }
                _ => None,
            }
        };
        crate::projectile::enemy_impact(ballista, shot, None);
        if let Some((id, hits)) = target {
            info!(hits, "ballista hit a dummy");
            crate::npc::hit(player, id, 0, 100);
            if hits == HITS_NEEDED {
                info!("shooting room solved");
                crate::gates::refresh(GATE);
            }
        }
        let close = |id| crate::effects::effect_package(id, crate::mirrors::TURN_MINUS);
        world::send_playfield(lever, false, world::Out::Records(vec![close(lever), close(ballista)]));
    });
}
