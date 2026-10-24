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
    pub name: String,
    /// Arrival point (a resurrection point; its Guid is in the data file).
    pub pos: [f32; 3],
    /// Facing (radians around the vertical axis).
    pub rot: f32,
    /// Teleport window id (RDB 1000644/1); 0 if none.
    pub window: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct Portal {
    to: u32,
    pos: [f32; 3],
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

/// The portal (destination playfield) the player is standing in, if any.
pub fn portal_at(playfield: u32, pos: [f32; 3]) -> Option<u32> {
    DATA.portals.get(&playfield.to_string())?.iter().find(|p| distance(p.pos, pos) <= PORTAL_RADIUS).map(|p| p.to)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// PlayfieldProxy as embedded in a BinaryStream (u32 length + PlayfieldProxyData).
fn proxy(playfield: u32) -> PlayfieldProxyData {
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
