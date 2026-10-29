//! Travel between playfields (worlds), within the same game connection.
//!
//! Triggers: the world map sends `GameCharAgent::TeleportMe(playfield, spawnPointID)` (the
//! spawn point is a resurrection point Guid, RDB 1000090); the teleport window (opened by the
//! server with `GameCharInterfaceProject::ScryShowTeleportMessageBox(windowId, ctx)`) asks
//! `ClientRequestDestinationPlayfieldStatus(PlayfieldProxy)`, needs
//! `ReplyToClientRequestDestinationPlayfieldStatus(ready, completed, empty)` before it shows its
//! buttons, and answers `GameCharAgentProject::TeleportMessageBoxAnswer(playfield or -1, ctx)`.
//! Hub portals send nothing: the server notices the player standing in one and opens that
//! destination's teleport window. The windows require completed quests (story.rs `journal`,
//! tunables `story.windows_unlocked`). Portals whose window would be empty ask with a Yes/No
//! message box instead (`PLAIN_WINDOWS`). Each portal also has a door (type 51016, invisible,
//! usable) as live sent one (LMO_Captures.cap): clicking it or pressing E on it queues an
//! ObjectInteraction on the door, which opens the same window wherever the player stands.
//!
//! The move: `IntendedTeleportIIR_c` + `n3TeleportIIR_t` (the client stops the old playfield and
//! goes to FlowControl DeepTeleport), then the new playfield's records as at login; the client
//! answers with CharInPlayIIR_t again.

use crate::instance::Pf;
use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{PlayfieldProxyData, ProtoGameCodeDynelIir, ProtoId32, ProtoQuat, ProtoSimpleDynelIir, ProtoVec3},
    types::Id32,
};
use serde::Deserialize;

use crate::{
    game::{PLAYFIELD_TYPE, create_game_object},
    world,
};

#[derive(Debug, Clone, Deserialize)]
pub struct Destination {
    #[serde(default)]
    pub name: String,
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
    /// The door players click (world_data.rs `portal_door`).
    door: Option<Door>,
}

/// A portal's door: an invisible `empty_monsterdata_1x1x1` (the client data's door spawns).
#[derive(Debug, Clone, Deserialize)]
struct Door {
    guid: u32,
    name: String,
    pos: [f32; 3],
    rot: [f32; 4],
    scale: f32,
    /// [resource type, visual id]
    visual: [u32; 2],
}

#[derive(Deserialize)]
struct Data {
    destinations: HashMap<String, Destination>,
    portals: HashMap<String, Vec<Portal>>,
}

static DATA: LazyLock<Data> =
    LazyLock::new(|| serde_json::from_value(crate::world_data::DESTINATIONS.clone()).expect("destinations"));

/// How close to a portal's position counts as standing in it.
const PORTAL_RADIUS: f32 = 3.0;
/// How far from every portal a player who arrived in one must go before portals open (assumed).
const REARM_DISTANCE: f32 = 8.0;

/// Where a player arrives in playfield `playfield` (a destination: the copy is chosen apart).
pub fn destination(playfield: u32) -> Option<&'static Destination> {
    DATA.destinations.get(&playfield.to_string())
}

/// Every destination, by playfield.
pub fn destinations() -> impl Iterator<Item = (u32, &'static Destination)> {
    DATA.destinations.iter().filter_map(|(k, d)| Some((k.parse().ok()?, d)))
}

/// Arrival points next to each playfield's portals, by the playfield the portal leads to
/// (tools/re/extract_return_points.py). Live used source-specific "TP_From<source>" points whose
/// positions were server-side; these stand in for them (docs/TUNABLES.md).
static RETURNS: LazyLock<HashMap<String, HashMap<String, Destination>>> =
    LazyLock::new(|| serde_json::from_value(serde_json::json!(*crate::world_data::RETURN_POINTS)).expect("return points"));

/// Where to arrive in `playfield` when coming back from `from` through a teleport window.
pub fn return_point(playfield: u32, from: u32) -> Option<&'static Destination> {
    RETURNS.get(&playfield.to_string())?.get(&from.to_string())
}

/// The hub, where portals leading back into their own world go (the worlds' gates home: e.g.
/// the Medieval Kingdom's only nearby portal is listed as leading to the Kingdom itself).
const HUB: u32 = 1200;

