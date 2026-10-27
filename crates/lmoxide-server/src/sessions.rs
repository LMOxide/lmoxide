//! State shared between the territory and game connections of one account.

use std::{
    collections::HashMap,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use lmoxide_net::types::Id32;
use tokio::sync::watch;

use crate::world;

/// account instance -> "territory connection is open"
static TERRITORY: LazyLock<Mutex<HashMap<u32, watch::Sender<bool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Marks the account's territory connection open; drop the guard when it closes.
pub fn territory_opened(account: u32) -> TerritoryGuard {
    let (tx, _) = watch::channel(true);
    TERRITORY.lock().unwrap().insert(account, tx);
    TerritoryGuard(account)
}

pub struct TerritoryGuard(u32);

impl Drop for TerritoryGuard {
    fn drop(&mut self) {
        if let Some(tx) = TERRITORY.lock().unwrap().remove(&self.0) {
            let _ = tx.send(false);
        }
    }
}

/// Waits (up to `timeout`) until the account has no open territory connection.
///
/// The client drops the territory connection itself when it moves to the game server, but
/// FlowControl only tolerates that while still in WaitingForFullUpdate (0x410290): if the
/// character arrives first, the client reaches WaitingToSendInPlay, sees the territory
/// connection go, and logs out. So the game server sends the world only after it is gone.
pub async fn territory_closed(account: u32, timeout: Duration) -> bool {
    let rx = TERRITORY.lock().unwrap().get(&account).map(|tx| tx.subscribe());
    let Some(mut rx) = rx else { return true };
    tokio::time::timeout(timeout, rx.wait_for(|open| !*open)).await.is_ok()
}

/// account instance -> the character it plays (picked in the territory; the chat server finds
/// the character by the account's login cookie)
static CHARACTERS: LazyLock<Mutex<HashMap<u32, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_character(account: u32, character: u32) {
    CHARACTERS.lock().unwrap().insert(account, character);
}

/// The character the account plays (its first, the account's id, if none was picked).
pub fn character_of(account: u32) -> u32 {
    CHARACTERS.lock().unwrap().get(&account).copied().unwrap_or(account)
}

/// account instance -> login name (the name of a new character)
static NAMES: LazyLock<Mutex<HashMap<u32, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_name(account: u32, name: &str) {
    NAMES.lock().unwrap().insert(account, name.to_string());
}

pub fn name(account: u32) -> Option<String> {
    NAMES.lock().unwrap().get(&account).cloned()
}

/// A logged-in game connection (for the control interface: players online, kicks), by character.
struct Online {
    account: u32,
    /// Tells apart a newer connection of the same account.
    conn_id: u64,
    username: String,
    character_name: String,
    gm_level: u32,
    logged_in_at: i64,
    outbox: world::Outbox,
}

static ONLINE: LazyLock<Mutex<HashMap<u32, Online>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// Registers the account's game connection with `character`, kicking an older one of the
/// account; dropping the guard unregisters it.
pub fn online(account: u32, character: u32, username: &str, character_name: &str, gm_level: u32, outbox: world::Outbox) -> OnlineGuard {
    let conn_id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let entry = Online {
        account,
        conn_id,
        username: username.into(),
        character_name: character_name.into(),
        gm_level,
        logged_in_at: lmoxide_db::now(),
        outbox,
    };
    let mut online = ONLINE.lock().unwrap();
    for old in online.values().filter(|o| o.account == account) {
        let _ = old.outbox.send(world::Out::Kick(Some("Logged in from another client.".into())));
    }
    online.insert(character, entry);
    OnlineGuard { character, conn_id }
}

pub struct OnlineGuard {
    character: u32,
    conn_id: u64,
}

impl Drop for OnlineGuard {
    fn drop(&mut self) {
        let mut online = ONLINE.lock().unwrap();
        if online.get(&self.character).is_some_and(|o| o.conn_id == self.conn_id) {
            online.remove(&self.character);
        }
    }
}

/// Whether a character is in play.
pub fn is_online(character: u32) -> bool {
    ONLINE.lock().unwrap().contains_key(&character)
}

/// The GM level of a character that is online (0 otherwise).
pub fn gm_level(character: u32) -> u32 {
    ONLINE.lock().unwrap().get(&character).map_or(0, |o| o.gm_level)
}

/// Disconnects the account's game connection; false if it isn't online.
pub fn kick(account: u32, reason: Option<String>) -> bool {
    let online = ONLINE.lock().unwrap();
    let mut kicked = false;
    for o in online.values().filter(|o| o.account == account) {
        kicked |= o.outbox.send(world::Out::Kick(reason.clone())).is_ok();
    }
    kicked
}

/// Runs a test command as the account's character (as typed in chat): the characters it went
/// to, each with a receiver that resolves (with an error: the sender is dropped) once the
/// command has been handled. Empty if the account isn't online.
pub fn command(account: u32, text: &str) -> Vec<(u32, tokio::sync::oneshot::Receiver<()>)> {
    let online = ONLINE.lock().unwrap();
    let mut sent = Vec::new();
    for (&character, o) in online.iter().filter(|(_, o)| o.account == account) {
        let (done, handled) = tokio::sync::oneshot::channel();
        if o.outbox.send(world::Out::Command(text.to_string(), Some(std::sync::Arc::new(done)))).is_ok() {
            sent.push((character, handled));
        }
    }
    sent
}

/// The account's characters online (usually one).
pub fn characters_of(account: u32) -> Vec<u32> {
    ONLINE.lock().unwrap().iter().filter(|(_, o)| o.account == account).map(|(&c, _)| c).collect()
}

pub fn players() -> Vec<lmoxide_control::OnlinePlayer> {
    let online = ONLINE.lock().unwrap();
    let mut list: Vec<_> = online
        .iter()
        .map(|(&character, o)| {
            let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
            lmoxide_control::OnlinePlayer {
                account_id: o.account,
                username: o.username.clone(),
                character_id: character,
                character_name: o.character_name.clone(),
                playfield: world::whereabouts(id).map(|(pf, _)| pf),
                position: world::live_position(id),
                gm_level: o.gm_level,
                logged_in_at: o.logged_in_at,
            }
        })
        .collect();
    list.sort_by(|a, b| a.character_name.cmp(&b.character_name));
    list
}

pub fn players_online() -> u32 {
    ONLINE.lock().unwrap().len() as u32
}

/// Disconnects everyone (shutdown); their connections save the characters as they close.
pub fn kick_all(reason: &str) {
    for o in ONLINE.lock().unwrap().values() {
        let _ = o.outbox.send(world::Out::Kick(Some(reason.into())));
    }
}
