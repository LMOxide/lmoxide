//! Territory manager: hosts the `PlayerAgent` endpoint (character list, creation, login to a
//! game server).

use std::sync::Arc;

use lmoxide_net::{
    Endpoint, Reader, Writer,
    methods::{player_agent as agent, player_interface as interface},
    proto::{FidData, PlayfieldProxyData, ProtoId32},
    types::Id32,
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
    let mut cookie = 0;
    // the game mode the login is for (its universe port): only its characters are listed
    let mut mode = cfg.default_mode();
    // a reconnect (LoginFromCrash): the client waits on its loading screen to be sent on to a
    // game server with its character, without picking it again
    let mut crashed = false;
    let mut _session = None;
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
            (Some(Endpoint::PlayerAgent), method @ (agent::AUTHENTICATE | agent::LOGIN_FROM_CRASH)) => {
                // Authenticate(account, cookie, territoryVersion). A client that lost its
                // servers (e.g. a server restart) reconnects with LoginFromCrash(account, cookie,
                // character) instead; AuthenticateAck(1) puts it back on the normal path
                // (GameClientConnectivity::AuthenticateAck -> GetStartupData).
                let claimed = r.u32()?;
                let their_cookie = r.u32()?;
                let extra = r.u32()?;
                let ip_key = format!("ip:{}", conn.peer.ip());
                let session = crate::state::or_log(crate::state::db().game_session_mode(their_cookie), "session", None);
                let valid = crate::security::allowed(&ip_key) && session.as_ref().is_some_and(|(a, _)| *a == claimed);
                let crash = method == agent::LOGIN_FROM_CRASH;
                info!(account = claimed, valid, crash, extra, "Authenticate");
                if !valid {
                    warn!(account = claimed, "invalid login cookie");
                    crate::security::failed(&ip_key);
                    let args = Writer::new().i32(0).finish();
                    conn.send_as(Some(claimed), Endpoint::PlayerInterface, interface::AUTHENTICATE_ACK, args).await?;
                    break;
                }
                account = claimed;
                cookie = their_cookie;
                mode = session.and_then(|(_, m)| crate::instance::Mode::parse(&m)).unwrap_or(mode);
                crashed = crash;
                conn.authenticated = true;
                _session = Some(crate::sessions::territory_opened(account));
                let args = Writer::new().i32(status).finish();
                conn.send_as(Some(account), Endpoint::PlayerInterface, interface::AUTHENTICATE_ACK, args)
                    .await?;
            }
            _ if !conn.authenticated => anyhow::bail!("{} before Authenticate", rpc.describe()),
            (Some(Endpoint::PlayerAgent), agent::GET_STARTUP_DATA) => {
                let md5 = r.string()?;
                info!(account, %md5, "GetStartupData");
                let me = Some(account);
                conn.send_as(me, Endpoint::PlayerInterface, interface::UPDATE_CLIENT_PLAYER_DATA, client_player_data(account, mode))
                    .await?;
                conn.send_as(me, Endpoint::PlayerInterface, interface::SET_DIMENSION_LIST, dimension_list())
                    .await?;
                let setup = Writer::new().i32(status).finish();
                conn.send_as(me, Endpoint::PlayerInterface, interface::PLAYER_SETUP_COMPLETE, setup)
                    .await?;
                if std::mem::take(&mut crashed) {
                    // back with the character played last
                    let last = listed(account, mode).into_iter().max_by_key(|c| c.saved_at).map_or(account, |c| c.id);
                    connect_character(&mut conn, &cfg, account, last, cookie).await?;
                }
            }
            (Some(Endpoint::PlayerAgent), agent::CLIENT_REPLY_KEEP_COOKIE) => {}
            (Some(Endpoint::PlayerAgent), agent::CREATE_CHARACTER_WITH_DATA) => {
                let dim = r.u32()?;
                let n = r.container_len()?;
                let mut data = Vec::with_capacity(n);
                for _ in 0..n {
                    data.push((r.string()?, r.string()?));
                }
                let (region, age, gender) = (r.u16()?, r.u16()?, r.u16()?);
                let igr = r.string()?;
                let (user_type, service_type) = (r.u32()?, r.u32()?);
                info!(account, dim, ?data, region, age, gender, %igr, user_type, service_type, "CreateCharacterWithData");
                // A new character (the account's first has the account's id). It starts in the
                // creation playfield, where the client sends the name and minifigure picked on the
                // creation screen (tutorial.rs). One left unfinished there is used again.
                let pending = owned(account, mode).into_iter().find(|c| c.playfield == crate::tutorial::CREATION_PLAYFIELD).map(|c| c.id);
                let character = match pending {
                    Some(id) => id,
                    None => {
                        let id = new_id(account);
                        create_character(account, id, mode);
                        id
                    }
                };
                connect_character(&mut conn, &cfg, account, character, cookie).await?;
            }
            (Some(Endpoint::PlayerAgent), agent::LOGIN_CHARACTER) => {
                // LoginCharacter(instance, dataMap, language, region, age, gender, igr, userType,
                // serviceType): the saved character picked from the list
                let instance = r.u32()?;
                info!(account, instance, "LoginCharacter");
                // only the account's own characters
                let character = if listed(account, mode).iter().any(|c| c.id == instance) {
                    instance
                } else {
                    warn!(account, instance, "LoginCharacter: not this account's character");
                    listed(account, mode).first().map_or(account, |c| c.id)
                };
                connect_character(&mut conn, &cfg, account, character, cookie).await?;
            }
            (Some(Endpoint::PlayerAgent), agent::REQUEST_DELETE_CHARACTER) => {
                // RequestDeleteCharacter(instance): the character screen's delete. Only the
                // account's own characters; each is backed up before it goes. The client has no
                // "deleted" reply: it gets the character list again.
                let instance = r.u32()?;
                let owned = owned(account, mode).iter().any(|c| c.id == instance);
                info!(account, instance, owned, "RequestDeleteCharacter");
                if owned {
                    if crate::sessions::is_online(instance) {
                        warn!(account, instance, "not deleting a character in play");
                    } else {
                        delete_character(account, instance);
                    }
                }
                conn.send_as(Some(account), Endpoint::PlayerInterface, interface::UPDATE_CLIENT_PLAYER_DATA, client_player_data(account, mode))
                    .await?;
            }
            (Some(Endpoint::PlayerAgent), agent::GET_DIMENSION_LIST) => {
                conn.send_as(Some(account), Endpoint::PlayerInterface, interface::SET_DIMENSION_LIST, dimension_list())
                    .await?;
            }
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    Ok(())
}