/// The portal the player is standing in, if any: (destination playfield, teleport window).
/// The client has no portal trigger of its own; live, server scripts opened the window.
pub fn portal_at(playfield: Pf, pos: [f32; 3]) -> Option<(u32, u32)> {
    let portal = DATA.portals.get(&playfield.model().to_string())?.iter().find(|p| distance(p.pos, pos) <= PORTAL_RADIUS)?;
    Some((if portal.to == playfield.model() { HUB } else { portal.to }, portal.window))
}

/// Dynel type of doors.
pub const DOOR_TYPE: u32 = 51016;
/// Usable (ObjectInteraction), as live sent the portal doors.
const CARS_USABLE: u32 = 1;
/// The text category of the doors' names (TDB 55000, as live named them).
const NAME_CATEGORY: u32 = 55000;

/// The portal whose door is `guid`: (destination playfield, teleport window).
pub fn door_portal(guid: u32) -> Option<(u32, u32)> {
    DATA.portals.iter().find_map(|(k, ps)| {
        let playfield: u32 = k.parse().ok()?;
        let p = ps.iter().find(|p| p.door.as_ref().is_some_and(|d| d.guid == guid))?;
        Some((if p.to == playfield { HUB } else { p.to }, p.window))
    })
}

/// The portals' doors by playfield, each once (two portals can share one: the tutorial's
/// compass).
static DOORS_OF: LazyLock<HashMap<u32, Vec<Door>>> = LazyLock::new(|| {
    let mut seen = HashSet::new();
    let mut doors: HashMap<u32, Vec<Door>> = HashMap::new();
    for (k, ps) in &DATA.portals {
        let Ok(playfield) = k.parse::<u32>() else { continue };
        for d in ps.iter().filter_map(|p| p.door.clone()).filter(|d| seen.insert(d.guid)) {
            doors.entry(playfield).or_default().push(d);
        }
    }
    doors
});

/// The doors in the world, by copy.
static DOORS: LazyLock<Mutex<HashMap<Pf, Vec<world::Presence>>>> = LazyLock::new(Default::default);

pub fn start() {
    tracing::info!(count = DOORS_OF.values().map(Vec::len).sum::<usize>(), "portal doors read");
}

/// Puts the portals' doors of copy `pf` in (worlds.rs).
pub fn spawn_world(pf: Pf) {
    let mut presences = Vec::new();
    for d in DOORS_OF.get(&pf.model()).into_iter().flatten().cloned() {
        let (outbox, mut inbox) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move { while inbox.recv().await.is_some() {} });
        let id = Id32::new(DOOR_TYPE, crate::instance::fixture(pf, d.guid));
        let pos = std::sync::Arc::new(Mutex::new(d.pos));
        presences.push(world::join(id, pf, pos, move || door_records(&d, pf), outbox));
    }
    DOORS.lock().unwrap().insert(pf, presences);
}

/// Takes the doors of copy `pf` out.
pub fn despawn_world(pf: Pf) {
    let gone = DOORS.lock().unwrap().remove(&pf);
    drop(gone);
}

/// UpdateMessage + `DoorFullUpdateIIR_c` as live sent a portal's door: ProtoGameCodeDynelIIR,
/// ProtoSimpleDynelIIR (no collision, its name, the TDB 55000 name, CARS group 1, field 7 =
/// 51016, closed), visual, u32 0, an empty box (all f32 max), scale, u32 0, u8 0, u32 stat 169
/// (0), u32 the destination's element (DoorFullUpdateIIR_c::ReadSubClass: the door's stat
/// ElementalType, its map marker's element).
fn door_records(d: &Door, playfield: Pf) -> Vec<Bytes> {
    let element = door_portal(d.guid).map_or(0, |(to, _)| crate::worlds::element_of(to));
    door_dynel_records(d.guid, &d.name, d.pos, d.rot, d.scale, d.visual, element, playfield)
}

