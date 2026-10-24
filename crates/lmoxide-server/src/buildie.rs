//! Buildies: piles of bricks a player builds into something (a jump pad's mortar or fan, a ramp,
//! a cannon) by interacting with them, as in the recorded session (docs/PROTOCOL.md "Buildies").
//!
//! A buildie is a destructible (type 51321, CARS group 1) whose Health is the build progress.
//! The client queues an `ObjectInteractionCommand` (CommandType 18, data = the buildie's ID32);
//! the server accepts it and drives the build: `buildableStart`, the build rate (FloatStat
//! 2999049), progress ticks (Health += rate) while the builder stays close, and at the maximum the
//! built visual, `buildableStop` and CARS group 5. A jump pad under an unbuilt buildie doesn't
//! launch anyone. Placements and visuals: data/buildies.json (tools/re/demo/buildies.py); only
//! the recorded worlds' buildies are known.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{ProtoGameCodeDynelIir, ProtoId32, ProtoQuat, ProtoScryScriptIir, ProtoSimpleDynelIir, ProtoVec3},
    types::Id32,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    game::{STAT_HEALTH, create_game_object, stat_record},
    world,
};

/// CommandType of `ObjectInteractionCommand` (building).
pub const COMMAND_OBJECT_INTERACTION: u32 = 18;
const DESTRUCTIBLE_TYPE: u32 = 51321;
const STAT_CARS_GROUP: u32 = 1050;
/// Set to 1 while someone builds.
const STAT_BUILDING: u32 = 2_999_047;
/// FloatStat: the summed build rate, progress added per tick.
const STAT_BUILD_RATE: u32 = 2_999_049;
/// Sent as 58 when the build completes (meaning unknown).
const STAT_455: u32 = 455;
const CARS_BUILDABLE: u32 = 1;
const CARS_BUILT: u32 = 5;
/// A player's build rate (recorded: 62 per tick).
const PLAYER_RATE: u32 = 62;
/// Tick interval: the item stat `BuildableServerInterval` (0.1 on buildie templates, also the
/// 0.1 in the buildie's Destructible fields).
const TICK: Duration = Duration::from_millis(100);
/// The builder must stay this close (assumed).
const BUILD_RANGE: f32 = 6.0;
/// `SimpleDynelVisualChangeIIR_c` states as recorded: 4 = the built visual, 2 = particles off.
const VISUAL_BUILT: u32 = 4;
const VISUAL_PARTICLES_OFF: u32 = 2;

#[derive(Debug, Clone, Deserialize)]
struct Def {
    name: String,
    /// remoteformat XML naming the localized display name
    display_name: String,
    template: u32,
    playfield: u32,
    pos: [f32; 3],
    rot: [f32; 4],
    monster_data: u32,
    visual_type: u32,
    particles: Vec<[u32; 5]>,
    bounds: [f32; 6],
    scale: f32,
    max: u32,
    /// CoreDestructible f32, side
    destructible: (f32, u32),
    /// Destructible: u32, f32, f32, u32, f32
    tail: (u32, f32, f32, u32, f32),
    built_visual: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Idle,
    Building(Id32),
    Built,
}

struct Buildie {
    def: Def,
    progress: u32,
    state: State,
    _presence: Option<world::Presence>,
}

static BUILDIES: LazyLock<Mutex<HashMap<Id32, Buildie>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Puts every buildie into its world.
pub fn start() {
    let defs: Vec<Def> = serde_json::from_str(include_str!("../../../data/buildies.json")).expect("buildies.json");
    for (i, def) in defs.into_iter().enumerate() {
        // instances clear of the NPC module's destructibles
        let id = Id32::new(DESTRUCTIBLE_TYPE, 0x00b0_0000 + i as u32);
        let playfield = def.playfield;
        let pos = Arc::new(Mutex::new(def.pos));
        BUILDIES.lock().unwrap().insert(id, Buildie { def: def.clone(), progress: 0, state: State::Idle, _presence: None });
        let (outbox, mut inbox) = mpsc::unbounded_channel();
        tokio::spawn(async move { while inbox.recv().await.is_some() {} });
        let presence = world::join(id, playfield, pos, move || spawn_records(id), outbox);
        if let Some(b) = BUILDIES.lock().unwrap().get_mut(&id) {
            b._presence = Some(presence);
        }
    }
    info!(count = BUILDIES.lock().unwrap().len(), "buildies spawned");
}

/// Every buildie, for the test commands (devcmd.rs): (name, playfield, position, state).
pub fn points() -> Vec<(String, u32, [f32; 3], String)> {
    let mut v: Vec<_> = BUILDIES
        .lock()
        .unwrap()
        .values()
        .map(|b| (b.def.name.clone(), b.def.playfield, b.def.pos, format!("{:?}", b.state)))
        .collect();
    v.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    v
}

