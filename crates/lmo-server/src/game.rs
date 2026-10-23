//! Game server: the playfield connection the client opens after `LoginConnectReady`
//! (`ClientToGameserverRPC` / `GameserverToClientRPC`, then the gamecode endpoints).

use std::sync::{Arc, Mutex};

use bytes::Bytes;

use lmo_net::{
    Endpoint, Reader, Writer, iir,
    methods::{
        client_to_gameserver_rpc as agent, game_char_agent, game_char_interface,
        gameserver_to_client_rpc as client,
    },
    proto::{
        PlayfieldProxyData, ProtoAcgItem, ProtoCoreCharacterIir, ProtoFullCharacterIir,
        ProtoGameCodeDynelIir, ProtoId32, ProtoInventoryGame, ProtoInventoryGameEntry, ProtoQuat,
        ProtoVec3, proto_core_character_iir, proto_inventory_game::InventoryListEntry,
    },
    types::Id32,
};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{Config, conn::Conn, world};

pub async fn handle(mut conn: Conn, cfg: Arc<Config>) -> anyhow::Result<()> {
    conn.local = Endpoint::ClientToGameserverRPC;
    let status: i32 = std::env::var("LMO_GAME_AUTH_STATUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut character = 0;
    let mut name = String::new();
    let pos = Arc::new(Mutex::new(START_POS));
    let (outbox, mut inbox) = mpsc::unbounded_channel::<Vec<Bytes>>();
    let mut presence: Option<world::Presence> = None;
    loop {
        let rpc = tokio::select! {
            rpc = conn.recv() => match rpc? {
                Some(rpc) => rpc,
                None => break,
            },
            Some(records) = inbox.recv() => {
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records))
                    .await?;
                continue;
            }
        };
        let mut r = Reader::new(&rpc.args);
        match (rpc.target(), rpc.method()) {
            (Some(Endpoint::ClientToGameserverRPC), agent::AUTHENTICATE2) => {
                character = r.u32()?;
                let cookie = r.u32()?;
                let version = r.string()?;
                let valid = cookie == crate::universe::cookie_for(character);
                name = crate::sessions::name(character).unwrap_or_else(|| "Player".into());
                info!(character, cookie, %version, valid, %name, "Authenticate2");
                let args = Writer::new().i32(status).finish();
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::ACK_AUTHENTICATE, args)
                    .await?;
                // Characters are keyed by account instance for now (see territory.rs).
                if !crate::sessions::territory_closed(character, std::time::Duration::from_secs(10)).await {
                    warn!(character, "territory connection still open; sending the world anyway");
                }
                // Give the client a moment to process the territory disconnect.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let me = Id32::new(crate::territory::CHARACTER_TYPE, character);
                let mut records = vec![playfield_full_update(cfg.start_playfield)];
                records.extend(spawn_records(me, cfg.start_playfield, &name, START_POS));
                records.insert(3, full_player_character(me));
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records))
                    .await?;
            }
            (Some(Endpoint::ClientToGameserverRPC), agent::SIGNAL_PASS_BLOB) => {
                // The client's own IIRs (u32 length + records). Only headers are decoded; the
                // client sends one record per blob.
                let len = r.u32()? as usize;
                let blob = r.bytes(len, "blob")?;
                let mut br = Reader::new(blob);
                let h = iir::Header::read(&mut br)?;
                let rest = br.remaining();
                debug!(iir = h.name().unwrap_or("?"), identity = ?h.identity, rest = %crate::conn::hex(rest), "client IIR");
                match h.name() {
                    Some("CharInPlayIIR_t") => {
                        // Acknowledge: the client waits in WaitingToReceiveInPlay for this.
                        let echo = iir::pass_blob(&[iir::record("CharInPlayIIR_t", h.identity, false, |w| w)]);
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, echo)
                            .await?;
                        if presence.is_none() {
                            let (id, playfield, name, pos_handle) = (h.identity, cfg.start_playfield, name.clone(), pos.clone());
                            let pos = pos.clone();
                            let spawn = move || spawn_records(id, playfield, &name, *pos.lock().unwrap());
                            presence = Some(world::join(id, playfield, pos_handle, spawn, outbox.clone()));
                        }
                    }
                    Some("CharDCMoveIIR_t" | "MoveTaskIIR_c") => {
                        if h.name() == Some("CharDCMoveIIR_t") && rest.len() >= 13 {
                            let f = |i: usize| f32::from_be_bytes(rest[i..i + 4].try_into().unwrap());
                            *pos.lock().unwrap() = [f(1), f(5), f(9)];
                        }
                        if let Some(p) = &presence {
                            world::broadcast(p.id(), vec![Bytes::copy_from_slice(blob)]);
                        }
                    }
                    _ => info!(iir = h.name().unwrap_or("?"), identity = ?h.identity, rest = %crate::conn::hex(rest), "client IIR"),
                }
            }
            (Some(Endpoint::ClientToGameserverRPC), agent::PING) => {
                let seq = r.u32()?;
                let args = Writer::new().u32(seq).finish();
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PONG, args).await?;
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::PING) => {
                // Pong(uint32 i_nSequence, bigtime_t i_nServerTime); server time in microseconds
                let seq = r.u32()?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_micros() as u64);
                let args = Writer::new().u32(seq).u64(now).finish();
                conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::PONG, args)
                    .await?;
            }
            // Client-side bookkeeping that needs no answer (to be persisted later).
            (
                Some(Endpoint::GameCharAgent),
                game_char_agent::TUTORIAL_SHOWN
                | game_char_agent::ADD_SHORTCUT
                | game_char_agent::REMOVE_SHORTCUT
                | game_char_agent::SET_PLAYER_GUI_LAYOUT_MODE,
            ) => {}
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    Ok(())
}

