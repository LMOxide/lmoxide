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

/// account instance -> login name (the name of a new character)
static NAMES: LazyLock<Mutex<HashMap<u32, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_name(account: u32, name: &str) {
    NAMES.lock().unwrap().insert(account, name.to_string());
}

pub fn name(account: u32) -> Option<String> {
    NAMES.lock().unwrap().get(&account).cloned()
}

/// A logged-in game connection (for the control interface: players online, kicks).
struct Online {
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

/// Registers the account's game connection, kicking an older one; dropping the guard
/// unregisters it.
pub fn online(account: u32, username: &str, character_name: &str, gm_level: u32, outbox: world::Outbox) -> OnlineGuard {
    let conn_id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let entry = Online {
        conn_id,
        username: username.into(),
        character_name: character_name.into(),
        gm_level,
        logged_in_at: lmoxide_db::now(),
        outbox,
    };
    if let Some(old) = ONLINE.lock().unwrap().insert(account, entry) {
        let _ = old.outbox.send(world::Out::Kick(Some("Logged in from another client.".into())));
    }
    OnlineGuard { account, conn_id }
}

pub struct OnlineGuard {
    account: u32,
    conn_id: u64,
}

impl Drop for OnlineGuard {
    fn drop(&mut self) {
        let mut online = ONLINE.lock().unwrap();
        if online.get(&self.account).is_some_and(|o| o.conn_id == self.conn_id) {
            online.remove(&self.account);
        }
    }
}

/// The GM level of an account that is online (0 otherwise).
pub fn gm_level(account: u32) -> u32 {
    ONLINE.lock().unwrap().get(&account).map_or(0, |o| o.gm_level)
}

/// Disconnects the account's game connection; false if it isn't online.
pub fn kick(account: u32, reason: Option<String>) -> bool {
    ONLINE.lock().unwrap().get(&account).is_some_and(|o| o.outbox.send(world::Out::Kick(reason)).is_ok())
}

pub fn players() -> Vec<lmoxide_control::OnlinePlayer> {
    let online = ONLINE.lock().unwrap();
    let mut list: Vec<_> = online
        .iter()
        .map(|(&account, o)| lmoxide_control::OnlinePlayer {
            account_id: account,
            username: o.username.clone(),
            character_id: account,
            character_name: o.character_name.clone(),
            playfield: world::whereabouts(Id32::new(crate::territory::CHARACTER_TYPE, account)).map(|(pf, _)| pf),
            gm_level: o.gm_level,
            logged_in_at: o.logged_in_at,
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
