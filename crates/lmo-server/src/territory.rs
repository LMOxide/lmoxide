//! Territory manager: hosts the `PlayerAgent` endpoint (character list, creation, login to a
//! game server).

use std::sync::Arc;

use lmo_net::{
    Endpoint, Reader, Writer,
    methods::{player_agent as agent, player_interface as interface},
    proto::FidData,
};
use tracing::{info, warn};

use crate::{Config, conn::Conn};

pub async fn handle(mut conn: Conn, cfg: Arc<Config>) -> anyhow::Result<()> {
    conn.local = Endpoint::PlayerAgent;
    let status: i32 = std::env::var("LMO_TERRITORY_AUTH_STATUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut account = 0;
    let mut keepalive = tokio::time::interval(KEEPALIVE);
    keepalive.tick().await;
    loop {
        let rpc = tokio::select! {
            rpc = conn.recv() => match rpc? {
                Some(rpc) => rpc,
                None => break,
            },
            _ = keepalive.tick(), if account != 0 => {
                conn.send_as(Some(account), Endpoint::PlayerInterface, interface::VERIFY_CLIENT_STILL_CONNECTED, bytes::Bytes::new())
                    .await?;
                continue;
            }
        };
        let mut r = Reader::new(&rpc.args);
        match (rpc.target(), rpc.method()) {
            (Some(Endpoint::PlayerAgent), agent::AUTHENTICATE) => {
                account = r.u32()?;
                let cookie = r.u32()?;
                let version = r.u32()?;
                let valid = cookie == crate::universe::cookie_for(account);
                info!(account, cookie, version, valid, "Authenticate");
                let args = Writer::new().i32(status).finish();
                conn.send_as(Some(account), Endpoint::PlayerInterface, interface::AUTHENTICATE_ACK, args)
                    .await?;
            }
            (Some(Endpoint::PlayerAgent), agent::GET_STARTUP_DATA) => {
                let md5 = r.string()?;
                info!(account, %md5, "GetStartupData");
                let me = Some(account);
                conn.send_as(me, Endpoint::PlayerInterface, interface::UPDATE_CLIENT_PLAYER_DATA, client_player_data(account))
                    .await?;
                conn.send_as(me, Endpoint::PlayerInterface, interface::SET_DIMENSION_LIST, dimension_list())
                    .await?;
                let setup = Writer::new().i32(status).finish();
                conn.send_as(me, Endpoint::PlayerInterface, interface::PLAYER_SETUP_COMPLETE, setup)
                    .await?;
            }
            (Some(Endpoint::PlayerAgent), agent::CLIENT_REPLY_KEEP_COOKIE) => {}
            (Some(Endpoint::PlayerAgent), agent::GET_DIMENSION_LIST) => {
                conn.send_as(Some(account), Endpoint::PlayerInterface, interface::SET_DIMENSION_LIST, dimension_list())
                    .await?;
            }
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    let _ = cfg;
    Ok(())
}

/// The client drops an idle territory connection after 30 s; `VerifyClientStillConnected` is
/// answered with `ClientReplyKeepCookie`.
const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(10);

/// `UpdateClientPlayerData(ClientPlayerData_t, FidData_c, territoryID)`, decoded by the client at
/// 0x7c0000: ClientPlayerData_t = u32, map<u32, character> (entry reader 0xb49e80), map<u32, u32>;
/// FidData_c = embedded `FidData` protobuf; then a u32 territory ID.
fn client_player_data(account: u32) -> bytes::Bytes {
    let fid = FidData {
        player_instance: account,
        expansion_flags: 0,
        subscription_flags: 0,
        veteran_months: 0,
        offline_levels: 0,
        character_slots: 4,
        facebook_auth_url: None,
        unique_login_days: None,
        login_method: None,
    };
    Writer::new()
        .u32(account) // meaning unknown; first ClientPlayerData_t field
        .container_len(0) // characters
        .container_len(0) // u32 -> u32 map
        .proto(&fid)
        .u32(TERRITORY_ID)
        .finish()
}

const TERRITORY_ID: u32 = 1;

/// `SetDimensionList(DimensionDataVec_t)`: each entry is 8 u32s, bool, u32, bool (reader
/// 0x12ea940). The UI exposes m_Id, m_Name, m_Language, m_Load, m_Rules; the field order is
/// not yet mapped, so only the first (assumed id) is set.
fn dimension_list() -> bytes::Bytes {
    let w = Writer::new().container_len(1).u32(1);
    let w = (0..7).fold(w, |w, _| w.u32(0));
    w.bool(true).u32(0).bool(true).finish()
}
