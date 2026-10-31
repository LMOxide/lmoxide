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
//! | POST | `/v1/friends/changed` | [`FriendsChanged`] | [`Done`] (online characters told) |
//! | any | `/v1/survey/...` | | survey servers (lmoxide-server's survey.rs, docs/SURVEY.md "API"): [`Client::survey`] |

use serde::{Deserialize, Serialize};

/// What a program was built from (build.rs): the workspace version, the git commit and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    pub version: String,
    /// The full commit hash; none when built without git or `LMOXIDE_GIT_COMMIT`.
    pub commit: Option<String>,
    /// Unix time of that commit.
    pub commit_time: Option<i64>,
    /// Unix time it was built (when the build script last ran: a new commit, or
    /// `LMOXIDE_BUILD_TIME`).
    pub built: Option<i64>,
}

impl Build {
    /// This build's (lmoxide-control's build.rs; the same for every crate of the workspace).
    pub fn this() -> Build {
        let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
        Build {
            version: env!("CARGO_PKG_VERSION").into(),
            commit: some(env!("LMOXIDE_GIT_COMMIT")),
            commit_time: env!("LMOXIDE_GIT_COMMIT_TIME").parse().ok(),
            built: env!("LMOXIDE_BUILD_TIME").parse().ok(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    /// What the game server was built from (none from servers before it was there).
    #[serde(default)]
    pub build: Option<Build>,
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
    /// Which running copy of the playfield (0: the progression mode's main one; see
    /// `/v1/instances`).
    #[serde(default)]
    pub copy: Option<u32>,
    /// The character's game mode (`progression`, `maxed`, `world-editor`).
    #[serde(default)]
    pub mode: Option<String>,
    /// Where the character is now (x, y up, z); `None` until it is in play.
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// The way the character faces, in degrees: 0 along +z, 90 along +x (as its client last
    /// reported); `None` until it is in play.
    #[serde(default)]
    pub heading: Option<f32>,
    pub gm_level: u32,
    /// Unix time of the login.
    pub logged_in_at: i64,
}

/// A running copy of a world (`GET /v1/instances`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub playfield: u32,
    /// 0, 1, 2: the progression, maxed and world editor modes' main copies; 3 and up: extra
    /// copies (a team's pocket world, an open world past its cap, a preview).
    pub copy: u32,
    /// The key clients know it by (`PlayfieldProxyData.running_instance`).
    pub key: u32,
    /// Its game mode.
    pub mode: String,
    /// `main`, `extra` or `preview`.
    pub kind: String,
    /// A preview's owner (character id).
    pub preview_owner: Option<u32>,
    /// Characters in it (players, NPCs that joined the world: pets, escorts, ...).
    pub characters: u32,
    pub players: u32,
    /// Its NPCs and smashables.
    pub npcs: u32,
}

/// Every running copy, and how long the NPCs' AI takes (`GET /v1/instances`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instances {
    pub instances: Vec<Instance>,
    /// The last AI tick (every copy with players, ms) and the longest in the last minute.
    pub ai_tick_ms: f32,
    pub ai_tick_max_ms: f32,
    /// Players an open world's copy takes before arrivals get another (`--open-world-cap`).
    pub open_world_cap: u32,
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

/// Character friendships the API made or ended (account friends): online characters' friend
/// lists are updated. Pairs of character ids.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FriendsChanged {
    #[serde(default)]
    pub added: Vec<(u32, u32)>,
    #[serde(default)]
    pub removed: Vec<(u32, u32)>,
}

/// A test command (`!loot potion`) run as if the player had typed it in chat; the player's GM
/// level still decides whether it runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub command: String,
}

/// Achievements to take away from a character: `ids`, or with `all` every one (other tags, the
/// story and tutorial progress, stay).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoveAchievements {
    #[serde(default)]
    pub ids: Vec<u32>,
    #[serde(default)]
    pub all: bool,
}

