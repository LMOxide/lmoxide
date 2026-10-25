//! Levers, gates, Medusa mirrors and pressure plates: playfield simple dynels (RDB 1000057) the
//! original server sent as dynels, as it did the jump pads (docs/PROTOCOL.md "Levers, gates,
//! pressure plates"). What they did (which lever opened which gate, the plate puzzles) was
//! server script data that didn't survive, so they are scenery only: not usable (CARS group 5),
//! gates open, and nothing has collision (kdtree 0), so none of them can block a path.
//! The collectibles (discovery.rs) are spawned here too, usable (CARS group 1, as the buildies),
//! and the boss arenas' door visuals, shown or hidden as their encounter has them (encounter.rs).

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
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
    game::{create_game_object, int32_stat},
    world,
};

pub const SIMPLE_DYNEL_TYPE: u32 = 51320;
const STAT_CARS_GROUP: u32 = 1050;
/// Nobody can target or use them.
const CARS_INACTIVE: u32 = 5;
/// Usable (ObjectInteraction), as the buildies.
const CARS_USABLE: u32 = 1;
/// Live's invisible marker dynels ("XX_Invisible", recorded): template and [type, visual].
const INVISIBLE_TEMPLATE: u32 = 249343;
const INVISIBLE_VISUAL: [u32; 2] = [1010030, 95472];
/// Bounds for pieces whose RDB box is empty (assumed size, docs/TUNABLES.md).
const FALLBACK_BOUNDS: [f32; 6] = [-4.0, 0.0, -4.0, 4.0, 6.0, 4.0];

#[derive(Debug, Clone, Deserialize)]
struct Piece {
    name: String,
    pos: [f32; 3],
    rot: [f32; 4],
    /// [resource type, visual id]
    visual: [u32; 2],
    bounds: [f32; 6],
    #[serde(default = "one")]
    scale: f32,
    guid: u32,
}

fn one() -> f32 {
    1.0
}

impl Piece {
    fn is_gate(&self) -> bool {
        ["gate", "portcullis"].iter().any(|k| self.name.contains(k))
    }
}

/// Which 1000057 entries are spawned.
fn wanted(name: &str) -> bool {
    name.starts_with("pressureplate_")
        || name.starts_with("lego_lever")
        || name.starts_with("mirror_medusa")
        || (name.ends_with("_interactable") && (name.contains("gate") || name.contains("portcullis")))
        || crate::discovery::is_collectible(name)
}

/// The spawned pieces by guid: playfield, piece, presence (respawn: mirrors turned).
static PRESENCES: LazyLock<Mutex<HashMap<u32, (u32, Piece, world::Presence)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// (name, playfield, position) of every spawned piece, for the test commands (devcmd.rs).
static PIECES: Mutex<Vec<(String, u32, [f32; 3])>> = Mutex::new(Vec::new());

