//! The Underworld's front gate (2320), as the user remembers live: using the pedestal before it
//! puts the yellow crystal on it, its light shines on the gate and the gate opens (like the
//! Medusa mirrors). Past the gate is a floor of stones that collapses once someone walks onto
//! it, dropping them to the level below (the client data's "jump to location" marker; the
//! story's "Pass through the Underworld Gate" area is down there).
//!
//! The pieces are the client's (world content simple dynels): the pedestal is an invisible
//! marker ("pedestal", script group 90; the pedestal players see is static scenery), so it is
//! spawned usable as live made the Lost Creations button ("XX_Interactable"). The key isn't
//! checked: the teleport window that leads here already requires it. A copy nobody is in goes
//! back to the start.

use crate::instance::Pf;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{ProtoGameCodeDynelIir, ProtoQuat, ProtoSimpleDynelIir, ProtoVec3},
    types::Id32,
};
use serde::Deserialize;
use tracing::info;

use crate::{scenery::SIMPLE_DYNEL_TYPE, world};

pub const PLAYFIELD: u32 = 2320;
const PEDESTAL: u32 = 491_019;
/// "effect_mesh_loot_gold_crystal_key": the crystal that appears on the pedestal.
const CRYSTAL: u32 = 496_460;
/// "underworld_gate_a_myth_interactable" (blend setup 474600, as the LEGO gates).
pub const GATE: u32 = 491_018;
/// "evil_fort_floor_16x16_c_collapsing_medieval_interactable" (blend setup 474600: its open
/// motion is the collapse).
const FLOOR: u32 = 493_368;
/// "jump to location": where the floor drops players.
const DROP_TO: [f32; 3] = [228.43, 88.5, 321.83];
/// The crystal's looping glow (`particle_loot_gold_crystal_key_cinematic`, the Key to the
/// Underworld's own effect).
const CRYSTAL_GLOW: u32 = 495_376;
/// The gate's open effect: its light beam, souls and backlight, a camera shake and `anim`.
const GATE_OPENS: u32 = 492_693;
/// The light from the crystal to the gate: from the crystal along +z (the identity rotation's
/// facing) to the gate's "door target" marker.
const BEAM_FROM: [f32; 3] = [228.05, 105.65, 300.3];
const BEAM_LENGTH: f32 = 10.3;
/// The beam's dynel instance (clear of mirrors.rs, gates.rs and tiki.rs's).
const BEAM_INSTANCE: u32 = 0x00c3_0000;
/// How live made a simple dynel usable (lost_creations.rs's button): item template
/// "XX_Interactable" and reach.
const INTERACTABLE_TEMPLATE: u32 = 482_763;
const REACH: u32 = 50;
/// The pedestal's box (its marker's is 5 cm): about the size of the pedestal players see.
const PEDESTAL_BOUNDS: [f32; 6] = [-0.6, 0.0, -0.6, 0.6, 1.6, 0.6];
/// The floor's box (the RDB's is empty): 16 x 16 m.
const FLOOR_BOUNDS: [f32; 6] = [-8.0, -1.0, -8.0, 8.0, 1.0, 8.0];
/// Standing on the floor: within its square, not below it.
const FLOOR_HALF: f32 = 8.0;
const FLOOR_BELOW: f32 = 2.0;
/// From the first step on the floor to the drop (the collapse plays first).
const COLLAPSE_DELAY: Duration = Duration::from_millis(600);
/// How high the drop arcs (m): barely, it is a fall.
const DROP_APEX: f32 = 0.3;
const CHECK: Duration = Duration::from_millis(250);
const RESET_CHECK: Duration = Duration::from_secs(30);
const STAT_CARS_GROUP: u32 = 1050;
const CARS_USABLE: u32 = 1;
const CARS_INACTIVE: u32 = 5;

#[derive(Debug, Clone, Deserialize)]
struct Piece {
    name: String,
    pos: [f32; 3],
    rot: [f32; 4],
    visual: [u32; 2],
    bounds: [f32; 6],
    #[serde(default = "one")]
    scale: f32,
    guid: u32,
}

fn one() -> f32 {
    1.0
}

/// The pedestal, crystal and floor from the client's world content.
static PIECES: LazyLock<HashMap<u32, Piece>> = LazyLock::new(|| {
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
    worlds
        .get(&PLAYFIELD.to_string())
        .and_then(|w| w.get("simple_dynels"))
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|d| serde_json::from_value::<Piece>(d.clone()).ok())
        .filter(|p| [PEDESTAL, CRYSTAL, FLOOR].contains(&p.guid))
        .map(|p| (p.guid, p))
        .collect()
});

#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct State {
    open: bool,
    collapsed: bool,
}

static STATE: LazyLock<Mutex<HashMap<Pf, State>>> = LazyLock::new(Default::default);
static PRESENCES: LazyLock<Mutex<HashMap<Pf, Vec<world::Presence>>>> = LazyLock::new(Default::default);