/// Whether an unbuilt buildie stands at `at` (a jump pad there doesn't work yet).
pub fn unbuilt_at(playfield: u32, at: [f32; 3]) -> bool {
    BUILDIES
        .lock()
        .unwrap()
        .values()
        .any(|b| b.def.playfield == playfield && b.state != State::Built && distance(b.def.pos, at) < 1.0)
}

/// `player` asked to build what `data` (the ObjectInteractionCommand's data) names. Returns
/// `None` if that isn't a buildie, else whether the command is accepted (a refused one gets
/// success then failure, as live answered repeats).
pub fn interact(player: Id32, data: &[u8]) -> Option<bool> {
    let target = Id32::new(u32::from_be_bytes(data.get(0..4)?.try_into().ok()?), u32::from_be_bytes(data.get(4..8)?.try_into().ok()?));
    let mut buildies = BUILDIES.lock().unwrap();
    let b = buildies.get_mut(&target)?;
    if b.state != State::Idle {
        return Some(false);
    }
    let mut records = Vec::new();
    if b.progress > 0 {
        // a restart after a stop starts over (recorded)
        b.progress = 0;
        records.push(stat_record(target, STAT_BUILDING, 0));
        records.push(stat_record(target, STAT_HEALTH, 0));
    }
    b.state = State::Building(player);
    info!(?player, buildie = %b.def.name, "build started");
    records.extend(start_records(target));
    drop(buildies);
    world::send_playfield(target, false, world::Out::Records(records));
    tokio::spawn(build(target, player));
    Some(true)
}

async fn build(id: Id32, player: Id32) {
    let mut tick = tokio::time::interval(TICK);
    tick.tick().await;
    loop {
        tick.tick().await;
        // before taking BUILDIES: world::join holds the members lock while spawn_records takes it
        let builder = world::whereabouts(player);
        let mut buildies = BUILDIES.lock().unwrap();
        let Some(b) = buildies.get_mut(&id) else { return };
        if b.state != State::Building(player) {
            return;
        }
        let near = builder.is_some_and(|(pf, p)| pf == b.def.playfield && distance(p, b.def.pos) <= BUILD_RANGE);
        if !near {
            b.state = State::Idle;
            info!(?player, buildie = %b.def.name, progress = b.progress, "build stopped");
            drop(buildies);
            world::send_playfield(id, false, world::Out::Records(vec![script_record(id, "buildableStop"), float_stat_record(id, STAT_BUILD_RATE, 0.0)]));
            return;
        }
        b.progress = (b.progress + PLAYER_RATE).min(b.def.max);
        let mut records = vec![stat_record(id, STAT_HEALTH, b.progress)];
        let done = b.progress >= b.def.max;
        if done {
            b.state = State::Built;
            info!(?player, buildie = %b.def.name, "built");
            records.extend(built_records(id, &b.def));
        }
        let (playfield, pos) = (b.def.playfield, b.def.pos);
        drop(buildies);
        world::send_playfield(id, false, world::Out::Records(records));
        if done {
            crate::jumppad::activate(playfield, pos);
            return;
        }
    }
}

/// What starting a build sends (recorded order): the build rate, `buildableStart`, Building 1.
fn start_records(id: Id32) -> Vec<Bytes> {
    vec![float_stat_record(id, STAT_BUILD_RATE, PLAYER_RATE as f32), script_record(id, "buildableStart"), stat_record(id, STAT_BUILDING, 1)]
}

/// What completing a build sends (recorded order).
fn built_records(id: Id32, def: &Def) -> Vec<Bytes> {
    let mut r = vec![stat_record(id, STAT_HEALTH, def.max), stat_record(id, STAT_455, 58)];
    if let Some(v) = def.built_visual {
        r.push(visual_change_record(id, VISUAL_BUILT, v));
    }
    r.extend([
        script_record(id, "buildableStop"),
        visual_change_record(id, VISUAL_PARTICLES_OFF, 0),
        stat_record(id, STAT_CARS_GROUP, CARS_BUILT),
        float_stat_record(id, STAT_BUILD_RATE, 0.0),
        iir::record("UpdateSimpleDynelIIR_c", id, false, |w| w.u32(0).u8(1).u8(1).u8(0).u8(1)),
        stat_record(id, STAT_CARS_GROUP, CARS_BUILT),
    ]);
    r
}