/// The account's characters.
/// The account's characters in game mode `mode`.
fn owned(account: u32, mode: crate::instance::Mode) -> Vec<lmoxide_db::CharacterSummary> {
    crate::state::or_log(crate::state::db().characters_in(account, mode.id()), "characters", vec![])
}

/// The id of a new character of `account`: the account's own for its first (in any mode).
fn new_id(account: u32) -> u32 {
    if crate::state::or_log(crate::state::db().characters_of(account), "characters", vec![]).is_empty() {
        account
    } else {
        crate::state::or_log(crate::state::db().new_character_id(), "character id", account)
    }
}

/// The account's characters the client lists: not those still being created (the client keeps
/// the picked name only until it quits, so it offers creation again for them).
fn listed(account: u32, mode: crate::instance::Mode) -> Vec<lmoxide_db::CharacterSummary> {
    owned(account, mode).into_iter().filter(|c| c.playfield != crate::tutorial::CREATION_PLAYFIELD).collect()
}

/// Deletes `character` of `account`, first writing everything it had to
/// `<data dir>/deleted-characters/<character>-<unix time>.json` (the database row's contents).
fn delete_character(account: u32, character: u32) {
    let Some(c) = crate::store::Store.load(character) else { return };
    let dir = crate::state::data_dir().join("deleted-characters");
    let now = lmoxide_db::now();
    let backup = serde_json::json!({ "account": account, "character": character, "deleted_at": now, "data": c });
    let path = dir.join(format!("{character}-{now}.json"));
    let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, serde_json::to_vec_pretty(&backup).unwrap_or_default()));
    if let Err(e) = written {
        // without a backup it stays
        warn!(account, character, path = %path.display(), "backing up the deleted character failed, not deleting: {e}");
        return;
    }
    match crate::state::db().delete_character(character) {
        Ok(()) => info!(account, character, name = %c.name, backup = %path.display(), "character deleted"),
        Err(e) => warn!(account, character, "deleting the character failed: {e:#}"),
    }
}

/// Saves a new character `id` for `account` on the creation screen, without a name until the
/// player picks one (every game mode: the name comes from the creation screen).
fn create_character(account: u32, id: u32, mode: crate::instance::Mode) {
    let mut c = crate::game::new_character(String::new(), mode, crate::tutorial::CREATION_PLAYFIELD);
    // as live: no minifigure until the player's pick arrives (its starter is added with the
    // pick)
    c.team.clear();
    if mode.rules().progression {
        // the character owns only the members' minifigures until then
        let now = crate::smash::now_ms() / 1000;
        c.collection = Some(crate::loot::membership_minifigures().iter().map(|&t| (t, now)).collect());
    }
    match crate::store::Store.create(id, account, &c) {
        // (friends with the account's friends' characters from here, Db::save_character; their
        // lists show it once it has its name, community::renamed)
        Ok(()) => info!(account, character = id, name = %c.name, playfield = c.playfield, "character created"),
        Err(e) => warn!(account, "creating the character failed: {e:#}"),
    }
}