fn state(pf: Pf) -> State {
    STATE.lock().unwrap().get(&pf).copied().unwrap_or_default()
}

/// Whether a simple dynel (base guid) in a copy of `model` is the pedestal.
pub fn is_pedestal(model: u32, guid: u32) -> bool {
    model == PLAYFIELD && guid == PEDESTAL
}

/// Whether the gate is open in copy `pf` (gates.rs).
pub fn solved(pf: Pf) -> bool {
    state(pf).open
}

fn piece_id(pf: Pf, guid: u32) -> Id32 {
    crate::scenery::piece_id(pf, guid)
}

fn beam_id(pf: Pf) -> Id32 {
    Id32::new(SIMPLE_DYNEL_TYPE, crate::instance::fixture(pf, BEAM_INSTANCE))
}

/// A piece's spawn records (as scenery.rs): its own look, no collision; `usable` gives it live's
/// interactable template, reach and the name players see.
fn piece_records(p: &Piece, pf: Pf, bounds: [f32; 6], usable: Option<&str>) -> Vec<Bytes> {
    let id = piece_id(pf, p.guid);
    let cars = if usable.is_some() { CARS_USABLE } else { CARS_INACTIVE };
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(p.pos[0]), y: Some(p.pos[1]), z: Some(p.pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(p.rot[0]), y: Some(p.rot[1]), z: Some(p.rot[2]), w: Some(p.rot[3]) }),
        playfield_instance: Some(pf.key()),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(0),
        name: Some(p.name.clone()),
        template_item_id: usable.is_some().then_some(INTERACTABLE_TEMPLATE),
        interaction_radius: usable.is_some().then_some(REACH),
        display_name: usable.map(str::to_string),
        cars_group: Some(cars),
        is_closed: Some(true),
        ..Default::default()
    };
    let record = iir::record("SimpleDynelFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(p.visual[1]).u32(p.visual[0]).u32(0);
        bounds.iter().fold(w, |w, v| w.f32(*v)).f32(p.scale).u32(0).u8(0)
    });
    vec![crate::game::create_game_object(id, pf, p.pos), record, crate::game::int32_stat(id, STAT_CARS_GROUP, cars)]
}

fn join(id: Id32, pf: Pf, pos: [f32; 3], spawn: impl Fn() -> Vec<Bytes> + Send + Sync + 'static) -> world::Presence {
    let (outbox, mut inbox) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    world::join(id, pf, Arc::new(Mutex::new(pos)), spawn, outbox)
}

/// Puts the pedestal, the crystal (hidden until placed), the beam and the floor into copy `pf`
/// (worlds.rs).
pub fn spawn_world(pf: Pf) {
    if pf.model() != PLAYFIELD {
        return;
    }
    let mut presences = Vec::new();
    if let Some(p) = PIECES.get(&PEDESTAL).cloned() {
        presences.push(join(piece_id(pf, p.guid), pf, p.pos, move || piece_records(&p, pf, PEDESTAL_BOUNDS, Some("Pedestal"))));
    }
    if let Some(p) = PIECES.get(&CRYSTAL).cloned() {
        presences.push(join(piece_id(pf, p.guid), pf, p.pos, move || {
            let mut records = piece_records(&p, pf, p.bounds, None);
            let id = piece_id(pf, p.guid);
            if solved(pf) {
                records.push(crate::effects::effect_package(id, CRYSTAL_GLOW));
            } else {
                records.push(crate::encounter::door_record(id, true));
            }
            records
        }));
    }
    let beam = beam_id(pf);
    presences.push(join(beam, pf, BEAM_FROM, move || {
        let mut records = crate::scenery::invisible_records(beam, pf, BEAM_FROM, [0.0, 0.0, 0.0, 1.0]);
        if solved(pf) {
            records.push(crate::mirrors::beam_particle(beam, BEAM_LENGTH));
        }
        records
    }));
    if let Some(p) = PIECES.get(&FLOOR).cloned() {
        presences.push(join(piece_id(pf, p.guid), pf, p.pos, move || {
            let mut records = piece_records(&p, pf, FLOOR_BOUNDS, None);
            if state(pf).collapsed {
                records.push(crate::effects::effect_package(piece_id(pf, p.guid), crate::gates::OPEN));
            }
            records
        }));
    }
    PRESENCES.lock().unwrap().insert(pf, presences);
}

/// Takes copy `pf`'s pieces and state out.
pub fn despawn_world(pf: Pf) {
    let gone = PRESENCES.lock().unwrap().remove(&pf);
    drop(gone);
    STATE.lock().unwrap().remove(&pf);
}

/// `player` used the pedestal in copy `pf`: the crystal appears and glows, its light shines on
/// the gate and the gate opens. Once per copy.
pub fn use_pedestal(player: Id32, pf: Pf) {
    {
        let mut st = STATE.lock().unwrap();
        let s = st.entry(pf).or_default();
        if s.open {
            return;
        }
        s.open = true;
    }
    info!(?player, playfield = ?pf, "Underworld pedestal: the crystal opens the gate");
    show_open(pf);
}