/// The IIRs that make a character exist on a client: its DreamWorld GameObject, the dynel,
/// and its stats. The player's own client additionally needs `full_player_character`.
pub fn spawn_records(id: Id32, playfield: u32, name: &str, pos: [f32; 3]) -> Vec<Bytes> {
    vec![
        create_game_object(id, playfield, pos),
        simple_char_full_update(id, playfield, name, pos),
        int32_stat(id, STAT_LIFE, START_HEALTH),
        int32_stat(id, STAT_HEALTH, START_HEALTH),
        int32_stat(id, STAT_LEVEL, 1),
        iir::record("CharInPlayIIR_t", id, false, |w| w),
    ]
}

/// `UpdateMessage` op 1 (Activate 0x6ef4e0): removes the GameObject with this Id.
pub fn despawn_record(id: Id32) -> Bytes {
    iir::record("UpdateMessage", id, false, |w| id.write(w.u8(1)))
}

/// `CharDCMoveIIR_t` (ReadSubClass 0x605e40, direct-control/keyboard movement): u8 flags
/// (bit 4: yaw-only rotation, bits 2/3: no velocity blocks), position, rotation y and w, u8 mode,
/// f32, u8, u8, as the client sends it. Activate (0x605a90) only moves non-local dynels whose
/// movement state allows it; the bot uses `move_task_record` instead, which works.
#[allow(dead_code)]
pub fn move_record(id: Id32, pos: [f32; 3], heading: [f32; 2]) -> Bytes {
    iir::record("CharDCMoveIIR_t", id, false, |w| {
        w.u8(0x1c).f32(pos[0]).f32(pos[1]).f32(pos[2]).f32(heading[0]).f32(heading[1]).u8(0).f32(0.0).u8(0).u8(0)
    })
}

/// `MoveTaskIIR_c` as the client sends it for click-to-move: u8 0, start position, u8 0x0f,
/// 10 zero bytes, f32 (0.07 in samples), u32 n, n waypoints, 16 zero bytes. Layout copied from
/// client traffic; field meanings beyond the positions are unknown.
pub fn move_task_record(id: Id32, from: [f32; 3], to: [f32; 3]) -> Bytes {
    iir::record("MoveTaskIIR_c", id, false, |w| {
        let w = w.u8(0).f32(from[0]).f32(from[1]).f32(from[2]).u8(0x0f).u32(0).u32(0).u16(0).f32(0.072).u32(2);
        let w = [from, to].iter().fold(w, |w, p| w.f32(p[0]).f32(p[1]).f32(p[2]));
        w.u32(0).u32(0).u32(0).u32(0)
    })
}

