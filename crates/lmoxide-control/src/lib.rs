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
//! | POST | `/v1/players/{account}/command` | [`Command`] | [`CommandDone`] |
//! | GET | `/v1/test-commands` | | [`TestCatalog`] |
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

/// A test command's outcome: whether it reached the player, and what the server answered in
/// chat while running it (a teleport or a dialogue may add lines later, which aren't here).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDone {
    /// How many players were affected (0: not online).
    pub affected: u32,
    /// The chat lines from "Server" the command produced.
    #[serde(default)]
    pub replies: Vec<String>,
}

/// What the test commands accept (devcmd.rs), for tools that offer them as choices.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestCatalog {
    pub commands: Vec<TestCommand>,
    /// `!list <kind>` / `!goto <kind> [n]`
    pub kinds: Vec<TargetKind>,
    /// `!test <n|label>`
    pub scenarios: Vec<TestScenario>,
    /// `!dialog <n>`
    pub dialogues: Vec<TestDialogue>,
    /// `!loot <kind>`
    pub loot_kinds: Vec<String>,
    /// `!spawn [count] <name>`
    pub spawnable: Vec<Spawnable>,
    /// `!skin <monster data>`: the looks of the world's characters and smashables
    pub looks: Vec<Look>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCommand {
    /// e.g. `!goto <kind> [n]`
    pub usage: String,
    pub about: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetKind {
    pub kind: String,
    pub about: String,
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    /// for `!goto <kind> <n>` (1-based; the order can change when the world does, e.g. buildables)
    pub n: u32,
    pub label: String,
    pub playfield: u32,
    pub pos: [f32; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestScenario {
    /// for `!test <n>` (1-based)
    pub n: u32,
    pub label: String,
    pub about: String,
    /// minifigure names of the team it sets (the first active); empty: keeps the team
    pub team: Vec<String>,
    pub spawn: Vec<SpawnCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnCount {
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDialogue {
    /// for `!dialog <n>` (1-based)
    pub n: u32,
    /// the speaking NPC
    pub npc: String,
    pub playfield: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spawnable {
    pub name: String,
    /// `enemy` or `smashable`
    pub kind: String,
    pub level: u32,
    /// where it lives
    pub playfields: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Look {
    pub monster_data: u32,
    /// the names of the characters with this look
    pub names: Vec<String>,
    /// `enemy`, `smashable`, `npc` or `pet`
    pub kind: String,
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

    pub async fn command(&self, account: u32, command: &str) -> Result<CommandDone, Error> {
        self.send(self.http.post(format!("{}/v1/players/{account}/command", self.base)).json(&Command { command: command.into() }))
            .await
    }

    pub async fn test_catalog(&self) -> Result<TestCatalog, Error> {
        self.send(self.http.get(format!("{}/v1/test-commands", self.base))).await
    }

    pub async fn maintenance(&self, m: &Maintenance) -> Result<Status, Error> {
        self.send(self.http.post(format!("{}/v1/maintenance", self.base)).json(m)).await
    }
}

/// Compares two secrets in constant time (for the server side of the bearer check).
pub fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
