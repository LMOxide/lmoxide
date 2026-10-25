//! Jump pads (mortars, wind fans, catapults...): step on one and it throws you to its landing
//! point.
//!
//! The client sends nothing for a pad; the original server noticed the player standing on it
//! and drove the whole jump (seen in the recorded session): an effect on the pad, a `StopIIR_c`,
//! a ballistic `MoveTaskIIR_c` to the landing point and the "jumppad lock" buff, then at the end
//! a landing effect and a `StopIIR_c` at the landing point. Pads and landing points come from
//! data/world_content.json (`kind: "jump_pad"`, tools/re/extract_world_content.py); outside
//! the recorded worlds the landing points are guesses.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{ProtoGameCodeDynelIir, ProtoQuat, ProtoSimpleDynelIir, ProtoVec3},
    types::Id32,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    effects,
    game::{create_game_object, int32_stat},
    world,
};

/// Dynel type of the pads (as in the recorded session).
const PAD_TYPE: u32 = 51320;
/// The pads' template item ("Jumper") and client script.
const PAD_TEMPLATE: u32 = 86651;
const PAD_CLIENT_SCRIPT: u32 = 601600;
const CARS_GROUP: u32 = 3;
const CARS_INACTIVE: u32 = 5;
const STAT_CARS_GROUP: u32 = 1050;
/// How close (horizontally) the player must come to a pad's centre (assumed; the original
/// server launched within ~0.5 m, see docs/TUNABLES.md).
const TRIGGER_RADIUS: f32 = 1.0;
const TRIGGER_HEIGHT: f32 = 2.0;
/// Click-to-move only reports the path's end: a pad there triggers once the player has walked
/// to it at about this speed (assumed).
const WALK_SPEED: f32 = 6.0;
const GRAVITY: f32 = 9.8;
const DEFAULT_APEX: f32 = 4.0;
/// "XX_system jumppad lock": no control while flying.
const LOCK_BUFF: u32 = 73527;
/// Launch effects on the pad: mortar (also used for pads with other looks) and wind fan.
const MORTAR_EFFECT: u32 = 367572;
const FAN_EFFECT: u32 = 102631;
/// Landing effect on the player, shortly before touching down.
const LANDING_EFFECT: u32 = 102632;
/// The plain invisible visual of markers and invisible jumpers.
const INVISIBLE_VISUAL: u32 = 95472;
/// Most pads one jump may chain through (the longest route, 6200's PondToSwamp, has 4).
const MAX_HOPS: usize = 8;

#[derive(Debug, Clone, Deserialize)]
struct Pad {
    #[serde(default)]
    name: String,
    pos: [f32; 3],
    rot: [f32; 4],
    /// [resource type, visual id]
    visual: [u32; 2],
    bounds: [f32; 6],
    guid: u32,
    #[serde(default)]
    look: Option<String>,
    target: [f32; 3],
    #[serde(default)]
    apex: Option<f32>,
    #[serde(default)]
    target_by: Option<String>,
    /// "JUMP" on pads; the mortars of 4300's cannonball course (K28B) and an idle fan (28EI)
    /// look like pads but are scenery.
    #[serde(default)]
    tag: Option<String>,
}

impl Pad {
    fn id(&self) -> Id32 {
        Id32::new(PAD_TYPE, self.guid)
    }

    /// Whether landing on this pad throws the player on at once: the invisible jumpers (the
    /// clouds in 2210) and the middle stops of named routes (6200's SwampToVolcano1, 2, End;
    /// 1610's jumpie_1_chain_N). Other pads someone lands on fire only after a step off
    /// (5200's catapult pairs land you on the way back). Lily pads float on water, where
    /// nobody can step off (2210's lily crossings).
    fn throws_on(&self) -> bool {
        self.visual[1] == INVISIBLE_VISUAL
            || self.target_by.as_deref() == Some("chain")
            || self.name.contains("chain")
            || self.name.contains("lily")
    }

    fn launches(&self) -> bool {
        self.tag.as_deref() == Some("JUMP")
    }

    fn effect(&self) -> u32 {
        if self.look.as_deref().is_some_and(|l| l.contains("fan")) { FAN_EFFECT } else { MORTAR_EFFECT }
    }
}

#[derive(Deserialize)]
struct World {
    #[serde(default)]
    simple_dynels: Vec<serde_json::Value>,
}

