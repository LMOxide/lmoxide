//! Protection for servers reachable from the internet: per-address connection limits and
//! failure throttling (password guesses, forged cookies).

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

/// Open connections per address.
static CONNECTIONS: LazyLock<Mutex<HashMap<IpAddr, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Counts a new connection from `ip`; `None` if the address already has `max` open.
pub fn connection(ip: IpAddr, max: u32) -> Option<ConnectionGuard> {
    let mut c = CONNECTIONS.lock().unwrap();
    let n = c.entry(ip).or_default();
    if *n >= max {
        return None;
    }
    *n += 1;
    Some(ConnectionGuard(ip))
}

pub struct ConnectionGuard(IpAddr);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut c = CONNECTIONS.lock().unwrap();
        if let Some(n) = c.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                c.remove(&self.0);
            }
        }
    }
}

/// Failures allowed per key within [`WINDOW`] before it is locked out for the rest of it.
const MAX_FAILURES: usize = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);

/// Recent failures by key (`ip:<addr>`, `user:<name>`).
static FAILURES: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether `key` may try again (fewer than [`MAX_FAILURES`] recent failures).
pub fn allowed(key: &str) -> bool {
    let mut f = FAILURES.lock().unwrap();
    let now = Instant::now();
    let Some(times) = f.get_mut(key) else { return true };
    times.retain(|t| now.duration_since(*t) < WINDOW);
    let ok = times.len() < MAX_FAILURES;
    if times.is_empty() {
        f.remove(key);
    }
    ok
}

pub fn failed(key: &str) {
    let mut f = FAILURES.lock().unwrap();
    let times = f.entry(key.to_string()).or_default();
    times.push(Instant::now());
    // keep memory bounded if a key is hammered
    if times.len() > MAX_FAILURES * 2 {
        times.drain(..times.len() - MAX_FAILURES * 2);
    }
}

pub fn succeeded(key: &str) {
    FAILURES.lock().unwrap().remove(key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockout_after_failures() {
        let key = "user:test-lockout";
        for _ in 0..MAX_FAILURES {
            assert!(allowed(key));
            failed(key);
        }
        assert!(!allowed(key));
        succeeded(key);
        assert!(allowed(key));
    }

    #[test]
    fn connection_limit() {
        let ip: IpAddr = "192.0.2.7".parse().unwrap();
        let a = connection(ip, 2).unwrap();
        let _b = connection(ip, 2).unwrap();
        assert!(connection(ip, 2).is_none());
        drop(a);
        assert!(connection(ip, 2).is_some());
    }
}