/// What was taken away (with the earned "all of" achievements above them), and whether the
/// character was online (then its client was updated at once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovedAchievements {
    pub removed: Vec<u32>,
    pub online: bool,
}

/// A character's new name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameCharacter {
    pub name: String,
}

/// The name as stored, and whether the character was online (then every client nearby shows
/// it at once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterRenamed {
    pub name: String,
    pub online: bool,
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
    /// The GM level the command needs (the alpha testers' commands need less).
    #[serde(default)]
    pub gm_level: u32,
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
    /// where it lives (none: a template no world places, spawned by name)
    pub playfields: Vec<u32>,
    /// its look (monster data; 0 none)
    #[serde(default)]
    pub monster_data: u32,
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

    pub async fn instances(&self) -> Result<Instances, Error> {
        self.send(self.http.get(format!("{}/v1/instances", self.base))).await
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

    pub async fn remove_achievements(&self, character: u32, r: &RemoveAchievements) -> Result<RemovedAchievements, Error> {
        self.send(self.http.post(format!("{}/v1/characters/{character}/achievements/remove", self.base)).json(r)).await
    }

    /// Renames a character. Refusals come back as [`Error::Status`]: 404 no such character, 409
    /// another account's character has the name, 422 not a character name.
    pub async fn rename_character(&self, character: u32, r: &RenameCharacter) -> Result<CharacterRenamed, Error> {
        self.send(self.http.post(format!("{}/v1/characters/{character}/name", self.base)).json(r)).await
    }

    pub async fn test_catalog(&self) -> Result<TestCatalog, Error> {
        self.send(self.http.get(format!("{}/v1/test-commands", self.base))).await
    }

    pub async fn friends_changed(&self, f: &FriendsChanged) -> Result<Done, Error> {
        self.send(self.http.post(format!("{}/v1/friends/changed", self.base)).json(f)).await
    }

    pub async fn maintenance(&self, m: &Maintenance) -> Result<Status, Error> {
        self.send(self.http.post(format!("{}/v1/maintenance", self.base)).json(m)).await
    }

    /// Sends a `/v1/survey` request on as it is (lmoxide-api's survey pass-through), with who
    /// made it (`X-Lmoxide-Actor`): the answer's status, content type and body, errors included.
    pub async fn survey(&self, method: &str, path_and_query: &str, actor: &str, body: Vec<u8>) -> Result<(u16, Option<String>, Vec<u8>), Error> {
        self.forward("survey", method, path_and_query, actor, body).await
    }

    /// A play server's world content routes (`/v1/content/...`), forwarded as `survey` is.
    pub async fn content(&self, method: &str, path_and_query: &str, actor: &str, body: Vec<u8>) -> Result<(u16, Option<String>, Vec<u8>), Error> {
        self.forward("content", method, path_and_query, actor, body).await
    }

    /// A survey server's content editing routes (`/v1/editor/...`, the world editor's forms),
    /// forwarded as `survey` is.
    pub async fn editor(&self, method: &str, path_and_query: &str, actor: &str, body: Vec<u8>) -> Result<(u16, Option<String>, Vec<u8>), Error> {
        self.forward("editor", method, path_and_query, actor, body).await
    }

    async fn forward(&self, area: &str, method: &str, path_and_query: &str, actor: &str, body: Vec<u8>) -> Result<(u16, Option<String>, Vec<u8>), Error> {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
        let mut req = self.http.request(method, format!("{}/v1/{area}/{}", self.base, path_and_query.trim_start_matches('/')));
        req = req.bearer_auth(&self.token).header("x-lmoxide-actor", actor);
        if !body.is_empty() {
            req = req.header(reqwest::header::CONTENT_TYPE, "application/json").body(body);
        }
        let resp = req.send().await.map_err(Error::Unreachable)?;
        let status = resp.status().as_u16();
        let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
        let body = resp.bytes().await.map_err(Error::Decode)?.to_vec();
        Ok((status, content_type, body))
    }
}

/// Compares two secrets in constant time (for the server side of the bearer check).
pub fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