/// A door's spawn records (`door_records`): any usable door, `element` its destination's.
#[allow(clippy::too_many_arguments)]
pub fn door_dynel_records(guid: u32, name: &str, pos: [f32; 3], rot: [f32; 4], scale: f32, visual: [u32; 2], element: u32, playfield: Pf) -> Vec<Bytes> {
    let d = Door { guid, name: name.to_string(), pos, rot, scale, visual };
    let id = Id32::new(DOOR_TYPE, crate::instance::fixture(playfield, d.guid));
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(d.pos[0]), y: Some(d.pos[1]), z: Some(d.pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(d.rot[0]), y: Some(d.rot[1]), z: Some(d.rot[2]), w: Some(d.rot[3]) }),
        playfield_instance: Some(playfield.key()),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(0),
        name: Some(d.name.clone()),
        display_name: Some(format!(r#"<localized id="{}" category="{NAME_CATEGORY}" flags="" >"#, d.guid)),
        cars_group: Some(CARS_USABLE),
        waypoint_type: Some(DOOR_TYPE),
        is_closed: Some(true),
        ..Default::default()
    };
    let record = iir::record("DoorFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(d.visual[1]).u32(d.visual[0]).u32(0);
        (0..6).fold(w, |w, _| w.f32(f32::MAX)).f32(d.scale).u32(0).u8(0).u32(0).u32(element)
    });
    vec![create_game_object(id, playfield, d.pos), record]
}

/// Teleport windows whose RDB entry names only the destination (no picture, level or texts): the
/// client shows them as an empty window (73 to the LEGO Center, 74 to Pirate World). Their
/// portals ask with a plain Yes/No message box instead. (window, destination)
const PLAIN_WINDOWS: &[(u32, u32)] = &[(73, HUB), (74, 4600)];

/// Whether a portal opening `window` asks with a message box instead.
pub fn plain_window(window: u32) -> bool {
    plain_window_destination(window).is_some()
}

/// Where a plain window leads.
pub fn plain_window_destination(window: u32) -> Option<u32> {
    PLAIN_WINDOWS.iter().find(|&&(w, _)| w == window).map(|&(_, to)| to)
}

/// The question a portal with a plain window asks.
pub fn travel_question(playfield: u32) -> String {
    let name = destination(playfield).map_or_else(|| playfield.to_string(), |d| d.name.clone());
    format!("Do you want to travel to {name}?")
}

/// Every teleport window a portal opens: (window, destination, the playfields with such a
/// portal), by window.
pub fn windows() -> Vec<(u32, u32, Vec<u32>)> {
    let mut out: Vec<(u32, u32, Vec<u32>)> = Vec::new();
    for (k, ps) in &DATA.portals {
        let Ok(playfield) = k.parse::<u32>() else { continue };
        for p in ps {
            let to = if p.to == playfield { HUB } else { p.to };
            match out.iter_mut().find(|(w, t, _)| *w == p.window && *t == to) {
                Some((_, _, from)) if !from.contains(&playfield) => from.push(playfield),
                Some(_) => {}
                None => out.push((p.window, to, vec![playfield])),
            }
        }
    }
    for (_, _, from) in &mut out {
        from.sort();
    }
    out.sort();
    out
}

/// Every portal, for the test commands: (label, playfield, position), by playfield.
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    let mut out: Vec<_> = DATA
        .portals
        .iter()
        .filter_map(|(k, ps)| Some((k.parse::<u32>().ok()?, ps)))
        .flat_map(|(playfield, ps)| {
            ps.iter().map(move |p| {
                let to = if p.to == playfield { HUB } else { p.to };
                let name = destination(to).map_or_else(|| to.to_string(), |d| d.name.clone());
                let how = if plain_window(p.window) { "asks Yes/No".to_string() } else { format!("window {}", p.window) };
                (format!("to {name} ({how})"), playfield, p.pos)
            })
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.2[0].total_cmp(&b.2[0])));
    out
}

/// Characters teleported since their last movement report (game.rs keeps their portals shut
/// until they are clear of them).
static ARRIVED: LazyLock<Mutex<HashSet<u32>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// `character` was just teleported.
pub fn arrived(character: u32) {
    ARRIVED.lock().unwrap().insert(character);
}

/// Whether `character` was teleported since this was last asked.
pub fn take_arrival(character: u32) -> bool {
    ARRIVED.lock().unwrap().remove(&character)
}