static PADS: LazyLock<HashMap<u32, Vec<Pad>>> = LazyLock::new(|| {
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/world_content.json")).expect("world_content.json");
    worlds
        .into_iter()
        .filter_map(|(pf, w)| {
            let w = serde_json::from_value::<World>(w).ok()?;
            let pads = w
                .simple_dynels
                .into_iter()
                .filter(|d| d.get("kind").and_then(|k| k.as_str()) == Some("jump_pad"))
                .filter_map(|d| serde_json::from_value::<Pad>(d).ok())
                .collect();
            Some((pf.parse().ok()?, pads))
        })
        .collect()
});

/// What a player is doing with pads.
#[derive(Default)]
struct State {
    /// Flying until then.
    airborne_until: Option<Instant>,
    /// The pad the player stands on and that already fired (or that they landed on): it fires
    /// again only after they step off.
    on: Option<u32>,
    /// Bumped on every movement report; a pending (walk-there) launch is dropped if it changed.
    moves: u64,
}

static STATES: LazyLock<Mutex<HashMap<Id32, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static PRESENCES: Mutex<Vec<world::Presence>> = Mutex::new(Vec::new());

/// Puts every pad into its world.
pub fn start() {
    let mut presences = PRESENCES.lock().unwrap();
    for (&playfield, pads) in PADS.iter() {
        for pad in pads {
            let (outbox, mut inbox) = mpsc::unbounded_channel();
            tokio::spawn(async move { while inbox.recv().await.is_some() {} });
            let p = pad.clone();
            let pos = std::sync::Arc::new(Mutex::new(pad.pos));
            presences.push(world::join(pad.id(), playfield, pos, move || spawn_records(&p, playfield), outbox));
        }
    }
    info!(count = presences.len(), "jump pads spawned");
}

/// UpdateMessage + `SimpleDynelFullUpdateIIR_c` (ProtoGameCodeDynelIIR, ProtoSimpleDynelIIR,
/// VisualData: visual id, resource type, no particles, bounds, scale, u32, u8), then CarsGroup;
/// matches the recorded session's pads.
fn spawn_records(pad: &Pad, playfield: u32) -> Vec<Bytes> {
    let id = pad.id();
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pad.pos[0]), y: Some(pad.pos[1]), z: Some(pad.pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(pad.rot[0]), y: Some(pad.rot[1]), z: Some(pad.rot[2]), w: Some(pad.rot[3]) }),
        playfield_instance: Some(playfield),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(0),
        name: Some("Jumper".into()),
        cars_group: Some(CARS_GROUP),
        template_item_id: Some(PAD_TEMPLATE),
        is_closed: Some(true),
        client_script_id: Some(PAD_CLIENT_SCRIPT),
        ..Default::default()
    };
    let record = iir::record("SimpleDynelFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(pad.visual[1]).u32(pad.visual[0]).u32(0);
        pad.bounds.iter().fold(w, |w, v| w.f32(*v)).f32(1.0).u32(0).u8(0)
    });
    let mut records = vec![create_game_object(id, playfield, pad.pos), record, int32_stat(id, STAT_CARS_GROUP, cars_group(pad, playfield))];
    if crate::buildie::unbuilt_at(playfield, pad.pos) {
        // live: the pad under a buildie appears once the buildie is built (recorded: the wind
        // propeller hidden, jumper 602 shown)
        records.push(show_record(id, false));
    }
    records
}

/// `UpdateSimpleDynelIIR_c` as recorded: u32 0, u8 hidden, u8 1, u8 0, u8 1.
fn show_record(id: Id32, shown: bool) -> Bytes {
    iir::record("UpdateSimpleDynelIIR_c", id, false, |w| w.u32(0).u8(u8::from(!shown)).u8(1).u8(0).u8(1))
}

/// Whether a jump pad stands at `at` (under a buildie: a bouncer).
pub fn pad_at_spot(playfield: u32, at: [f32; 3]) -> bool {
    PADS.get(&playfield).is_some_and(|pads| pads.iter().any(|p| ((p.pos[0] - at[0]).powi(2) + (p.pos[2] - at[2]).powi(2)).sqrt() < 1.0))
}

