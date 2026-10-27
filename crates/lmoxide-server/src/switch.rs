//! Moving a player between this server and a peer next to it (the play server and the world
//! editor's survey server, `--peer-*`): same account and client, each server's own character.
//!
//! `!server <name>` sends `GameserverToClientRPC::RedirectToServer(peer game address)`, as live
//! moved players between playfield servers: the client leaves this game server and sends the
//! peer `Authenticate2` with the same character id and login cookie. The peer checks the cookie
//! in this server's database (`arrival`) and plays its own character of that id (made on the
//! first visit). Accounts are matched by username: the same login can have another id there.
//!
//! Chat can't follow: the client takes a chat server address only from the territory at login.
//! So the chat server it stays with passes the chat connection of a player who went to the
//! peer on to the peer's chat server (`away`, chat.rs).

use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex, OnceLock},
};

use lmoxide_db::Db;
use tracing::{info, warn};

pub struct Peer {
    /// What players call it (`play`, `editor`).
    pub name: String,
    /// Its game and chat servers, as clients reach them ("host:port").
    pub game_addr: String,
    pub chat_addr: String,
    db: Db,
}

static NAME: OnceLock<String> = OnceLock::new();
static PEER: OnceLock<Peer> = OnceLock::new();
/// accounts this server sent to the peer and that haven't come back
static AWAY: LazyLock<Mutex<HashSet<u32>>> = LazyLock::new(Default::default);

/// This server's name and its peer, if configured (`--peer-name` etc.).
pub fn init(name: &str, peer: Option<(String, std::path::PathBuf, String, String)>) -> anyhow::Result<()> {
    let _ = NAME.set(name.into());
    if let Some((peer_name, database, game_addr, chat_addr)) = peer {
        let db = Db::open(&database).map_err(|e| anyhow::anyhow!("opening the peer's database {}: {e}", database.display()))?;
        info!(peer = %peer_name, %game_addr, %chat_addr, "server switching: players can go to the peer with !server");
        let _ = PEER.set(Peer { name: peer_name, game_addr, chat_addr, db });
    }
    Ok(())
}

/// This server's name.
pub fn name() -> &'static str {
    NAME.get().map_or("play", String::as_str)
}

pub fn peer() -> Option<&'static Peer> {
    PEER.get()
}

/// `!server [name]`: the reply and the peer to go to; Err is the reply when nothing happens.
pub fn request(to: &str) -> Result<(String, &'static Peer), String> {
    let names = || match peer() {
        Some(p) => format!("this is the {} server; `!server {}` goes to the other one", name(), p.name),
        None => format!("this is the {} server; it has no other server to switch to (--peer-name)", name()),
    };
    let to = match to.to_ascii_lowercase().as_str() {
        "" => return Err(names()),
        "play" | "progression" | "game" => "play".to_string(),
        "editor" | "world-editor" | "survey" => "editor".to_string(),
        other => other.to_string(),
    };
    if to == name() {
        return Err(format!("already on the {} server", name()));
    }
    let peer = peer().filter(|p| p.name == to).ok_or_else(names)?;
    Ok((format!("Going to the {} server.", peer.name), peer))
}

/// The account a player's login cookie belongs to here when the peer sent them: this server's
/// account with the username of the cookie's account there, if the peer's account owns
/// `character` (the client keeps its character id) and no other account here does. `None` for
/// cookies of this server (or unknown ones).
pub fn arrival(cookie: u32, character: u32) -> Option<u32> {
    let db = crate::state::db();
    if crate::state::or_log(db.game_session(cookie), "session", None).is_some() {
        return None;
    }
    let peer = peer()?;
    let theirs = crate::state::or_log(peer.db.game_session(cookie), "peer session", None)?;
    let owner_there = crate::state::or_log(peer.db.character_account(character), "peer character", None).unwrap_or(character);
    if owner_there != theirs {
        warn!(character, "from the peer with another account's character");
        return None;
    }
    let name = crate::state::or_log(peer.db.account(theirs), "peer account", None)?.username;
    let mine = crate::state::or_log(db.account_by_name(&name), "account", None)?.id;
    match crate::state::or_log(db.character_account(character), "character", None) {
        Some(owner) if owner != mine => {
            warn!(character, %name, "from the peer: this character id belongs to another account here");
            None
        }
        _ => Some(mine),
    }
}

/// The chat cookie's account, also for a player the peer sent here (chat.rs).
pub fn session_account(cookie: u32) -> Option<u32> {
    let db = crate::state::db();
    crate::state::or_log(db.game_session(cookie), "session", None).or_else(|| {
        let peer = peer()?;
        let theirs = crate::state::or_log(peer.db.game_session(cookie), "peer session", None)?;
        let name = crate::state::or_log(peer.db.account(theirs), "peer account", None)?.username;
        crate::state::or_log(db.account_by_name(&name), "account", None).map(|a| a.id)
    })
}

/// Where the peer server last saved `character` (where a player sent there arrives; `None`:
/// new there, at the start).
pub fn saved_there(character: u32) -> Option<(u32, [f32; 3])> {
    let c = crate::state::or_log(peer()?.db.load_character(character), "peer character", None)?;
    Some((c.playfield, c.pos))
}

/// Marks the account as sent to the peer (true) or back here.
pub fn set_away(account: u32, away: bool) {
    let mut set = AWAY.lock().unwrap();
    if away {
        set.insert(account);
    } else {
        set.remove(&account);
    }
}

/// Where the chat of a player this server sent to the peer goes (`None`: stays here).
pub fn away(account: u32) -> Option<&'static Peer> {
    AWAY.lock().unwrap().contains(&account).then(peer).flatten()
}