/// Shows copy `pf`'s crystal, beam and gate as they are now.
fn show_open(pf: Pf) {
    let open = solved(pf);
    let crystal = piece_id(pf, CRYSTAL);
    let mut records = vec![crate::encounter::door_record(crystal, !open)];
    if open {
        records.push(crate::effects::effect_package(crystal, CRYSTAL_GLOW));
    }
    world::send_playfield(crystal, false, world::Out::Records(records));
    let beam = beam_id(pf);
    let light = if open { crate::mirrors::beam_particle(beam, BEAM_LENGTH) } else { crate::mirrors::beam_stop_record(beam) };
    world::send_playfield(beam, false, world::Out::Records(vec![light]));
    if open {
        let gate = piece_id(pf, GATE);
        world::send_playfield(gate, false, world::Out::Records(vec![crate::effects::effect_package(gate, GATE_OPENS)]));
    }
    crate::gates::refresh(pf, GATE);
}

/// Test command (`!keygate open|reset`): opens copy `pf`'s gate, or puts it back to the start
/// (the floor whole again). False if `pf` isn't the Underworld.
pub fn set(pf: Pf, open: bool) -> bool {
    if pf.model() != PLAYFIELD {
        return false;
    }
    let was = state(pf);
    STATE.lock().unwrap().insert(pf, State { open, collapsed: open && was.collapsed });
    show_open(pf);
    if was.collapsed && !open {
        let floor = piece_id(pf, FLOOR);
        world::send_playfield(floor, false, world::Out::Records(vec![crate::effects::effect_package(floor, crate::mirrors::TURN_MINUS)]));
    }
    true
}

/// Watches the open gates' floors (a player on one collapses it and falls) and puts copies
/// nobody is in back to the start.
pub fn start() {
    info!(pieces = PIECES.len(), "Underworld gate");
    tokio::spawn(async {
        let mut tick = tokio::time::interval(CHECK);
        loop {
            tick.tick().await;
            for pf in crate::worlds::copies_of(PLAYFIELD).into_iter().filter(|pf| solved(*pf) && !crate::instance::rules(*pf).editor) {
                watch_floor(pf);
            }
        }
    });
    tokio::spawn(async {
        let mut tick = tokio::time::interval(RESET_CHECK);
        loop {
            tick.tick().await;
            for pf in crate::worlds::copies_of(PLAYFIELD) {
                let empty = !world::positions(pf).iter().any(|(id, _)| crate::npc::is_player(*id));
                if empty && STATE.lock().unwrap().remove(&pf).is_some_and(|s| s != State::default()) {
                    info!(playfield = ?pf, "Underworld gate reset");
                }
            }
        }
    });
}

/// Players standing on copy `pf`'s floor: the first collapses it, and everyone on it falls.
fn watch_floor(pf: Pf) {
    let Some(floor) = PIECES.get(&FLOOR) else { return };
    let on: Vec<(Id32, world::Position)> = world::position_handles(pf)
        .into_iter()
        .filter(|(id, _)| crate::npc::is_player(*id) && !crate::jumppad::airborne(*id) && !FALLING.lock().unwrap().contains(id))
        .filter(|(_, h)| {
            let at = *h.lock().unwrap();
            (at[0] - floor.pos[0]).abs() <= FLOOR_HALF && (at[2] - floor.pos[2]).abs() <= FLOOR_HALF && at[1] > floor.pos[1] - FLOOR_BELOW
        })
        .collect();
    if on.is_empty() {
        return;
    }
    let first = {
        let mut st = STATE.lock().unwrap();
        let s = st.entry(pf).or_default();
        !std::mem::replace(&mut s.collapsed, true)
    };
    if first {
        info!(playfield = ?pf, "Underworld floor collapses");
        let id = piece_id(pf, FLOOR);
        world::send_playfield(id, false, world::Out::Records(vec![crate::effects::effect_package(id, crate::gates::OPEN)]));
    }
    for (player, handle) in on {
        FALLING.lock().unwrap().insert(player);
        tokio::spawn(async move {
            if first {
                tokio::time::sleep(COLLAPSE_DELAY).await;
            }
            let plain = crate::jumppad::Tumble { knockback: false, launched: false, direction: 0 };
            crate::jumppad::throw_as(player, &handle, DROP_TO, DROP_APEX, plain).await;
            FALLING.lock().unwrap().remove(&player);
        });
    }
}

/// Players falling now (not caught again).
static FALLING: LazyLock<Mutex<HashSet<Id32>>> = LazyLock::new(Default::default);

/// The pedestal's position, for `!goto pedestal` (devcmd.rs).
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    PIECES.get(&PEDESTAL).map(|p| ("Underworld gate pedestal".to_string(), PLAYFIELD, p.pos)).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_are_in_the_world_content() {
        assert_eq!(PIECES.len(), 3, "pedestal, crystal, floor");
        assert!(BEAM_INSTANCE < 1 << 24);
    }
}