/// A server-side character that circles whoever else is on the playfield, standing in for a
/// second player (`--bot`).
pub async fn run_bot(playfield: u32) {
    let id = Id32::new(crate::territory::CHARACTER_TYPE, 0x0b07);
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    let pos = Arc::new(Mutex::new(START_POS));
    let spawn_pos = pos.clone();
    let spawn = move || spawn_records(id, playfield, "Bot", *spawn_pos.lock().unwrap());
    let _presence = world::join(id, playfield, pos.clone(), spawn, outbox);
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    let mut t = 0.0f32;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(1000));
    loop {
        tick.tick().await;
        t += 0.6;
        let center = world::someone_else(id, playfield).unwrap_or(START_POS);
        let from = *pos.lock().unwrap();
        let to = [center[0] + 4.0 * t.cos(), center[1], center[2] + 4.0 * t.sin()];
        *pos.lock().unwrap() = to;
        world::broadcast(id, vec![move_task_record(id, from, to)]);
    }
}

/// ID32 type of playfields (key of the client's PlayfieldFactory map, 0x96cee0).
pub const PLAYFIELD_TYPE: u32 = 51100;

/// `PlayfieldFIIR_t`: n3PlayfieldFullUpdateIIR_t::ReadSubClass (0x7f60f0) = u32, u32, u32,
/// PlayfieldProxyData protobuf, u32, blob, blob; then PlayfieldFIIR_t (0x9d9b10) adds 6 u32.
/// Activate (0x7f5670) creates the playfield from the PlayfieldProxyData model id; the second
/// phase (0x7f5d30) reads blob 2 as dynamic pathfinding data (0xbd3900: u32 count, entries) and
/// blob 1 as a u32 count of nested IIRs. Both must hold at least a count: reading a u32 from an
/// empty stream yields garbage, which made the client allocate until it ran out of memory.
fn playfield_full_update(playfield: u32) -> bytes::Bytes {
    let id = Id32::new(PLAYFIELD_TYPE, playfield);
    let proxy = PlayfieldProxyData {
        dimension_id: Some(1),
        model_id: Some(ProtoId32 { r#type: Some(PLAYFIELD_TYPE), instance: Some(playfield) }),
        // key of the client's playfield map ("Inserting playfield N in map"); dynels refer to it
        // via ProtoGameCodeDynelIIR.playfield_instance
        running_instance: Some(playfield),
        ..Default::default()
    };
    iir::record("PlayfieldFIIR_t", id, false, |w| {
        let empty_list = 0u32.to_be_bytes();
        let w = w.u32(0).u32(0).u32(0).proto(&proxy).u32(0).blob(&empty_list).blob(&empty_list);
        (0..6).fold(w, |w, _| w.u32(0))
    })
}

/// Version the client's CoreFullPlayerCharacterIIR reader expects (0x1b81edc).
const FULL_CHARACTER_VERSION: u32 = 24;
/// Empty 1009-encoded container.
const EMPTY: u32 = 0x3f1;

/// `FullPlayerCharacterIIR` (the player's own character), minimal: every list empty.
/// Header protobuf `ProtoFullCharacterIIR`, then CoreFullPlayerCharacterIIR::ReadSubClass
/// (0x602ed0) and FullPlayerCharacterIIR's additions (0xe3f9f0):
///
/// ```text
/// u32 version (24)
/// u32 n,   n x u32                        0x9cd8c0
/// cont<u32>                               0x5f8d80
/// u32 n,   n x element                    0x9cdf60
/// cont<u32 -> u32>                        0x60aa60
/// cont<u32 -> element>                    0x630ab0
/// cont<u32 -> u32>                        0x5f9f70
/// cont<ID32>, cont<ID32>                  0x970930 (IdentityLists)
/// [0x67d240 if flag +0xf1]
/// u32 n,   n x (u32, u64, u64)
/// u32 n,   n x element                    0x5dafa0
/// u32 n,   n x u32
/// cont<u32 -> 5 x u32>                    0xa83b30
/// u32 n,   n x (u32, u32)
/// u32 n,   n x (u32, u32)
/// cont<u32 -> element>                    0xe43450
/// ```
fn full_player_character(id: Id32) -> bytes::Bytes {
    let proto = ProtoFullCharacterIir {
        inventory: vec![minifigure_inventory(id.instance)],
        ..Default::default()
    };
    iir::record("FullPlayerCharacterIIR", id, false, |w| {
        w.proto(&proto)
            .u32(FULL_CHARACTER_VERSION)
            .u32(0)
            .u32(EMPTY)
            .u32(0)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(0)
            .u32(0)
            .u32(0)
            .u32(EMPTY)
            .u32(0)
            .u32(0)
            .u32(EMPTY)
    })
}

const START_HEALTH: u32 = 100;
/// MonsterData (RDB type 1010514) of every minifigure item template: the shared animated body.
/// Without it remote characters are invisible ("has no animated mesh", 0x5f2960).
const MINIFIGURE_MONSTER_DATA: u32 = 2212;

/// Engine stat ids (data/stats.json, from the client's stat table at 0x1607bd0).
const STAT_LIFE: u32 = 1;
const STAT_HEALTH: u32 = 27;
const STAT_LEVEL: u32 = 54;

/// Near the portals in playfield 1200 (from the client's RDBTeleportLocations).
const START_POS: [f32; 3] = [320.0, 56.0, 110.0];

/// `SimpleCharFullUpdateIIR_t`: creates the character dynel (it is a GameCodeDynelFullUpdateIIR,
/// i.e. an n3DynelFactoryCallbackObject). Header protobuf `ProtoGameCodeDynelIIR` (position,
/// rotation, playfield). SimpleChar part (0xe0b150): u8 version 4, u8, 5 u32, string, 4 u32,
/// u32 n + n u32, u32 n + n (u32,u32), u32 n + n (u32,u32); then CoreCharacterFullUpdateIIR
/// (0x5f4650): u8 'B', u32, ProtoCoreCharacterIIR (generic_stats must have exactly as many
/// entries as g_Int32StatsOrder, which holds one stat), u32 n + n u32, blob, cont, cont<u32,u8>,
/// cont, u32 n + n (u32,u32), u32 n + n (u32,u32), cont<u32,u32>, cont<u32,u32>.
/// `Int32StatIIR` (ReadSubClass 0x87a940): u32 stat, u32 value. Applying it (0x87b270) calls
/// the dynel's `StatHolder_t` (+0xac) SetStat — the same holder Flash's `GetStat` reads, so this
/// is what the HUD health bar (`GetStat(e_Health) / GetStat(e_Life)`) sees.
fn int32_stat(id: Id32, stat: u32, value: u32) -> bytes::Bytes {
    iir::record("Int32StatIIR", id, false, |w| w.u32(stat).u32(value))
}

fn simple_char_full_update(id: Id32, playfield: u32, name: &str, pos: [f32; 3]) -> bytes::Bytes {
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield),
        running_effects: vec![],
    };
    let core = ProtoCoreCharacterIir {
        name: Some(name.to_string()),
        stats: Some(proto_core_character_iir::CharacterStats {
            life: Some(START_HEALTH),
            health: Some(START_HEALTH),
            level: Some(1),
            monster_data: Some(MINIFIGURE_MONSTER_DATA),
            monster_scale: Some(100),
            ..Default::default()
        }),
        generic_stats: vec![0],
        dimension_id: Some(1),
        ..Default::default()
    };
    iir::record("SimpleCharFullUpdateIIR_t", id, false, |w| {
        let w = w.proto(&dynel).u8(4).u8(0);
        let w = (0..5).fold(w, |w, _| w.u32(0)).string(name);
        let w = (0..4).fold(w, |w, _| w.u32(0));
        let w = w.u32(0)
            .u32(0)
            .u32(0)
            // CoreCharacterFullUpdateIIR
            .u8(b'B')
            .u32(0)
            .proto(&core)
            .u32(0)
            .blob(&[])
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(EMPTY);
        // two (u32, u32) maps (+0x14c, +0x154); their consumer isn't known yet
        w.u32(0).u32(0).u32(EMPTY).u32(EMPTY)
    })
}

