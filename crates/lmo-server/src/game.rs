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

use crate::{
    Config,
    conn::Conn,
    store::{Character, Store},
    world,
};

pub async fn handle(mut conn: Conn, cfg: Arc<Config>) -> anyhow::Result<()> {
    conn.local = Endpoint::ClientToGameserverRPC;
    let status: i32 = std::env::var("LMO_GAME_AUTH_STATUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let store = Store::new(&cfg.data_dir);
    let mut character = 0;
    let mut me = Character {
        name: String::new(),
        playfield: cfg.start_playfield,
        pos: START_POS,
        team: START_TEAM.to_vec(),
    };
    let pos = Arc::new(Mutex::new(START_POS));
    let mut last_save = std::time::Instant::now();
    let (outbox, mut inbox) = mpsc::unbounded_channel::<world::Out>();
    let mut presence: Option<world::Presence> = None;
    loop {
        let rpc = tokio::select! {
            rpc = conn.recv() => match rpc? {
                Some(rpc) => rpc,
                None => break,
            },
            Some(out) = inbox.recv() => {
                match out {
                    world::Out::Records(records) => {
                        let blob = iir::pass_blob(&records);
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob).await?;
                    }
                    world::Out::Rpc { target, method, args } => conn.send_as(Some(character), target, method, args).await?,
                }
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
                me = store.load(character).unwrap_or_else(|| Character {
                    name: crate::sessions::name(character).unwrap_or_else(|| "Player".into()),
                    ..me.clone()
                });
                *pos.lock().unwrap() = me.pos;
                info!(character, cookie, %version, valid, name = %me.name, playfield = me.playfield, pos = ?me.pos, "Authenticate2");
                let args = Writer::new().i32(status).finish();
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::ACK_AUTHENTICATE, args)
                    .await?;
                // Characters are keyed by account instance for now (see territory.rs).
                if !crate::sessions::territory_closed(character, std::time::Duration::from_secs(10)).await {
                    warn!(character, "territory connection still open; sending the world anyway");
                }
                // Give the client a moment to process the territory disconnect.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                let mut records = vec![playfield_full_update(me.playfield)];
                records.extend(spawn_records(id, &me));
                records.insert(3, full_player_character(id, &me));
                if cfg.gm_level != 0 {
                    records.push(int32_stat(id, STAT_GM_LEVEL, cfg.gm_level));
                }
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
                            let (id, snapshot, pos_handle) = (h.identity, me.clone(), pos.clone());
                            let pos = pos.clone();
                            let spawn = move || remote_spawn_records(id, &Character { pos: *pos.lock().unwrap(), ..snapshot.clone() });
                            presence = Some(world::join(id, me.playfield, pos_handle, spawn, outbox.clone()));
                        }
                    }
                    Some(kind @ ("CharDCMoveIIR_t" | "MoveTaskIIR_c")) => {
                        if let Some(p) = reported_position(kind, rest) {
                            *pos.lock().unwrap() = p;
                            if last_save.elapsed() > SAVE_INTERVAL {
                                last_save = std::time::Instant::now();
                                me.pos = p;
                                if let Err(e) = store.save(character, &me) {
                                    warn!(character, "saving character failed: {e:#}");
                                }
                            }
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
            (Some(Endpoint::GameCharAgent), game_char_agent::VICINITY_CHAT_MESSAGE) => {
                // VicinityChatMessage(const String& message, const int mood) -> everyone on the
                // playfield, sender included: GameCharInterface::VicinityChatMessage(fromName,
                // message, mood, gmLDBNametagInstance).
                let message = r.string()?;
                let mood = r.i32()?;
                info!(name = %me.name, %message, mood, "chat");
                if let Some(p) = &presence {
                    let args = Writer::new().string(&me.name).string(&message).i32(mood).i32(0).finish();
                    let out = world::Out::Rpc {
                        target: Endpoint::GameCharInterface,
                        method: game_char_interface::VICINITY_CHAT_MESSAGE,
                        args,
                    };
                    world::send_playfield(p.id(), true, out);
                }
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
    if character != 0 {
        me.pos = *pos.lock().unwrap();
        match store.save(character, &me) {
            Ok(()) => info!(character, pos = ?me.pos, "character saved"),
            Err(e) => warn!(character, "saving character failed: {e:#}"),
        }
    }
    Ok(())
}

/// Characters are saved on disconnect and, while moving, at most this often.
const SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Where a movement IIR says the character is (CharDCMoveIIR_t) or is heading
/// (MoveTaskIIR_c: the path's last waypoint). See `move_record` / `move_task_record`.
fn reported_position(kind: &str, rest: &[u8]) -> Option<[f32; 3]> {
    let f = |i: usize| rest.get(i..i + 4).map(|b| f32::from_be_bytes(b.try_into().unwrap()));
    let at = |i: usize| Some([f(i)?, f(i + 4)?, f(i + 8)?]);
    match kind {
        "CharDCMoveIIR_t" => at(1),
        _ => {
            let n = rest.get(28..32).map(|b| u32::from_be_bytes(b.try_into().unwrap()))? as usize;
            if n == 0 || n > 64 {
                return None;
            }
            at(32 + 12 * (n - 1))
        }
    }
}

/// The IIRs that make a character exist on a client: its DreamWorld GameObject, the dynel,
/// and its stats. The player's own client additionally needs `full_player_character` (and
/// sends its own CharInPlayIIR_t); others get `remote_spawn_records`.
pub fn spawn_records(id: Id32, c: &Character) -> Vec<Bytes> {
    vec![
        create_game_object(id, c.playfield, c.pos),
        simple_char_full_update(id, c),
        int32_stat(id, STAT_LIFE, START_HEALTH),
        int32_stat(id, STAT_HEALTH, START_HEALTH),
        int32_stat(id, STAT_LEVEL, 1),
    ]
}

/// What other clients need to see a character: its spawn records, already in play.
pub fn remote_spawn_records(id: Id32, c: &Character) -> Vec<Bytes> {
    let mut records = spawn_records(id, c);
    records.push(iir::record("CharInPlayIIR_t", id, false, |w| w));
    records
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
    let bot = Character { name: "Bot".into(), playfield, pos: START_POS, team: START_TEAM.to_vec() };
    let spawn = move || remote_spawn_records(id, &Character { pos: *spawn_pos.lock().unwrap(), ..bot.clone() });
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
fn full_player_character(id: Id32, c: &Character) -> bytes::Bytes {
    let proto = ProtoFullCharacterIir {
        inventory: vec![minifigure_inventory(id.instance, &c.team)],
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
/// A character's body and costume come from its MonsterData stat (RDB type 1010514). Each
/// minifigure item template has a SetStatGameAction setting MonsterData to its own monsterdata
/// (template 1000620 body: `d2 02 00 00 00 | 67 01 00 00 | f2 02 ...` then the u32 value);
/// the 2212 in the templates' stat block is the generic body ("Aztec_Warrior").
/// `CoreCharacterFullUpdateIIR::Transfer` applies `stats.monster_data` before building the
/// animated mesh; without it remote characters are invisible.
const MINIFIGURE_MONSTER_DATA: [(u32, u32); 16] = [
    (663917, 663907), // Queen
    (663918, 663902), // Animal Control
    (663919, 663906), // Jewel Thief
    (663920, 663876), // Tribal Woman
    (663921, 663855), // Ballerina
    (663922, 663904),
    (663923, 663903), // Faun
    (663924, 663905),
    (663925, 663909),
    (663926, 663916),
    (663927, 663912),
    (663928, 663915), // Kendo Fighter
    (663929, 663910), // Astronaut
    (663930, 663911), // Laser Mech
    (663931, 663914),
    (663932, 663913), // Farmer
];

/// Monsterdata for a minifigure item template (the generic body if unknown).
fn monster_data_for(template: u32) -> u32 {
    MINIFIGURE_MONSTER_DATA.iter().find(|(t, _)| *t == template).map_or(2212, |(_, m)| *m)
}

/// Engine stat ids (data/stats.json, from the client's stat table at 0x1607bd0).
const STAT_LIFE: u32 = 1;
const STAT_HEALTH: u32 = 27;
const STAT_LEVEL: u32 = 54;
const STAT_GM_LEVEL: u32 = 215;

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

fn simple_char_full_update(id: Id32, c: &Character) -> bytes::Bytes {
    let (playfield, name, pos) = (c.playfield, c.name.as_str(), c.pos);
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
            // the active (first) team slot's minifigure
            monster_data: Some(c.team.first().map_or(2212, |&(_, t)| monster_data_for(t))),
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

fn minifigure_inventory(character: u32, team: &[(u32, u32)]) -> ProtoInventoryGame {
    ProtoInventoryGame {
        inventory_id: ProtoId32 { r#type: Some(MINIFIGURE_INVENTORY), instance: Some(character) },
        max_size: Some(64),
        disabled_slots_count: None,
        items: team
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
