//! Territory manager: hosts the `PlayerAgent` endpoint (character list, creation, login to a
//! game server).

use std::sync::Arc;

use lmo_net::{
    Endpoint, Reader, Writer,
    methods::{player_agent as agent, player_interface as interface},
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
    while let Some(rpc) = conn.recv().await? {
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
            (Some(Endpoint::PlayerAgent), agent::GET_STARTUP_DATA | agent::GET_DIMENSION_LIST) => {
                warn!(account, args = %crate::conn::hex(&rpc.args), "not implemented yet");
            }
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    let _ = cfg;
    Ok(())
}
