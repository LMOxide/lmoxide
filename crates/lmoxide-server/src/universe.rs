//! Universe agent: the first server the client talks to. Checks the account's password and
//! redirects the client to the territory manager with a login cookie.
//!
//! The password arrives encrypted against the server's login key (see
//! `lmoxide_net::login`): clients must be patched with this server's public key (the patch
//! server does it), otherwise the answer can't be decrypted and the login fails.

use std::sync::Arc;

// `UniverseAgent` methods are numbered by declaration order rather than hashed.
use lmoxide_net::{
    Endpoint, Reader, Writer,
    login::LoginStatus,
    methods::{universe_agent as agent, universe_interface as interface},
    types::Id32,
};
use tracing::{error, info, warn};

use crate::{Config, conn::Conn, security, state};

/// Account type in the client's `ID32` scheme (from lmo-redux's captured values).
const PLAYER_ID_TYPE: u32 = 0x2712;

/// How long a login cookie stays valid (reconnects after a crash reuse it).
const SESSION_TTL: i64 = 24 * 3600;

pub async fn handle(mut conn: Conn, cfg: Arc<Config>) -> anyhow::Result<()> {
    conn.local = Endpoint::UniverseAgent;
    let mut username = String::new();
    let mut challenge = String::new();
    while let Some(rpc) = conn.recv().await? {
        let mut r = Reader::new(&rpc.args);
        match (rpc.target(), rpc.method()) {
            (Some(Endpoint::UniverseAgent), agent::INITIATE_AUTHENTICATION) => {
                username = r.string()?;
                let game_id = r.u32()?;
                info!(%username, game_id, "InitiateAuthentication");
                challenge = (0..16).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
                let args = Writer::new().string(&challenge).finish();
                conn.send(Endpoint::UniverseInterface, interface::CHALLENGE, args).await?;
            }
            (Some(Endpoint::UniverseAgent), agent::ANSWER_CHALLENGE) => {
                let answer = r.string()?;
                if challenge.is_empty() {
                    anyhow::bail!("AnswerChallenge before InitiateAuthentication");
                }
                let result = authenticate(&cfg, &conn, &username, &challenge, &answer);
                challenge.clear();
                match result {
                    Ok((account, gm_level)) => {
                        let player = Id32::new(PLAYER_ID_TYPE, account);
                        let cookie = state::db().create_game_session(account, SESSION_TTL)?;
                        crate::sessions::set_name(account, &username);
                        info!(%username, account, gm_level, "login accepted");
                        // The client handler (0xb4b580) treats nAuthStatus 1 as authenticated,
                        // reports any non-zero eReason as a login problem, and asserts that our
                        // caller instance equals the player instance and the cookie isn't 0.
                        let args = player
                            .write(Writer::new().i32(1))
                            .string(&cfg.public_addr(cfg.territory_port))
                            .u32(cookie)
                            .u32(LoginStatus::Ok as u32)
                            .finish();
                        conn.send_as(Some(account), Endpoint::UniverseInterface, interface::ACK_AUTHENTICATE, args)
                            .await?;
                        conn.authenticated = true;
                    }
                    Err(status) => {
                        warn!(%username, ?status, "login refused");
                        let args = Id32::new(0, 0).write(Writer::new().i32(0)).string("").u32(0).u32(status as u32).finish();
                        conn.send(Endpoint::UniverseInterface, interface::ACK_AUTHENTICATE, args).await?;
                    }
                }
            }
            (Some(Endpoint::UniverseAgent), agent::MERGE_SUBSCRIPTION) => {
                warn!("MergeSubscription not supported");
                let args = Writer::new().i32(0).finish();
                conn.send(Endpoint::UniverseInterface, interface::ACK_SUBSCRIPTION_MERGE, args).await?;
            }
            (Some(Endpoint::UniverseAgent), agent::CLIENT_DISCONNECTED) => {}
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    Ok(())
}

/// Checks a login; the account id and GM level, or why it was refused.
fn authenticate(cfg: &Config, conn: &Conn, username: &str, challenge: &str, answer: &str) -> Result<(u32, u32), LoginStatus> {
    let ip_key = format!("ip:{}", conn.peer.ip());
    let user_key = format!("user:{}", username.to_ascii_lowercase());
    if !security::allowed(&ip_key) || !security::allowed(&user_key) {
        warn!(%username, "too many failed logins, try again later");
        return Err(LoginStatus::AuthFailure);
    }
    let fail = |why: &str| {
        warn!(%username, "login failed: {why}");
        security::failed(&ip_key);
        security::failed(&user_key);
        LoginStatus::AuthFailure
    };
    let decrypted = match state::login_key().decrypt_answer(answer) {
        Ok(a) => a,
        Err(e) => {
            return Err(fail(&format!(
                "{e}; is the client patched with this server's login key? (lmoxide-server login-key)"
            )));
        }
    };
    if decrypted.challenge != challenge || !decrypted.username.eq_ignore_ascii_case(username) {
        return Err(fail("answer doesn't match the challenge"));
    }
    let password = decrypted.password;
    let db = state::db();
    let internal = |e: lmoxide_db::Error| {
        error!("database: login: {e}");
        LoginStatus::SystemError
    };
    // a server sharing another's logins takes that one's password (and new accounts) first
    if let Some(shared) = state::shared_accounts()
        && let Err(e) = crate::admin::share_accounts(shared, db, Some(username))
    {
        warn!(%username, "shared account: {e:#}");
    }
    let account = match db.account_by_name(username).map_err(internal)? {
        Some(a) if a.has_password => {
            if !db.verify_password(a.id, &password).map_err(internal)? {
                return Err(fail("wrong password"));
            }
            a
        }
        // accounts created without a password: the first login sets it
        Some(a) if cfg.auto_register => {
            db.set_password(a.id, Some(&password)).map_err(|e| fail(&e.to_string()))?;
            db.audit_account("login", "set-first-password", &a, None).map_err(internal)?;
            a
        }
        Some(_) => return Err(fail("the account has no password yet (lmoxide-server account passwd)")),
        None if cfg.auto_register => {
            let a = db.create_account(username, Some(&password), 0).map_err(|e| fail(&e.to_string()))?;
            db.audit_account("login", "auto-register", &a, None).map_err(internal)?;
            info!(%username, account = a.id, "account registered on first login");
            a
        }
        None => return Err(fail("no such account")),
    };
    security::succeeded(&user_key);
    security::succeeded(&ip_key);
    if let Some(ban) = account.active_ban() {
        warn!(%username, ?ban, "banned account");
        return Err(LoginStatus::Banned);
    }
    let gm_level = account.gm_level.max(cfg.default_gm_level);
    if state::maintenance().is_some() && gm_level == 0 {
        return Err(LoginStatus::MaintenanceMode);
    }
    db.record_login(account.id, &conn.peer.ip().to_string()).map_err(internal)?;
    Ok((account.id, gm_level))
}