/// Sends the client on to the game server with the account's `character`.
async fn connect_character(conn: &mut Conn, cfg: &Config, account: u32, character: u32, cookie: u32) -> anyhow::Result<()> {
    crate::sessions::set_character(account, character);
    let character = Id32::new(CHARACTER_TYPE, character);
    // ChatServerConnectReady(const String& address, uint cookie, const ID32& characterID):
    // the client only stores the address ("host:port") and connects after the
    // character's full update; it must come before LoginConnectReady.
    let chat = character
        .write(
            Writer::new()
                .string(&cfg.public_addr(cfg.chat_port))
                .u32(cookie),
        )
        .finish();
    conn.send_as(Some(account), Endpoint::PlayerInterface, interface::CHAT_SERVER_CONNECT_READY, chat)
        .await?;
    conn.send_as(Some(account), Endpoint::PlayerInterface, interface::LOGIN_CONNECT_READY, login_connect_ready(cfg, character, cookie))
        .await?;
    Ok(())
}

/// The client drops an idle territory connection after 30 s; `VerifyClientStillConnected` is
/// answered with `ClientReplyKeepCookie`.
const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(10);

/// `UpdateClientPlayerData(ClientPlayerData_t, FidData_c, territoryID)`, decoded by the client at
/// 0x7c0000: ClientPlayerData_t = u32, map<u32, character> (entry reader 0xb49e80), map<u32, u32>;
/// FidData_c = embedded `FidData` protobuf; then a u32 territory ID. The account's saved
/// character goes in the map, else the client offers character creation at every login.
fn client_player_data(account: u32, mode: crate::instance::Mode) -> bytes::Bytes {
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
        .raw(&characters(account, mode))
        .container_len(0) // u32 -> u32 map
        .proto(&fid)
        .u32(TERRITORY_ID)
        .finish()
}

const TERRITORY_ID: u32 = 1;

/// The character map: character instance -> PlayerCharacterData_t (operator>> 0x193b269 in the
/// Linux build): u32 account, u32 character, 3 name strings (first, middle, last), u32
/// dimension, u32, string creation date, u32 playtime (s), u32 playfield, u32 level, u32, u32,
/// u64 last login (Unix time), u32, u32, string language, u32, u32, string, u32 level, u32.
/// Values as in the recorded session (its 2, 11 and 41757 unknown fields are left 0).
fn characters(account: u32, mode: crate::instance::Mode) -> Vec<u8> {
    let list: Vec<(u32, crate::store::Character)> =
        listed(account, mode).into_iter().filter_map(|s| Some((s.id, crate::store::Store.load(s.id)?))).collect();
    let mut w = Writer::new().container_len(list.len());
    for (id, c) in &list {
        let level = c.level.max(1);
        w = w.u32(*id).u32(account).u32(*id);
        // first, middle and last name (creation picks three)
        let mut names = c.name.splitn(3, ' ');
        w = (0..3).fold(w, |w, _| w.string(names.next().unwrap_or("")));
        w = w.u32(1).u32(0).string("").u32(0).u32(c.playfield).u32(level).u32(0).u32(0).u64(0);
        w = w.u32(0).u32(0).string("en").u32(0).u32(0).string("").u32(level).u32(0);
    }
    w.finish().to_vec()
}

/// `SetDimensionList(DimensionDataVec_t)`: each entry is 8 u32s, bool, u32, bool (reader
/// 0x12ea940). The UI exposes m_Id, m_Name, m_Language, m_Load, m_Rules; the field order is
/// not yet mapped, so only the first (assumed id) is set.
fn dimension_list() -> bytes::Bytes {
    let w = Writer::new().container_len(1).u32(1);
    let w = (0..7).fold(w, |w, _| w.u32(0));
    w.bool(true).u32(0).bool(true).finish()
}

/// `ID32` type of characters (the client's character-list code uses 50000).
pub const CHARACTER_TYPE: u32 = 50000;

/// `LoginConnectReady(gameServerAddress, nCookie, cClientCharacterID, PlayfieldProxy)`
/// (decoder 0x7bf400): string, u32, ID32, embedded `PlayfieldProxyData` protobuf.
/// The playfield is the character's saved one (new characters: `--start-playfield`).
fn login_connect_ready(cfg: &Config, character: Id32, cookie: u32) -> bytes::Bytes {
    let (mode, playfield) = crate::store::Store
        .load(character.instance)
        .map_or((cfg.default_mode(), cfg.start_playfield), |c| (crate::instance::Mode::of_character(&c), c.playfield));
    let pf = PlayfieldProxyData {
        dimension_id: Some(1),
        model_id: Some(ProtoId32 { r#type: Some(crate::game::PLAYFIELD_TYPE), instance: Some(playfield) }),
        running_instance: Some(crate::instance::login_arrival(mode, playfield, character.instance).key()),
        ..Default::default()
    };
    character
        .write(
            Writer::new()
                .string(&cfg.public_addr(cfg.game_port))
                .u32(cookie),
        )
        .proto(&pf)
        .finish()
}
