//! Game server: the playfield connection the client opens after `LoginConnectReady`
//! (`ClientToGameserverRPC` / `GameserverToClientRPC`, then the gamecode endpoints).

use std::sync::Arc;

use lmo_net::{
    Endpoint, Reader, Writer, iir,
    methods::{client_to_gameserver_rpc as agent, gameserver_to_client_rpc as client},
    proto::{PlayfieldProxyData, ProtoId32},
    types::Id32,
};
use tracing::{info, warn};

use crate::{Config, conn::Conn};

pub async fn handle(mut conn: Conn, cfg: Arc<Config>) -> anyhow::Result<()> {
    conn.local = Endpoint::ClientToGameserverRPC;
    let status: i32 = std::env::var("LMO_GAME_AUTH_STATUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut character = 0;
    while let Some(rpc) = conn.recv().await? {
        let mut r = Reader::new(&rpc.args);
        match (rpc.target(), rpc.method()) {
            (Some(Endpoint::ClientToGameserverRPC), agent::AUTHENTICATE2) => {
                character = r.u32()?;
                let cookie = r.u32()?;
                let version = r.string()?;
                let valid = cookie == crate::universe::cookie_for(character);
                info!(character, cookie, %version, valid, "Authenticate2");
                let args = Writer::new().i32(status).finish();
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::ACK_AUTHENTICATE, args)
                    .await?;
                let blob = iir::pass_blob(&[playfield_full_update(cfg.start_playfield)]);
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob)
                    .await?;
            }
            (Some(Endpoint::ClientToGameserverRPC), agent::PING) => {
                let seq = r.u32()?;
                let args = Writer::new().u32(seq).finish();
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PONG, args).await?;
            }
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    Ok(())
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
        ..Default::default()
    };
    iir::record("PlayfieldFIIR_t", id, false, |w| {
        let empty_list = 0u32.to_be_bytes();
        let w = w.u32(0).u32(0).u32(0).proto(&proxy).u32(0).blob(&empty_list).blob(&empty_list);
        (0..6).fold(w, |w, _| w.u32(0))
    })
}