/// UpdateMessage + `DestructibleFullUpdateIIR_c` as recorded (see npc.rs `smashable_records` for
/// the layout; Health = progress, the u32 after it = the maximum), then the current state.
fn spawn_records(id: Id32) -> Vec<Bytes> {
    let buildies = BUILDIES.lock().unwrap();
    let Some(b) = buildies.get(&id) else { return vec![] };
    let d = &b.def;
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(d.pos[0]), y: Some(d.pos[1]), z: Some(d.pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(d.rot[0]), y: Some(d.rot[1]), z: Some(d.rot[2]), w: Some(d.rot[3]) }),
        playfield_instance: Some(d.playfield),
        running_effects: vec![],
    };
    // once built, as live re-sent it: the built visual in field 1, CARS group 5, no particles,
    // Health at the maximum and the Destructible's built flag
    let built = b.state == State::Built;
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(if built { d.built_visual.unwrap_or(0) } else { 0 }),
        name: Some(d.name.clone()),
        display_name: Some(d.display_name.clone()),
        cars_group: Some(if built { CARS_BUILT } else { CARS_BUILDABLE }),
        template_item_id: Some(d.template),
        is_closed: Some(true),
        ..Default::default()
    };
    let particles: &[[u32; 5]] = if built { &[] } else { &d.particles };
    let record = iir::record("DestructibleFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(d.monster_data).u32(d.visual_type).u32(particles.len() as u32);
        let w = particles.iter().flatten().fold(w, |w, v| w.u32(*v));
        let w = d.bounds.iter().fold(w, |w, v| w.f32(*v)).f32(d.scale).u32(0).u8(0);
        let w = w.u32(b.progress).u32(d.max).f32(d.destructible.0).u32(d.destructible.1);
        let w = Id32::default().write(w);
        w.u32(d.tail.0).f32(d.tail.1).f32(d.tail.2).u32(u32::from(built)).f32(d.tail.4)
    });
    let mut records = vec![create_game_object(id, d.playfield, d.pos), record];
    if let State::Building(_) = b.state {
        records.extend(start_records(id));
    }
    records
}

/// `ScryScriptIIR_c` as recorded: version 5, ScriptMessage with the message name and the buildie.
fn script_record(id: Id32, message: &str) -> Bytes {
    let msg = ProtoScryScriptIir {
        version: Some(5),
        r#type: Some(2),
        int_parameter: Some(0),
        string_parameter_one: Some(message.into()),
        string_parameter_two: Some(String::new()),
        id_parameter: Some(ProtoId32 { r#type: Some(id.kind), instance: Some(id.instance) }),
        variable: vec![],
    };
    iir::record("ScryScriptIIR_c", id, false, |w| w.proto(&msg))
}

/// `FloatStatIIR`: u32 stat, f32 value.
fn float_stat_record(id: Id32, stat: u32, value: f32) -> Bytes {
    iir::record("FloatStatIIR", id, false, |w| w.u32(stat).f32(value))
}

/// `SimpleDynelVisualChangeIIR_c`: u32 state, u32 visual.
fn visual_change_record(id: Id32, state: u32, visual: u32) -> Bytes {
    iir::record("SimpleDynelVisualChangeIIR_c", id, false, |w| w.u32(state).u32(visual))
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_record_matches_recording() {
        // the ramp's DestructibleFullUpdateIIR_c body in the recorded session
        let body = "0000002c0a0f0d4f2d574315acff2f421d168d7e4312140d0000000015f404353f1d0000000025f204353f188ad48210000000770800120758585f52616d701a623c72656d6f7465666f726d61742069643d22323430363231222063617465676f72793d22353032303022206b65793d225721405a47523173683144554c405a572831603022206b6e75626f743d22302220203e3c2f72656d6f7465666f726d61743e280140edd70e60010003ab39000f6a1900000001000272b100000000000000000000000000000000bfc020cdb423d70abdd6d80f3fbfffe1408bcd583f91985c3f800000000000000000000000000006403f800000000000000000000000000000000000003dcccccd00000000000000003f800000";
        let defs: Vec<Def> = serde_json::from_str(include_str!("../../../data/buildies.json")).unwrap();
        let mut def = defs.into_iter().find(|d| d.name == "XX_Ramp").unwrap();
        def.playfield = 33597962; // the recorded instance
        let id = Id32::new(51321, 2606);
        BUILDIES.lock().unwrap().insert(id, Buildie { def, progress: 0, state: State::Idle, _presence: None });
        let records = spawn_records(id);
        assert!(crate::conn::hex(&records[1]).ends_with(body), "{}", crate::conn::hex(&records[1]));
    }

    #[test]
    fn script_record_matches_recording() {
        // ScryScriptIIR_c for buildie (51321, 2606) in the recorded session (body after the header)
        let rec = script_record(Id32::new(51321, 2606), "buildableStart");
        let body = "00000021080510021800220e6275696c6461626c6553746172742a00320708f9900310ae14";
        assert!(crate::conn::hex(&rec).ends_with(body), "{}", crate::conn::hex(&rec));
    }
}
