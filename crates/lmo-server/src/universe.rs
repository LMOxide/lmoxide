//! Universe agent: the first server the client talks to. Authenticates the account and
//! redirects the client to the territory manager.

use std::sync::Arc;

// `UniverseAgent` methods are numbered by declaration order rather than hashed.
use lmo_net::{
    Endpoint, Reader, Writer,
    methods::{universe_agent as agent, universe_interface as interface},
    types::Id32,
};
use tracing::{info, warn};

use crate::{Config, conn::Conn};

/// Account type in the client's `ID32` scheme (from lmo-redux's captured values).
const PLAYER_ID_TYPE: u32 = 0x2712;

pub async fn handle(mut conn: Conn, cfg: Arc<Config>) -> anyhow::Result<()> {
    conn.local = Endpoint::UniverseAgent;
    let mut username = String::new();
    while let Some(rpc) = conn.recv().await? {
        let mut r = Reader::new(&rpc.args);
        match (rpc.target(), rpc.method()) {
            (Some(Endpoint::UniverseAgent), agent::INITIATE_AUTHENTICATION) => {
                username = r.string()?;
                let game_id = r.u32()?;
                info!(%username, game_id, "InitiateAuthentication");
                // The real server sent a Diffie-Hellman challenge. We don't verify the answer,
                // so any challenge string the client accepts will do.
                let challenge = Writer::new().string("00000000000000000000000000000000").finish();
                conn.send(Endpoint::UniverseInterface, interface::CHALLENGE, challenge).await?;
            }
            (Some(Endpoint::UniverseAgent), agent::ANSWER_CHALLENGE) => {
                let answer = r.string()?;
                info!(%username, answer_len = answer.len(), "AnswerChallenge");
                let player = Id32::new(PLAYER_ID_TYPE, account_instance(&username));
                // The client handler (0xb4b580) treats nAuthStatus 1 as authenticated, reports
                // any non-zero eReason as a login problem, and asserts that our caller
                // instance equals the player instance.
                let args = player
                    .write(Writer::new().i32(1))
                    .string(&cfg.public_addr(cfg.territory_port))
                    .u32(cookie_for(player.instance))
                    .u32(0)
                    .finish();
                conn.send_as(
                    Some(player.instance),
                    Endpoint::UniverseInterface,
                    interface::ACK_AUTHENTICATE,
                    args,
                )
                .await?;
            }
            (Some(Endpoint::UniverseAgent), agent::MERGE_SUBSCRIPTION) => {
                warn!("MergeSubscription not supported");
                let args = Writer::new().i32(0).finish();
                conn.send(Endpoint::UniverseInterface, interface::ACK_SUBSCRIPTION_MERGE, args)
                    .await?;
            }
            (Some(Endpoint::UniverseAgent), agent::CLIENT_DISCONNECTED) => {}
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    Ok(())
}

/// Stable per-username account instance until we have an account database.
pub fn account_instance(username: &str) -> u32 {
    lmo_net::rpc::endpoint_hash(username) & 0x7fff_ffff
}

/// Login cookie handed to the territory manager. Not secret yet: derived from the account.
pub fn cookie_for(instance: u32) -> u32 {
    instance ^ 0x6c47_0581
}
