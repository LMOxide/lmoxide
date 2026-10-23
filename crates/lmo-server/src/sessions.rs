//! State shared between the territory and game connections of one account.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use tokio::sync::watch;

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

/// account instance -> login name (shown as the character name until characters are stored)
static NAMES: LazyLock<Mutex<HashMap<u32, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_name(account: u32, name: &str) {
    NAMES.lock().unwrap().insert(account, name.to_string());
}

pub fn name(account: u32) -> Option<String> {
    NAMES.lock().unwrap().get(&account).cloned()
}