/// Whether the player at `pos` is well clear of every portal (`REARM_DISTANCE`): someone who
/// arrives in a portal (logging in, teleporting to a world whose arrival point is next to one,
/// e.g. Volcano Island's by Pioneer Bay) must walk this far away before it opens for them.
pub fn clear_of_portals(playfield: Pf, pos: [f32; 3]) -> bool {
    DATA.portals.get(&playfield.model().to_string()).is_none_or(|ps| ps.iter().all(|p| distance(p.pos, pos) > REARM_DISTANCE))
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// PlayfieldProxy as embedded in a BinaryStream (u32 length + PlayfieldProxyData): the copy, its
/// element (live sent it for every zone: the minimap ring) and, in a Lost Creations level, the
/// run's fields (lost_creations.rs).
pub(crate) fn proxy(playfield: Pf) -> PlayfieldProxyData {
    let proxy = PlayfieldProxyData {
        dimension_id: Some(1),
        model_id: Some(ProtoId32 { r#type: Some(PLAYFIELD_TYPE), instance: Some(playfield.model()) }),
        running_instance: Some(playfield.key()),
        elemental_type: Some(crate::worlds::element(playfield)),
        ..Default::default()
    };
    crate::lost_creations::proxy(playfield, proxy)
}

/// `IntendedTeleportIIR_c`: PlayfieldProxy, u8 local (0: a different playfield), u8.
pub fn intended_teleport_record(character: Id32, playfield: Pf) -> Bytes {
    iir::record("IntendedTeleportIIR_c", character, false, |w| w.proto(&proxy(playfield)).u8(0).u8(0))
}

/// `n3TeleportIIR_t` (ReadSubClass 0x12e82a8): position, rotation quaternion, PlayfieldProxy,
/// ID32, BinaryStream (empty: u32 0), u8.
pub fn teleport_record(character: Id32, playfield: Pf, pos: [f32; 3], rot: f32) -> Bytes {
    let (s, c) = (rot / 2.0).sin_cos();
    iir::record("n3TeleportIIR_t", character, false, |w| {
        let w = w.f32(pos[0]).f32(pos[1]).f32(pos[2]).f32(0.0).f32(s).f32(0.0).f32(c);
        Id32::default().write(w.proto(&proxy(playfield))).u32(0).u8(0)
    })
}

/// One main quest of the journal at login: its completed quests (quest, distance: a story
/// step's place in its line, repeat count).
pub struct JournalMain {
    pub main: u32,
    pub quests: Vec<(u32, u32, u32)>,
}

/// `JournalUpdateIIR` (header protobuf JournalUpdateIIRProto) at login: the completed quests
/// (story.rs `journal`: the story steps done, the area quests, and the quests the teleport
/// windows require, which they check with CoreJournalHolder::HasEverCompletedQuest).
pub fn journal_record(character: Id32, mains: &[JournalMain]) -> Bytes {
    use lmoxide_net::proto::{
        JournalUpdateIirProto,
        journal_update_iir_proto::{AddedMainQuestType, AddedQuestType},
    };
    let added_main_quest = mains
        .iter()
        .map(|m| AddedMainQuestType {
            main_quest_id: Some(m.main),
            quest: m
                .quests
                .iter()
                .map(|&(q, distance, repeat)| AddedQuestType {
                    quest_id: Some(q),
                    failed: Some(false),
                    distance: (distance > 0).then_some(distance),
                    repeat_count: Some(repeat),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
        .collect();
    let msg = JournalUpdateIirProto { added_main_quest, ..Default::default() };
    iir::record("JournalUpdateIIR", character, false, |w| w.proto(&msg))
}

/// A resurrection point (World Map teleport spot), the worlds' `respawn_points` (world_data.rs)
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
    serde_json::from_value(serde_json::json!(*crate::world_data::RESURRECTION_POINTS)).expect("resurrection points")
});

/// The teleport spot `guid` of `playfield`.
pub fn spot(playfield: u32, guid: u32) -> Option<&'static Spot> {
    SPOTS.get(&playfield.to_string())?.iter().find(|s| s.guid == guid)
}

/// The teleport spot of `playfield` nearest to `pos` (where a smashed team respawns).
pub fn nearest_spot(playfield: u32, pos: [f32; 3]) -> Option<&'static Spot> {
    let d = |s: &Spot| (s.pos[0] - pos[0]).powi(2) + (s.pos[1] - pos[1]).powi(2) + (s.pos[2] - pos[2]).powi(2);
    SPOTS.get(&playfield.to_string())?.iter().min_by(|a, b| d(a).total_cmp(&d(b)))
}

/// SaveDataVec_t of a playfield's spots, all unlocked: u32 (n+1)·1009, then per spot
/// (SaveDataEntry_t, reader 0x135c4ed) position, ID32 {51100, playfield}, u32 0, u32 0, u32 guid,
/// u32 restoration level, u8 shady, u8 discoverable, u8 unlocked. The map shows a spot as usable
/// when it isn't discoverable or is unlocked.
fn spots(w: lmoxide_net::Writer, playfield: u32) -> lmoxide_net::Writer {
    let list = SPOTS.get(&playfield.to_string()).map(Vec::as_slice).unwrap_or(&[]);
    list.iter().fold(w.u32((list.len() as u32 + 1) * 1009), |w, s| {
        let w = w.f32(s.pos[0]).f32(s.pos[1]).f32(s.pos[2]);
        Id32::new(PLAYFIELD_TYPE, playfield).write(w).u32(0).u32(0).u32(s.guid).u32(s.restoration).u8(s.shady as u8).u8(0).u8(1)
    })
}

/// `GameCharInterface::ValidResurrectionSpots(Vector3Member position, SaveDataVec_t)`: the spots
/// of the player's current playfield (the World Map's own-world markers).
pub fn valid_spots_args(playfield: Pf, pos: [f32; 3]) -> Bytes {
    spots(lmoxide_net::Writer::new().f32(pos[0]).f32(pos[1]).f32(pos[2]), playfield.model()).finish()
}

/// `GameserverToClientRPC::SignalResurrectionSpotsReceived(InstanceId playfield, Vector3Member
/// centre, SaveDataVec_t)`: the answer to `RequestResurrectionSpots` when the map shows another
/// world.
pub fn signal_spots_args(playfield: u32) -> Bytes {

    let centre = destination(playfield)
        .map(|d| d.pos)
        .or_else(|| SPOTS.get(&playfield.to_string()).and_then(|l| l.first()).map(|s| s.pos))
        .unwrap_or_default();
    spots(lmoxide_net::Writer::new().u32(playfield).f32(centre[0]).f32(centre[1]).f32(centre[2]), playfield).finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn door_guids_leave_the_copy_byte_free() {
        assert!(DOORS_OF.values().flatten().all(|d| d.guid < 1 << 24));
    }

    #[test]
    fn portal_doors() {
        assert_eq!(iir::type_id("DoorFullUpdateIIR_c"), 0x33a5_6fb0);
        // the hub's Mythology Mountains portal: live's door 525862 (scale 3)
        let door = DATA.portals["1200"].iter().find_map(|p| p.door.as_ref().filter(|d| d.guid == 525862)).expect("door");
        assert_eq!((door.scale, door.visual), (3.0, [1010201, 465832]));
        assert_eq!(door_portal(525862).map(|(_, w)| w), Some(67));
        // Monster Laboratory's door stands 4.4 m off its portal point
        assert!(door_portal(656807).is_some());
        // the record ends as live's did (LMO_Captures.cap, the hub's door 7951): visual, u32 0,
        // empty box, scale 3, zeros
        let record = &door_records(door, Pf::main(1200))[1];

        let tail = "00071ba8000f6a1900000000".to_string() + &"7f7fffff".repeat(6) + "40400000" + &"00".repeat(13);
        assert!(crate::conn::hex(record).ends_with(&tail));
        // and its protobuf: no collision, the name, CARS group 1, field 7 = 51016, closed
        // (live's name ended in a space)
        assert!(crate::conn::hex(record).contains("08001217656d7074795f6d6f6e73746572646174615f3178317831"));
        assert!(crate::conn::hex(record).contains("280138c88e036001"));
        // a dungeon's door ends with its element, as live's did (Isle of Yarr's to Natives'
        // Hollow: Magic)
        let (guid, _) = DATA.portals["4200"].iter().find(|p| p.to == 1640).and_then(|p| p.door.as_ref()).map(|d| (d.guid, ())).expect("door to 1640");
        let d = DOORS_OF[&4200].iter().find(|d| d.guid == guid).unwrap();
        assert!(crate::conn::hex(&door_records(d, Pf::main(4200))[1]).ends_with("00000004"));
    }

    #[test]
    fn return_points_stay_clear_of_portals() {
        // coming back to the hub from Isle of Yarr: next to the hub's Isle of Yarr portal
        let p = return_point(1200, 4200).expect("hub return point");
        let portal = DATA.portals["1200"].iter().find(|q| q.to == 4200).unwrap();
        let d = ((p.pos[0] - portal.pos[0]).powi(2) + (p.pos[2] - portal.pos[2]).powi(2)).sqrt();
        assert!(d > PORTAL_RADIUS && d < 10.0, "{d}");
    }
}
