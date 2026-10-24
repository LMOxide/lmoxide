//! Travel between playfields (worlds), within the same game connection.
//!
//! Triggers: the world map sends `GameCharAgent::TeleportMe(playfield, spawnPointID)` (the
//! spawn point is a resurrection point Guid, RDB 1000090); the teleport window (opened by the
//! server with `GameCharInterfaceProject::ScryShowTeleportMessageBox(windowId, ctx)`) asks
//! `ClientRequestDestinationPlayfieldStatus(PlayfieldProxy)`, needs
//! `ReplyToClientRequestDestinationPlayfieldStatus(ready, completed, empty)` before it shows its
//! buttons, and answers `GameCharAgentProject::TeleportMessageBoxAnswer(playfield or -1, ctx)`.
//! Hub portals send nothing: the server notices the player standing in one and opens that
//! destination's teleport window. The windows (and World Map markers) require completed story
//! quests, which `journal_record` provides.
//!
//! The move: `IntendedTeleportIIR_c` + `n3TeleportIIR_t` (the client stops the old playfield and
//! goes to FlowControl DeepTeleport), then the new playfield's records as at login; the client
//! answers with CharInPlayIIR_t again.

use std::{collections::HashMap, sync::LazyLock};

use bytes::Bytes;
use lmo_net::{
    iir,
    proto::{PlayfieldProxyData, ProtoId32},
    types::Id32,
};
use serde::Deserialize;

use crate::game::PLAYFIELD_TYPE;

#[derive(Debug, Clone, Deserialize)]
pub struct Destination {
    /// Arrival point (a resurrection point; its Guid is in the data file).
    pub pos: [f32; 3],
    /// Facing (radians around the vertical axis).
    pub rot: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct Portal {
    to: u32,
    pos: [f32; 3],
    /// Teleport window the portal opens (tools/re/extract_portal_windows.py).
    window: u32,
}

#[derive(Deserialize)]
struct Data {
    destinations: HashMap<String, Destination>,
    portals: HashMap<String, Vec<Portal>>,
}

static DATA: LazyLock<Data> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../data/destinations.json")).expect("destinations.json"));

/// How close to a portal's position counts as standing in it.
const PORTAL_RADIUS: f32 = 3.0;

pub fn destination(playfield: u32) -> Option<&'static Destination> {
    DATA.destinations.get(&playfield.to_string())
}

/// The hub, where portals leading back into their own world go (the worlds' gates home: e.g.
/// the Medieval Kingdom's only nearby portal is listed as leading to the Kingdom itself).
const HUB: u32 = 1200;