/// Field ids in DreamWorld reflection data are the ELF/PJW hash of the field name (0x4ab4a0).
fn elf_hash(name: &str) -> u32 {
    let mut h: u32 = 0;
    for &b in name.as_bytes() {
        h = (h << 4).wrapping_add(b as u32);
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// `UpdateMessage` (dw::UpdateMessage, op 0 = create): creates the DreamWorld GameObject that
/// dynel IIRs wait for (GameCodeDynelFullUpdateIIR::PollStatus needs it, n3DynelFactory asserts
/// it). Layout (ReadSubClass 0x6ef680): u8 op, ID32, [op 0: u32 playfield key, blob].
/// The blob is a reflected dw::GameObject (0x6ef090): u32 byte length, then (u32 field id,
/// value) pairs; fields registered in 0x6f3a10: Id (ID32, +0x148), Position (Vector3Member),
/// Rotation, Scale. AddGameObject (0x6edf10) keys the object by its Id. Unlike the outer
/// stream, the reflection data is little-endian (the nested BinaryStream doesn't byte-swap).
/// A reflected object: u32 LE byte length, then (u32 LE field id, value) pairs.
fn reflect(fields: &[(u32, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, value) in fields {
        body.extend_from_slice(&id.to_le_bytes());
        body.extend_from_slice(value);
    }
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

fn create_game_object(id: Id32, playfield: u32, pos: [f32; 3]) -> bytes::Bytes {
    // Only Id: Position is a reflected struct (Vector3Member) and the dynel IIR carries the
    // position anyway. ID32 is itself a reflected struct {type @0, instance @4} (0x6f3e80),
    // so it nests as its own length + fields; u32 fields are plain little-endian values.
    let _ = pos;
    let id_value = reflect(&[(elf_hash("type"), &id.kind.to_le_bytes()), (elf_hash("instance"), &id.instance.to_le_bytes())]);
    let object = reflect(&[(elf_hash("Id"), &id_value)]);
    iir::record("UpdateMessage", id, false, |w| id.write(w.u8(0)).u32(playfield).blob(&object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elf_hash_matches_client() {
        // classic PJW/ELF hash values
        assert_eq!(elf_hash(""), 0);
        assert_eq!(elf_hash("Id"), (b'I' as u32) * 16 + b'd' as u32);
    }
}

/// Inventory 101 holds the minifigures; positions 6, 8 and 20 are the three active team slots
/// (the client's Lua `player:GetInventoryItem(101, 6/8/20)`, and CreateMinifigureItem asks for
/// exactly those positions).
const MINIFIGURE_INVENTORY: u32 = 101;
/// Minifigure item templates 663917..=663932 (listed in a client Lua table of minifigures).
const START_TEAM: [(u32, u32); 3] = [(6, 663929), (8, 663921), (20, 663932)];

fn acg_item(template: u32, level: u32) -> ProtoAcgItem {
    ProtoAcgItem {
        template1: Some(template),
        template2: Some(template),
        template3: Some(template),
        level: Some(level),
        ..Default::default()
    }
}

fn minifigure_inventory(character: u32) -> ProtoInventoryGame {
    ProtoInventoryGame {
        inventory_id: ProtoId32 { r#type: Some(MINIFIGURE_INVENTORY), instance: Some(character) },
        max_size: Some(64),
        disabled_slots_count: None,
        items: START_TEAM
            .iter()
            .map(|&(pos, template)| InventoryListEntry {
                pos,
                item: ProtoInventoryGameEntry {
                    item_id: Some(u64::from(template)),
                    item_data: Some(acg_item(template, 1)),
                    ..Default::default()
                },
            })
            .collect(),
    }
}
