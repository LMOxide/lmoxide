//! The game server's internal control interface: a small JSON-over-HTTP API that
//! `lmoxide-server` serves on `--control-bind` (private by default) for `lmoxide-api`, which
//! adds user authentication, scopes and rate limits in front of it.
//!
//! Every request carries `Authorization: Bearer <control token>` (a shared secret, at least 32
//! characters). Routes, relative to the control address:
//!
//! | Method | Path | Body | Answer |
//! |---|---|---|---|
//! | GET | `/v1/status` | | [`Status`] |
//! | GET | `/v1/players` | | `Vec<`[`OnlinePlayer`]`>` |
//! | POST | `/v1/players/{account}/kick` | [`Kick`] | [`Done`] |
//! | POST | `/v1/broadcast` | [`Broadcast`] | [`Done`] |
//! | POST | `/v1/maintenance` | [`Maintenance`] | [`Status`] |

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    /// Unix time the server started.
    pub started_at: i64,
    pub players_online: u32,
    /// While on, only accounts with a GM level may log in.
    pub maintenance: bool,
    pub maintenance_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlinePlayer {
    pub account_id: u32,
    pub username: String,
    pub character_id: u32,
    pub character_name: String,
    /// `None` until the character is in play.
    pub playfield: Option<u32>,
    pub gm_level: u32,
    /// Unix time of the login.
    pub logged_in_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Kick {
    /// Shown to the player in chat before the disconnect.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Broadcast {
    pub message: String,
}

/// A test command (`!loot potion`) run as if the player had typed it in chat; the player's GM
/// level still decides whether it runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Maintenance {
    pub enabled: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Done {
    /// How many players were affected.
    pub affected: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("game server unreachable: {0}")]
    Unreachable(#[source] reqwest::Error),
    #[error("game server answered {status}: {body}")]
    Status { status: u16, body: String },
    #[error("bad answer from the game server: {0}")]
    Decode(#[source] reqwest::Error),
}

/// Client for the control interface.
#[derive(Clone)]
pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Client {
    /// `base` is e.g. `http://127.0.0.1:7300`.
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("HTTP client");
        Self { base: base.into().trim_end_matches('/').to_string(), token: token.into(), http }
    }

    async fn send<T: for<'de> Deserialize<'de>>(&self, req: reqwest::RequestBuilder) -> Result<T, Error> {
        let resp = req.bearer_auth(&self.token).send().await.map_err(Error::Unreachable)?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Status { status: status.as_u16(), body });
        }
        resp.json().await.map_err(Error::Decode)
    }

    pub async fn status(&self) -> Result<Status, Error> {
        self.send(self.http.get(format!("{}/v1/status", self.base))).await
    }

    pub async fn players(&self) -> Result<Vec<OnlinePlayer>, Error> {
        self.send(self.http.get(format!("{}/v1/players", self.base))).await
    }

    pub async fn kick(&self, account: u32, kick: &Kick) -> Result<Done, Error> {
        self.send(self.http.post(format!("{}/v1/players/{account}/kick", self.base)).json(kick)).await
    }

    pub async fn broadcast(&self, message: &str) -> Result<Done, Error> {
        self.send(self.http.post(format!("{}/v1/broadcast", self.base)).json(&Broadcast { message: message.into() }))
            .await
    }

    pub async fn maintenance(&self, m: &Maintenance) -> Result<Status, Error> {
        self.send(self.http.post(format!("{}/v1/maintenance", self.base)).json(m)).await
    }
}

/// Compares two secrets in constant time (for the server side of the bearer check).
pub fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
