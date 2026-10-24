//! Who is calling: `Authorization: Bearer <secret>` with an API key or a login session token
//! (both `lmox_...`). API keys carry a scope; a session's scope follows its account's GM level
//! (`--moderator-gm-level`, `--admin-gm-level`), and accounts below both only reach `/v1/me`.
//!
//! An API key used by a bot on behalf of someone else (a Discord moderator) may say who in
//! `X-On-Behalf-Of`; it is recorded in the audit log next to the key's name. It is the key
//! holder's claim, not authentication.
//!
//! Also the request throttles: a token bucket per client address for every request, and a
//! failure lockout for logins and registrations.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use lmoxide_db::Scope;

use crate::{AppState, error::ApiError};

/// An authenticated caller.
#[derive(Debug, Clone)]
pub struct Auth {
    /// For the audit log: `api-key:<name>` or `account:<username>`.
    pub actor: String,
    /// `None`: an account without admin rights (only its own data).
    pub scope: Option<Scope>,
    /// Set for login sessions.
    pub account: Option<u32>,
    /// Set for login sessions (to log out).
    pub token: Option<String>,
}

impl Auth {
    pub fn require(&self, scope: Scope) -> Result<(), ApiError> {
        if self.scope.is_some_and(|s| s >= scope) { Ok(()) } else { Err(ApiError::forbidden()) }
    }
}

/// The scope an account's login session gets.
pub fn account_scope(state: &AppState, gm_level: u32) -> Option<Scope> {
    if gm_level >= state.cfg.admin_gm_level {
        Some(Scope::Admin)
    } else if gm_level >= state.cfg.moderator_gm_level {
        Some(Scope::Moderate)
    } else {
        None
    }
}

/// The `X-On-Behalf-Of` header, if usable: printable, at most 100 characters.
fn on_behalf_of(parts: &Parts) -> Option<String> {
    let v = parts.headers.get("x-on-behalf-of")?.to_str().ok()?.trim();
    (!v.is_empty() && v.len() <= 100 && !v.chars().any(char::is_control)).then(|| v.to_string())
}

fn bearer(parts: &Parts) -> Option<&str> {
    parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim)
}

impl FromRequestParts<Arc<AppState>> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> Result<Self, ApiError> {
        let token = bearer(parts).ok_or_else(ApiError::unauthorized)?.to_string();
        let behalf = on_behalf_of(parts);
        let st = state.clone();
        crate::blocking(state, move |db| {
            if let Some(key) = db.authenticate_api_key(&token)? {
                let actor = match behalf {
                    Some(who) => format!("api-key:{} for {who}", key.name),
                    None => format!("api-key:{}", key.name),
                };
                return Ok(Auth { actor, scope: Some(key.scope), account: None, token: None });
            }
            let Some((account_id, _)) = db.session_account(&token)? else {
                return Err(ApiError::unauthorized());
            };
            let account = db.account(account_id)?.ok_or_else(ApiError::unauthorized)?;
            if account.active_ban().is_some() {
                return Err(ApiError::unauthorized());
            }
            Ok(Auth {
                actor: format!("account:{}", account.username),
                scope: account_scope(&st, account.gm_level),
                account: Some(account.id),
                token: Some(token),
            })
        })
        .await
    }
}

/// Credentials if given, for routes that are public but say more to authenticated callers.
pub struct MaybeAuth(pub Option<Auth>);

impl FromRequestParts<Arc<AppState>> for MaybeAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> Result<Self, ApiError> {
        if bearer(parts).is_none() {
            return Ok(MaybeAuth(None));
        }
        Auth::from_request_parts(parts, state).await.map(|a| MaybeAuth(Some(a)))
    }
}

/// The caller's address: the TCP peer, or behind a reverse proxy (`--trust-forwarded-for`)
/// the last `X-Forwarded-For` entry, which that proxy appended.
pub fn client_ip(state: &AppState, parts_headers: &axum::http::HeaderMap, peer: SocketAddr) -> IpAddr {
    if state.cfg.trust_forwarded_for
        && let Some(ip) = parts_headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse().ok())
    {
        return ip;
    }
    peer.ip()
}

/// Requests per second allowed per address, with bursts up to [`BURST`].
const RATE: f64 = 10.0;
const BURST: f64 = 60.0;

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, (f64, Instant)>>,
}

impl RateLimiter {
    fn take(&self, ip: IpAddr) -> bool {
        let mut b = self.buckets.lock().unwrap();
        let now = Instant::now();
        if b.len() > 10_000 {
            // forget idle addresses (full buckets)
            b.retain(|_, (_, t)| now.duration_since(*t) < Duration::from_secs((BURST / RATE) as u64));
        }
        let (tokens, last) = b.entry(ip).or_insert((BURST, now));
        *tokens = (*tokens + now.duration_since(*last).as_secs_f64() * RATE).min(BURST);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub async fn rate_limit(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let ip = client_ip(&state, req.headers(), peer);
    if !state.limiter.take(ip) {
        return ApiError::too_many().into_response();
    }
    next.run(req).await
}

/// Failed logins/registrations allowed per key within [`FAIL_WINDOW`].
const MAX_FAILURES: usize = 10;
const FAIL_WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
pub struct Lockout {
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl Lockout {
    pub fn allowed(&self, key: &str) -> bool {
        let mut f = self.failures.lock().unwrap();
        let now = Instant::now();
        let Some(times) = f.get_mut(key) else { return true };
        times.retain(|t| now.duration_since(*t) < FAIL_WINDOW);
        times.len() < MAX_FAILURES
    }

    pub fn failed(&self, key: &str) {
        let mut f = self.failures.lock().unwrap();
        let times = f.entry(key.to_string()).or_default();
        times.push(Instant::now());
        if times.len() > MAX_FAILURES * 2 {
            times.drain(..times.len() - MAX_FAILURES * 2);
        }
    }

    pub fn succeeded(&self, key: &str) {
        self.failures.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_runs_dry() {
        let l = RateLimiter::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let allowed = (0..100).filter(|_| l.take(ip)).count();
        assert_eq!(allowed, BURST as usize);
    }

    #[test]
    fn lockout() {
        let l = Lockout::default();
        for _ in 0..MAX_FAILURES {
            assert!(l.allowed("k"));
            l.failed("k");
        }
        assert!(!l.allowed("k"));
        l.succeeded("k");
        assert!(l.allowed("k"));
    }
}