/// Spawned pieces whose name contains `pattern`, sorted by playfield.
pub fn points(pattern: &str) -> Vec<(String, u32, [f32; 3])> {
    let mut v: Vec<_> = PIECES.lock().unwrap().iter().filter(|p| p.0.contains(pattern)).cloned().collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

/// Puts every piece into its world.
pub fn start() {
    let worlds: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/world_content.json")).expect("world_content.json");
    for (pf, w) in worlds {
        let Ok(playfield) = pf.parse::<u32>() else { continue };
        let Some(list) = w.get("simple_dynels").and_then(|v| v.as_array()) else { continue };
        for piece in list.iter().filter_map(|d| serde_json::from_value::<Piece>(d.clone()).ok()).filter(|p| wanted(&p.name) || crate::encounter::is_door(p.guid) || crate::encounter::is_shield(p.guid)) {
            PIECES.lock().unwrap().push((piece.name.clone(), playfield, piece.pos));
            let presence = join(&piece, playfield);
            PRESENCES.lock().unwrap().insert(piece.guid, (playfield, piece, presence));
        }
    }
    info!(count = PRESENCES.lock().unwrap().len(), "scenery spawned");
}

fn join(piece: &Piece, playfield: u32) -> world::Presence {
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    let id = Id32::new(SIMPLE_DYNEL_TYPE, piece.guid);
    let pos = Arc::new(Mutex::new(piece.pos));
    let p = piece.clone();
    world::join(id, playfield, pos, move || spawn_records(&p, playfield), outbox)
}

/// An invisible marker dynel ("XX_Invisible", as live spawned to carry particles: the plain
/// marker visual, not usable, no collision) at `pos`, facing `rot`: its spawn records.
pub fn invisible_records(id: Id32, playfield: u32, pos: [f32; 3], rot: [f32; 4]) -> Vec<Bytes> {
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(rot[0]), y: Some(rot[1]), z: Some(rot[2]), w: Some(rot[3]) }),
        playfield_instance: Some(playfield),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(0),
        name: Some("XX_Invisible".into()),
        cars_group: Some(CARS_INACTIVE),
        template_item_id: Some(INVISIBLE_TEMPLATE),
        ..Default::default()
    };
    let record = iir::record("SimpleDynelFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(INVISIBLE_VISUAL[1]).u32(INVISIBLE_VISUAL[0]).u32(0);
        FALLBACK_BOUNDS.iter().fold(w, |w, v| w.f32(*v)).f32(1.0).u32(0).u8(0)
    });
    vec![create_game_object(id, playfield, pos), record, int32_stat(id, STAT_CARS_GROUP, CARS_INACTIVE)]
}

/// UpdateMessage + `SimpleDynelFullUpdateIIR_c` (as for jump pads: ProtoGameCodeDynelIIR,
/// ProtoSimpleDynelIIR, VisualData with no particles, bounds, scale, u32, u8), then CarsGroup.
fn spawn_records(p: &Piece, playfield: u32) -> Vec<Bytes> {
    let id = Id32::new(SIMPLE_DYNEL_TYPE, p.guid);
    let usable = crate::discovery::is_collectible(&p.name) || crate::mirrors::is_mirror(p.guid);
    let cars_group = if usable { CARS_USABLE } else { CARS_INACTIVE };
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(p.pos[0]), y: Some(p.pos[1]), z: Some(p.pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(p.rot[0]), y: Some(p.rot[1]), z: Some(p.rot[2]), w: Some(p.rot[3]) }),
        playfield_instance: Some(playfield),
        running_effects: vec![],
    };
    // an arena's shield: a wall while its door is closed (encounter.rs); nothing else collides
    // (n3VisualDynel::EnableCollision(0))
    let shield = crate::encounter::shield_blocks(p.guid);
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(if shield == Some(true) { crate::encounter::SHIELD_COLLISION } else { 0 }),
        name: Some(p.name.clone()),
        cars_group: Some(cars_group),
        // gates open; mirrors as turned (mirrors.rs)
        is_closed: Some(crate::mirrors::closed(p.guid).unwrap_or(!p.is_gate())),
        ..Default::default()
    };
    // the RDB gives the animated pieces an empty box (min +3.4e38, max -3.4e38); send a generous
    // one instead, which at worst affects culling
    let bounds = if p.bounds[0] > p.bounds[3] { FALLBACK_BOUNDS } else { p.bounds };
    let record = iir::record("SimpleDynelFullUpdateIIR_c", id, false, |w| {
        // (a shield shows its collision mesh, as recorded)
        let visual = if shield.is_some() { crate::encounter::SHIELD_COLLISION } else { p.visual[1] };
        let w = w.proto(&dynel).proto(&simple).u32(visual).u32(p.visual[0]).u32(0);
        bounds.iter().fold(w, |w, v| w.f32(*v)).f32(p.scale).u32(0).u8(0)
    });
    let mut records = vec![create_game_object(id, playfield, p.pos), record, int32_stat(id, STAT_CARS_GROUP, cars_group)];
    records.extend(crate::mirrors::pose_records(p.guid));
    // hidden: an arena door that's open now, a mirror puzzle's gate once solved
    if crate::encounter::door_hidden(p.guid).or_else(|| crate::mirrors::gate_open(p.guid)) == Some(true) {
        records.push(crate::encounter::door_record(id, true));
    }
    records
}