/// Pads under an unbuilt buildie are inactive (CARS group 5, as recorded) until it is built.
fn cars_group(pad: &Pad, playfield: u32) -> u32 {
    if !pad.launches() || crate::buildie::unbuilt_at(playfield, pad.pos) { CARS_INACTIVE } else { CARS_GROUP }
}

/// Every pad, for the test commands (devcmd.rs): (name, playfield, position).
/// Where pad `guid` stands.
pub fn pad_pos(playfield: u32, guid: u32) -> Option<[f32; 3]> {
    PADS.get(&playfield)?.iter().find(|p| p.guid == guid).map(|p| p.pos)
}

pub fn points() -> Vec<(String, u32, [f32; 3])> {
    let mut v: Vec<_> = PADS
        .iter()
        .flat_map(|(&pf, pads)| pads.iter().map(move |p| (p.look.clone().unwrap_or_else(|| "pad".into()), pf, p.pos)))
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

/// A working pad at `at`, the nearest if several (6200's pond routes cross: SwampToPond3 and
/// PondToSwamp2 stand 0.8 m apart).
fn pad_at(playfield: u32, at: [f32; 3]) -> Option<&'static Pad> {
    let flat = |p: &Pad| ((p.pos[0] - at[0]).powi(2) + (p.pos[2] - at[2]).powi(2)).sqrt();
    PADS.get(&playfield)?
        .iter()
        .filter(|p| flat(p) <= TRIGGER_RADIUS && (p.pos[1] - at[1]).abs() <= TRIGGER_HEIGHT)
        .min_by(|a, b| flat(a).total_cmp(&flat(b)))
        .filter(|p| p.launches() && !crate::buildie::unbuilt_at(playfield, p.pos))
}

/// The buildie at `at` was built: the pad there starts working, and whoever stands on it (the
/// builder, usually) is launched right away rather than at their next step.
pub fn activate(playfield: u32, at: [f32; 3]) {
    let Some(pads) = PADS.get(&playfield) else { return };
    for pad in pads.iter().filter(|p| ((p.pos[0] - at[0]).powi(2) + (p.pos[2] - at[2]).powi(2)).sqrt() < 1.0) {
        let records = vec![show_record(pad.id(), true), int32_stat(pad.id(), STAT_CARS_GROUP, CARS_GROUP)];
        world::send_playfield(pad.id(), false, world::Out::Records(records));
        for (player, pos) in world::position_handles(playfield) {
            let here = *pos.lock().unwrap();
            if !crate::npc::is_player(player) || pad_at(playfield, here).is_none_or(|p| p.guid != pad.guid) {
                continue;
            }
            STATES.lock().unwrap().entry(player).or_default().on = Some(pad.guid);
            info!(?player, pad = pad.guid, "launched on the pad as it was built");
            tokio::spawn(launch(player, playfield, pad, pos));
        }
    }
}

/// `UpdateSimpleDynelIIR_c` showing or hiding a pad and its CARS group, for one player.
pub fn pad_records(guid: u32, shown: bool) -> Vec<Bytes> {
    let id = Id32::new(PAD_TYPE, guid);
    vec![show_record(id, shown), int32_stat(id, STAT_CARS_GROUP, if shown { CARS_GROUP } else { CARS_INACTIVE })]
}

/// The player reported being at `at` (`from`: where a click-to-move path started, `at` being
/// its end). Launches them if that is on a pad.
pub fn moved(player: Id32, playfield: u32, pos: &world::Position, at: [f32; 3], from: Option<[f32; 3]>) {
    // (the tutorial keeps a pad shut until the player beat the wave before it)
    let pad = pad_at(playfield, at).filter(|p| crate::tutorial::pad_open(player, playfield, p.guid));
    let moves = {
        let mut states = STATES.lock().unwrap();
        let s = states.entry(player).or_default();
        s.moves += 1;
        if s.airborne_until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        s.airborne_until = None;
        let id = pad.map(|p| p.guid);
        if id.is_none() || s.on == id {
            s.on = id;
            return;
        }
        if from.is_none() {
            s.on = id;
        }
        s.moves
    };
    let Some(pad) = pad else { return };
    let walk = from.map_or(0.0, |f| ((at[0] - f[0]).powi(2) + (at[2] - f[2]).powi(2)).sqrt() / WALK_SPEED);
    let pos = pos.clone();
    tokio::spawn(async move {
        if walk > 0.0 {
            tokio::time::sleep(Duration::from_secs_f32(walk)).await;
            let mut states = STATES.lock().unwrap();
            let s = states.entry(player).or_default();
            if s.moves != moves {
                return;
            }
            s.on = Some(pad.guid);
        }
        launch(player, playfield, pad, pos).await;
    });
}