/// The portal the player is standing in, if any: (destination playfield, teleport window).
/// The client has no portal trigger of its own; live, server scripts opened the window.
pub fn portal_at(playfield: u32, pos: [f32; 3]) -> Option<(u32, u32)> {
    let portal = DATA.portals.get(&playfield.to_string())?.iter().find(|p| distance(p.pos, pos) <= PORTAL_RADIUS)?;
    Some((if portal.to == playfield { HUB } else { portal.to }, portal.window))
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// PlayfieldProxy as embedded in a BinaryStream (u32 length + PlayfieldProxyData).
pub(crate) fn proxy(playfield: u32) -> PlayfieldProxyData {
    PlayfieldProxyData {
        dimension_id: Some(1),
        model_id: Some(ProtoId32 { r#type: Some(PLAYFIELD_TYPE), instance: Some(playfield) }),
        running_instance: Some(playfield),
        ..Default::default()
    }
}

/// `IntendedTeleportIIR_c`: PlayfieldProxy, u8 local (0: a different playfield), u8.
pub fn intended_teleport_record(character: Id32, playfield: u32) -> Bytes {
    iir::record("IntendedTeleportIIR_c", character, false, |w| w.proto(&proxy(playfield)).u8(0).u8(0))
}

/// `n3TeleportIIR_t` (ReadSubClass 0x12e82a8): position, rotation quaternion, PlayfieldProxy,
/// ID32, BinaryStream (empty: u32 0), u8.
pub fn teleport_record(character: Id32, playfield: u32, pos: [f32; 3], rot: f32) -> Bytes {
    let (s, c) = (rot / 2.0).sin_cos();
    iir::record("n3TeleportIIR_t", character, false, |w| {
        let w = w.f32(pos[0]).f32(pos[1]).f32(pos[2]).f32(0.0).f32(s).f32(0.0).f32(c);
        Id32::default().write(w.proto(&proxy(playfield))).u32(0).u8(0)
    })
}

/// `JournalUpdateIIR` (header protobuf JournalUpdateIIRProto) with every story line completed
/// (data/story_quests.json): the teleport windows and the World Map check
/// CoreJournalHolder::HasEverCompletedQuest for their required quests.
pub fn journal_record(character: Id32) -> Bytes {
    use lmo_net::proto::{
        JournalUpdateIirProto,
        journal_update_iir_proto::{AddedMainQuestType, AddedQuestType},
    };
    let table: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/story_quests.json")).expect("story_quests.json");
    let added_main_quest = table
        .iter()
        .filter_map(|(main, quests)| {
            let main: u32 = main.parse().ok()?;
            let quest = quests
                .as_array()?
                .iter()
                .filter_map(|q| q.as_u64())
                .map(|q| AddedQuestType { quest_id: Some(q as u32), failed: Some(false), ..Default::default() })
                .collect();
            Some(AddedMainQuestType { main_quest_id: Some(main), quest, ..Default::default() })
        })
        .collect();
    let msg = JournalUpdateIirProto { added_main_quest, ..Default::default() };
    iir::record("JournalUpdateIIR", character, false, |w| w.proto(&msg))
}

/// A resurrection point (World Map teleport spot), data/resurrection_points.json
/// (tools/re/extract_resurrection_points.py, from RDB 1000090).
#[derive(Debug, Clone, Deserialize)]
pub struct Spot {
    pub guid: u32,
    pub pos: [f32; 3],
    pub rot: f32,
    pub restoration: u32,
    pub shady: bool,
}

static SPOTS: LazyLock<HashMap<String, Vec<Spot>>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../data/resurrection_points.json")).expect("resurrection_points.json")
});

/// The teleport spot `guid` of `playfield`.
pub fn spot(playfield: u32, guid: u32) -> Option<&'static Spot> {
    SPOTS.get(&playfield.to_string())?.iter().find(|s| s.guid == guid)
}

/// SaveDataVec_t of a playfield's spots, all unlocked: u32 (n+1)·1009, then per spot
/// (SaveDataEntry_t, reader 0x135c4ed) position, ID32 {51100, playfield}, u32 0, u32 0, u32 guid,
/// u32 restoration level, u8 shady, u8 discoverable, u8 unlocked. The map shows a spot as usable
/// when it isn't discoverable or is unlocked.
fn spots(w: lmo_net::Writer, playfield: u32) -> lmo_net::Writer {
    let list = SPOTS.get(&playfield.to_string()).map(Vec::as_slice).unwrap_or(&[]);
    list.iter().fold(w.u32((list.len() as u32 + 1) * 1009), |w, s| {
        let w = w.f32(s.pos[0]).f32(s.pos[1]).f32(s.pos[2]);
        Id32::new(PLAYFIELD_TYPE, playfield).write(w).u32(0).u32(0).u32(s.guid).u32(s.restoration).u8(s.shady as u8).u8(0).u8(1)
    })
}

/// `GameCharInterface::ValidResurrectionSpots(Vector3Member position, SaveDataVec_t)`: the spots
/// of the player's current playfield (the World Map's own-world markers).
pub fn valid_spots_args(playfield: u32, pos: [f32; 3]) -> Bytes {
    spots(lmo_net::Writer::new().f32(pos[0]).f32(pos[1]).f32(pos[2]), playfield).finish()
}

/// `GameserverToClientRPC::SignalResurrectionSpotsReceived(InstanceId playfield, Vector3Member
/// centre, SaveDataVec_t)`: the answer to `RequestResurrectionSpots` when the map shows another
/// world.
pub fn signal_spots_args(playfield: u32) -> Bytes {
    let centre = destination(playfield)
        .map(|d| d.pos)
        .or_else(|| SPOTS.get(&playfield.to_string()).and_then(|l| l.first()).map(|s| s.pos))
        .unwrap_or_default();
    spots(lmo_net::Writer::new().u32(playfield).f32(centre[0]).f32(centre[1]).f32(centre[2]), playfield).finish()
}