/// Forgets a player who left.
pub fn player_left(player: Id32) {
    STATES.lock().unwrap().remove(&player);
}

/// The jump, and on through every pad it lands on that throws on (`Pad::throws_on`).
async fn launch(player: Id32, playfield: u32, mut pad: &'static Pad, pos: world::Position) {
    for _ in 0..MAX_HOPS {
        let end = hop(player, pad, &pos).await;
        let next = pad_at(playfield, end).filter(|p| p.guid != pad.guid);
        if let Some(s) = STATES.lock().unwrap().get_mut(&player) {
            // landing on another pad doesn't fire it until the player steps off
            s.on = next.map(|p| p.guid);
        }
        match next {
            Some(p) if p.throws_on() => pad = p,
            _ => break,
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    world::send_playfield(player, true, world::Out::Records(vec![effects::buff_remove(player, LOCK_BUFF, Id32::default())]));
}

/// One jump: arc maths from the recorded jumps (all seven fit exactly): the arc peaks `apex`
/// above the higher of start and end; T = time up + time down. Returns where it landed.
async fn hop(player: Id32, pad: &'static Pad, pos: &world::Position) -> [f32; 3] {
    let start = *pos.lock().unwrap();
    let end = pad.target;
    let top = start[1].max(end[1]) + pad.apex.unwrap_or(DEFAULT_APEX);
    let up = (2.0 * (top - start[1]) / GRAVITY).sqrt();
    let down = (2.0 * (top - end[1]) / GRAVITY).sqrt();
    let t = up + down;
    let vy = GRAVITY * up;
    let (dx, dz) = (end[0] - start[0], end[2] - start[2]);
    let h = (dx * dx + dz * dz).sqrt().max(0.001);
    let dir = [dx / h, dz / h];
    if let Some(s) = STATES.lock().unwrap().get_mut(&player) {
        s.airborne_until = Some(Instant::now() + Duration::from_secs_f32(t + 0.3));
    }
    info!(?player, pad = pad.guid, ?end, t, "jump pad");
    // `ParameterizedBallisticMoveTask` (12): u8 0, start, u8 12, u8 0, f32 T, f32 T, f32 0,
    // start, end, horizontal direction, u8 0, u8 0, f32 horizontal speed, f32 vertical speed,
    // u8 1, u8 1
    let flight = iir::record("MoveTaskIIR_c", player, false, |w| {
        let w = w.u8(0).f32(start[0]).f32(start[1]).f32(start[2]).u8(12).u8(0).f32(t).f32(t).f32(0.0);
        let w = [start, end, [dir[0], 0.0, dir[1]]].iter().flatten().fold(w, |w, v| w.f32(*v));
        w.u8(0).u8(0).f32(h / t).f32(vy).u8(1).u8(1)
    });
    let records = vec![
        effects::effect_package(pad.id(), pad.effect()),
        stop_record(player, start, dir),
        flight,
        effects::buff_add(player, LOCK_BUFF, Id32::default(), 1500),
    ];
    world::send_playfield(player, true, world::Out::Records(records));

    tokio::time::sleep(Duration::from_secs_f32((t - 0.2).max(0.0))).await;
    world::send_playfield(player, true, world::Out::Records(vec![effects::effect_package(player, LANDING_EFFECT)]));
    tokio::time::sleep(Duration::from_secs_f32(t.min(0.2))).await;
    *pos.lock().unwrap() = end;
    world::send_playfield(player, true, world::Out::Records(vec![stop_record(player, end, dir)]));
    end
}

/// `StopIIR_c`: position, rotation quaternion (yaw towards `dir`), u8 0, u8 0.
fn stop_record(id: Id32, at: [f32; 3], dir: [f32; 2]) -> Bytes {
    let half = dir[0].atan2(dir[1]) / 2.0;
    iir::record("StopIIR_c", id, false, |w| {
        w.f32(at[0]).f32(at[1]).f32(at[2]).f32(0.0).f32(half.sin()).f32(0.0).f32(half.cos()).u8(0).u8(0)
    })
}
