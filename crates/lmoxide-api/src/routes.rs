//! The `/v1` routes. Each handler's `#[utoipa::path]` feeds the OpenAPI document served at
//! `/v1/openapi.json`.

use std::{collections::BTreeMap, net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use lmoxide_db::{Ban, Scope};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppState,
    auth::{Auth, MaybeAuth, account_scope, client_ip},
    blocking,
    error::{ApiError, ApiResult, ErrorBody},
};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(online_players))
        .routes(routes!(config))
        .routes(routes!(playfields))
        .routes(routes!(minifigures))
        .routes(routes!(tokens))
        .routes(routes!(login))
        .routes(routes!(logout))
        .routes(routes!(renew))
        .routes(routes!(register))
        .routes(routes!(me))
        .routes(routes!(change_own_password))
        .routes(routes!(my_players))
        .routes(routes!(my_command))
        .merge(crate::discord::router())
        .routes(routes!(instances))
        .routes(routes!(players))
        .routes(routes!(kick))
        .routes(routes!(player_command))
        .routes(routes!(remove_achievements))
        .routes(routes!(test_commands))
        .merge(crate::catalog::router())
        .routes(routes!(broadcast))
        .routes(routes!(maintenance))
        .routes(routes!(list_accounts, create_account))
        .routes(routes!(get_account, update_account, delete_account))
        .routes(routes!(set_password))
        .routes(routes!(ban))
        .routes(routes!(unban))
        .routes(routes!(get_character))
        .routes(routes!(list_api_keys, create_api_key))
        .routes(routes!(revoke_api_key))
        .routes(routes!(audit_log))
}

// ------------------------------------------------------------------ models

/// What a credential may do; each includes the ones before it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApiScope {
    Read,
    Moderate,
    /// The Discord bot's key: `moderate` plus the `/v1/discord` routes.
    Discord,
    Admin,
}

impl From<Scope> for ApiScope {
    fn from(s: Scope) -> Self {
        match s {
            Scope::Read => ApiScope::Read,
            Scope::Moderate => ApiScope::Moderate,
            Scope::Discord => ApiScope::Discord,
            Scope::Admin => ApiScope::Admin,
        }
    }
}

impl From<ApiScope> for Scope {
    fn from(s: ApiScope) -> Self {
        match s {
            ApiScope::Read => Scope::Read,
            ApiScope::Moderate => Scope::Moderate,
            ApiScope::Discord => Scope::Discord,
            ApiScope::Admin => Scope::Admin,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct Status {
    /// Whether the game server answered.
    pub game_server_online: bool,
    pub version: Option<String>,
    /// Unix time the game server started.
    pub started_at: Option<i64>,
    pub players_online: Option<u32>,
    pub maintenance: Option<bool>,
    pub maintenance_message: Option<String>,
}

/// A player as anyone may see them (in-game names only).
#[derive(Serialize, ToSchema)]
pub struct PublicPlayer {
    pub character_name: String,
    /// Playfield id; `null` while loading.
    pub playfield: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct Player {
    pub account_id: u32,
    pub username: String,
    pub character_id: u32,
    pub character_name: String,
    pub playfield: Option<u32>,
    /// Which running copy of the playfield (`/v1/instances`); `null` until it is in play.
    pub copy: Option<u32>,
    /// The character's game mode: `progression`, `maxed` or `world-editor`.
    pub mode: Option<String>,
    /// Where the character is now (x, y up, z); `null` until it is in play.
    pub position: Option<[f32; 3]>,
    /// The way the character faces, in degrees: 0 along +z, 90 along +x; `null` until it is in
    /// play.
    pub heading: Option<f32>,
    pub gm_level: u32,
    /// Unix time.
    pub logged_in_at: i64,
}

impl From<lmoxide_control::OnlinePlayer> for Player {
    fn from(p: lmoxide_control::OnlinePlayer) -> Self {
        Player {
            account_id: p.account_id,
            username: p.username,
            character_id: p.character_id,
            character_name: p.character_name,
            playfield: p.playfield,
            copy: p.copy,
            mode: p.mode,
            position: p.position,
            heading: p.heading,
            gm_level: p.gm_level,
            logged_in_at: p.logged_in_at,
        }
    }
}

/// A running copy of a world.
#[derive(Serialize, ToSchema)]
pub struct Instance {
    pub playfield: u32,
    /// 0, 1, 2: the progression, maxed and world editor modes' main copies; 3 and up: extra
    /// copies (a team's pocket world, an open world past its cap, a preview).
    pub copy: u32,
    /// The key clients know it by.
    pub key: u32,
    /// Its game mode: `progression`, `maxed` or `world-editor`.
    pub mode: String,
    /// `main`, `extra` or `preview`.
    pub kind: String,
    /// A preview's owner (character id).
    pub preview_owner: Option<u32>,
    /// Characters in it (players and the NPCs that joined the world: pets, escorts).
    pub characters: u32,
    pub players: u32,
    /// Its NPCs and smashables.
    pub npcs: u32,
}

/// Every running copy of a world, and how long the NPCs' AI takes.
#[derive(Serialize, ToSchema)]
pub struct Instances {
    pub instances: Vec<Instance>,
    /// The last AI tick (every copy with players), ms, and the longest in the last minute.
    pub ai_tick_ms: f32,
    pub ai_tick_max_ms: f32,
    /// Players an open world's copy takes before arrivals get another.
    pub open_world_cap: u32,
}

#[derive(Serialize, ToSchema)]
pub struct BanInfo {
    /// Unix time the ban ends; `null`: permanent.
    pub until: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Account {
    pub id: u32,
    pub username: String,
    pub has_password: bool,
    pub gm_level: u32,
    /// Unix time.
    pub created_at: i64,
    pub last_login_at: Option<i64>,
    /// Only shown to admins.
    pub last_login_ip: Option<String>,
    /// The ban in force, if any.
    pub ban: Option<BanInfo>,
    /// The linked Discord user id (a snowflake, as a string); `null`: not linked.
    pub discord_id: Option<String>,
    pub characters: Vec<CharacterSummary>,
}

#[derive(Serialize, ToSchema)]
pub struct CharacterSummary {
    pub id: u32,
    pub name: String,
    /// The game mode the character belongs to: `progression`, `maxed` or `world-editor`.
    pub mode: String,
    pub playfield: u32,
    pub stars: u64,
    /// Unix time of the last save.
    pub saved_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct Character {
    pub id: u32,
    pub name: String,
    /// The game mode the character belongs to: `progression`, `maxed` or `world-editor`.
    pub mode: String,
    pub playfield: u32,
    pub position: [f32; 3],

    /// Minifigure item templates in team order (the first is active).
    pub team: Vec<u32>,
    pub stars: u64,
    /// Active upgrade bitmask per minifigure template.
    pub upgrades: BTreeMap<u32, u32>,
    /// Token counters (minifigure parts): token id -> gained, spent.
    pub tokens: BTreeMap<u32, TokenCount>,
    /// Player XP.
    pub xp: u64,
    /// Player level; 0 = never set (a game server without `--progression` plays every
    /// character at level 30).
    pub level: u32,
    /// Minifigure ranks (0..8) by item template, where set (`--progression`).
    pub minifigure_ranks: BTreeMap<u32, u32>,
    /// Earned achievements.
    pub achievements: Vec<EarnedAchievement>,
    /// Achievement counters: stat -> minifigure template (0 = the player) -> value (stat 3000116
    /// enemies defeated, 3000126 smashables smashed, 3000112/3000113 per-minifigure counters).
    pub counters: BTreeMap<u32, BTreeMap<u32, u64>>,
    /// What the character defeated, by name (enemies, champions, bosses, smashables).
    pub defeats: BTreeMap<String, DefeatCount>,
    /// Playfields entered, by playfield id.
    pub visits: BTreeMap<u32, VisitCount>,
    /// Collectibles and places found ("<playfield>:<world object name>") -> unix time.
    pub finds: BTreeMap<String, i64>,
}

#[derive(Serialize, ToSchema)]
pub struct DefeatCount {
    /// enemy, champion, boss or smashable
    pub kind: String,
    pub count: u64,
    /// Unix times of the first and the last.
    pub first_at: i64,
    pub last_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct VisitCount {
    pub count: u64,
    /// Unix times of the first and the last.
    pub first_at: i64,
    pub last_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct EarnedAchievement {
    /// Lore tree tag id.
    pub id: u32,
    /// Its name in the client data, e.g. "Global_Smash 1000 Monsters" (empty: not an
    /// achievement, story or tutorial progress).
    pub name: String,
    /// As the game shows it: its title ("Fiery Explorer"), the title of the achievement it is
    /// part of ("Paleontologist") and its text ("Find the Volcano Summit in Dinosaur Rise.").
    pub title: Option<String>,
    pub group: Option<String>,
    pub text: Option<String>,
    /// Achievement points.
    pub points: u32,
    /// Unix time it was earned.
    pub achieved_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct TokenCount {
    pub gained: u64,
    pub spent: u64,
}

#[derive(Deserialize, ToSchema)]
pub struct Credentials {
    #[schema(min_length = 3, max_length = 32, pattern = "^[A-Za-z0-9_.-]+$")]
    pub username: String,
    #[schema(min_length = 8, max_length = 128)]
    pub password: String,
}

#[derive(Deserialize, ToSchema)]
pub struct Login {
    #[schema(min_length = 3, max_length = 32, pattern = "^[A-Za-z0-9_.-]+$")]
    pub username: String,
    #[schema(min_length = 8, max_length = 128)]
    pub password: String,
    /// Stay signed in: the session lasts `--remember-days` instead of `--session-ttl-hours`
    /// (for a token kept across browser restarts; renew it with `/v1/auth/renew`).
    #[serde(default)]
    pub remember: bool,
}

#[derive(Deserialize, ToSchema, Default)]
pub struct Renew {
    /// As in the login: a long session (`--remember-days`) or a short one.
    #[serde(default)]
    pub remember: bool,
}

#[derive(Serialize, ToSchema)]
pub struct Session {
    /// Send as `Authorization: Bearer <token>`.
    pub token: String,
    /// Unix time.
    pub expires_at: i64,
    /// `null`: the account may only see and change itself (`/v1/me`).
    pub scope: Option<ApiScope>,
    pub account: Account,
    /// A session from a one-time sign-in code (`/v1/auth/code`): `/v1/me/password` takes a new
    /// password without the current one.
    pub password_reset: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct ChangePassword {
    /// Required, except in a `password_reset` session.
    #[serde(default)]
    pub current_password: Option<String>,
    #[schema(min_length = 8, max_length = 128)]
    pub new_password: String,
}

#[derive(Deserialize, ToSchema)]
pub struct NewAccount {
    #[schema(min_length = 3, max_length = 32, pattern = "^[A-Za-z0-9_.-]+$")]
    pub username: String,
    /// Without one the account can't log in until a password is set.
    #[schema(min_length = 8, max_length = 128)]
    pub password: Option<String>,
    #[serde(default)]
    pub gm_level: u32,
}

#[derive(Deserialize, ToSchema)]
pub struct AccountChanges {
    #[schema(min_length = 3, max_length = 32, pattern = "^[A-Za-z0-9_.-]+$")]
    pub username: Option<String>,
    pub gm_level: Option<u32>,
}

#[derive(Deserialize, ToSchema)]
pub struct SetPassword {
    /// `null` removes the password (the account can't log in).
    #[schema(min_length = 8, max_length = 128)]
    pub password: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct BanRequest {
    /// Length in days; permanent without it.
    pub days: Option<u32>,
    pub reason: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CommandRequest {
    /// A test command as typed in chat, e.g. `!fx 163540`, `!skin 2212`, `!state 29`.
    pub command: String,
}

/// Achievements to take away: `ids`, or with `all` every one.
#[derive(Deserialize, ToSchema, Default)]
pub struct RemoveAchievementsRequest {
    /// Achievement ids (tags) to remove; the earned "all of" achievements above them go too.
    #[serde(default)]
    pub ids: Vec<u32>,
    /// Remove every achievement, and the journal (finds, defeats, visits) and achievement
    /// counters, so they can be earned again. Story and tutorial progress (tags that aren't
    /// achievements), stars and XP stay.
    #[serde(default)]
    pub all: bool,
}

#[derive(Serialize, ToSchema)]
pub struct RemovedAchievements {
    /// The achievement ids removed.
    pub removed: Vec<u32>,
    /// The character was online: its client was updated at once (counters show again after
    /// its next login).
    pub online: bool,
}

#[derive(Deserialize, ToSchema, Default)]
pub struct KickRequest {
    /// Shown to the player in chat.
    pub reason: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct BroadcastRequest {
    /// 1-500 characters, shown in every player's chat from "Server".
    #[schema(min_length = 1, max_length = 500)]
    pub message: String,
}

#[derive(Deserialize, ToSchema)]
pub struct MaintenanceRequest {
    pub enabled: bool,
    pub message: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Done {
    /// How many players were affected.
    pub affected: u32,
}

#[derive(Serialize, ToSchema)]
pub struct CommandResult {
    /// How many players it reached (0: not online).
    pub affected: u32,
    /// What the server answered in the player's chat while running it (e.g. `effect package
    /// 1748 (12 of 2518)`, a usage line, `Test commands need a GM level.`). Lines that come
    /// later (after a teleport, during a dialogue) aren't here.
    pub replies: Vec<String>,
}

/// What the test commands accept, for tools that offer them as choices (the game server's
/// devcmd.rs). Targets and spawnable names come from the world as loaded; their numbers are
/// what `!goto <kind> <n>`, `!test <n>` and `!dialog <n>` take.
#[derive(Serialize, ToSchema)]
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
    /// `!spawn [count] <name>`: enemies and smashables
    pub spawnable: Vec<Spawnable>,
    /// `!skin <monster data>`: the looks of the world's characters and smashables
    pub looks: Vec<Look>,
}

#[derive(Serialize, ToSchema)]
pub struct TestCommand {
    /// e.g. `!goto <kind> [n]`
    pub usage: String,
    pub about: String,
    /// The GM level the command needs (alpha testers have some of them).
    pub gm_level: u32,
}

#[derive(Serialize, ToSchema)]
pub struct TargetKind {
    pub kind: String,
    pub about: String,
    pub targets: Vec<Target>,
}

#[derive(Serialize, ToSchema)]
pub struct Target {
    /// For `!goto <kind> <n>` (from 1).
    pub n: u32,
    pub label: String,
    pub playfield: u32,
    /// x, y, z
    pub pos: [f32; 3],
}

#[derive(Serialize, ToSchema)]
pub struct TestScenario {
    /// For `!test <n>` (from 1).
    pub n: u32,
    pub label: String,
    pub about: String,
    /// Minifigure names of the team it sets, the first active; empty: the team stays.
    pub team: Vec<String>,
    pub spawn: Vec<SpawnCount>,
}

#[derive(Serialize, ToSchema)]
pub struct SpawnCount {
    pub name: String,
    pub count: u32,
}

#[derive(Serialize, ToSchema)]
pub struct TestDialogue {
    /// For `!dialog <n>` (from 1).
    pub n: u32,
    pub npc: String,
    pub playfield: u32,
}

#[derive(Serialize, ToSchema)]
pub struct Spawnable {
    pub name: String,
    /// `enemy` or `smashable`
    pub kind: String,
    pub level: u32,
    pub playfields: Vec<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct Look {
    pub monster_data: u32,
    /// Names of the characters with this look.
    pub names: Vec<String>,
    /// `enemy`, `smashable`, `npc` or `pet`
    pub kind: String,
}

impl From<lmoxide_control::TestCatalog> for TestCatalog {
    fn from(c: lmoxide_control::TestCatalog) -> Self {
        Self {
            commands: c.commands.into_iter().map(|c| TestCommand { usage: c.usage, about: c.about, gm_level: c.gm_level }).collect(),
            kinds: c
                .kinds
                .into_iter()
                .map(|k| TargetKind {
                    kind: k.kind,
                    about: k.about,
                    targets: k.targets.into_iter().map(|t| Target { n: t.n, label: t.label, playfield: t.playfield, pos: t.pos }).collect(),
                })
                .collect(),
            scenarios: c
                .scenarios
                .into_iter()
                .map(|s| TestScenario {
                    n: s.n,
                    label: s.label,
                    about: s.about,
                    team: s.team,
                    spawn: s.spawn.into_iter().map(|sp| SpawnCount { name: sp.name, count: sp.count }).collect(),
                })
                .collect(),
            dialogues: c.dialogues.into_iter().map(|d| TestDialogue { n: d.n, npc: d.npc, playfield: d.playfield }).collect(),
            loot_kinds: c.loot_kinds,
            spawnable: c.spawnable.into_iter().map(|s| Spawnable { name: s.name, kind: s.kind, level: s.level, playfields: s.playfields }).collect(),
            looks: c.looks.into_iter().map(|l| Look { monster_data: l.monster_data, names: l.names, kind: l.kind }).collect(),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    pub scope: ApiScope,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked: bool,
    /// Set: the key acts as this account, and its scope follows the account's GM level as a
    /// login session's does (`scope` is then `read` and unused).
    pub account_id: Option<u32>,
}

#[derive(Deserialize, ToSchema)]
pub struct NewApiKey {
    pub name: String,
    pub scope: ApiScope,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedApiKey {
    pub key: ApiKey,
    /// The secret, shown only now: send as `Authorization: Bearer <secret>`.
    pub secret: String,
}

#[derive(Serialize, ToSchema)]
pub struct AuditEntry {
    pub id: i64,
    pub at: i64,
    pub actor: String,
    pub action: String,
    /// What the action was about; for accounts, the username at the time.
    pub target: Option<String>,
    /// For actions on an account: its id (the account may since have been deleted).
    pub target_account_id: Option<u32>,
    pub details: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct AccountQuery {
    /// Only usernames containing this (case-insensitive).
    pub search: Option<String>,
    #[serde(default)]
    pub offset: u32,
    /// At most 200 (default 50).
    pub limit: Option<u32>,
}

#[derive(Deserialize, IntoParams)]
pub struct AuditQuery {
    /// At most 500 (default 100).
    pub limit: Option<u32>,
    /// Entries older than this id (paging).
    pub before: Option<i64>,
}

pub(crate) fn api_key(k: lmoxide_db::ApiKey) -> ApiKey {
    ApiKey {
        id: k.id,
        name: k.name,
        scope: k.scope.into(),
        created_at: k.created_at,
        last_used_at: k.last_used_at,
        revoked: k.revoked,
        account_id: k.account_id,
    }
}

pub(crate) fn account_dto(db: &lmoxide_db::Db, a: lmoxide_db::Account, show_ip: bool) -> lmoxide_db::Result<Account> {
    let characters = db
        .characters_of(a.id)?
        .into_iter()
        .map(|c| CharacterSummary { id: c.id, name: c.name, mode: c.mode, playfield: c.playfield, stars: c.stars, saved_at: c.saved_at })
        .collect();
    let ban = a.active_ban().map(|b| BanInfo { until: b.until, reason: b.reason.clone() });
    Ok(Account {
        id: a.id,
        username: a.username,
        has_password: a.has_password,
        gm_level: a.gm_level,
        created_at: a.created_at,
        last_login_at: a.last_login_at,
        last_login_ip: if show_ip { a.last_login_ip } else { None },
        ban,
        discord_id: a.discord_id,
        characters,
    })
}

/// Largest survey request (an imported set).
pub const SURVEY_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// `/v1/survey/{*path}`: the world editor's routes (docs/SURVEY.md "API": placing, selecting,
/// hiding, notes, scripts, import and export), passed on to the game server as they are, with
/// the caller as the author of what they change. Accounts from `--editor-gm-level` (the
/// contributors, as the game's world editor mode) and moderate-scope keys. Not in the OpenAPI
/// document (a pass-through; the shapes are lmoxide-survey's).
pub async fn survey(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    auth: Auth,
    method: axum::http::Method,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    auth.require_editor(&state)?;
    // the path as sent: ids with slashes in them come percent-encoded (`spawn%2F4200%2F...`)
    let path = uri.path().strip_prefix("/v1/survey/").unwrap_or_default();
    let target = match uri.query() {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    };
    let (status, content_type, body) = control(&state)?.survey(method.as_str(), &target, &auth.actor, body.to_vec()).await.map_err(ApiError::game_server)?;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp = (status, body).into_response();
    if let Some(ct) = content_type.and_then(|c| axum::http::HeaderValue::from_str(&c).ok()) {
        resp.headers_mut().insert(axum::http::header::CONTENT_TYPE, ct);
    }
    Ok(resp)
}

/// `/v1/editor/{*path}`: a survey server's content editing routes (docs/WORLD-EDITOR-CONTENT.md:
/// records, schemas, catalogs, changes, history), passed on like [`survey`] with the caller as
/// the author. As [`survey`]: contributors and up.
pub async fn editor(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    auth: Auth,
    method: axum::http::Method,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    auth.require_editor(&state)?;
    // the path as sent: ids with slashes in them come percent-encoded (`spawn%2F4200%2F...`)
    let path = uri.path().strip_prefix("/v1/editor/").unwrap_or_default();
    let target = match uri.query() {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    };
    let (status, content_type, body) = control(&state)?.editor(method.as_str(), &target, &auth.actor, body.to_vec()).await.map_err(ApiError::game_server)?;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp = (status, body).into_response();
    if let Some(ct) = content_type.and_then(|c| axum::http::HeaderValue::from_str(&c).ok()) {
        resp.headers_mut().insert(axum::http::header::CONTENT_TYPE, ct);
    }
    Ok(resp)
}

/// A play server's world content (docs/WORLD-CONTENT.md: the approval queue, rules, summaries),
/// forwarded to its control interface like [`survey`]. Admin only.
pub async fn content(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    auth: Auth,
    method: axum::http::Method,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    auth.require(Scope::Admin)?;
    // the path as sent: ids with slashes in them come percent-encoded (`spawn%2F4200%2F...`)
    let path = uri.path().strip_prefix("/v1/content/").unwrap_or_default();
    let target = match uri.query() {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    };
    let (status, content_type, body) = control(&state)?.content(method.as_str(), &target, &auth.actor, body.to_vec()).await.map_err(ApiError::game_server)?;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp = (status, body).into_response();
    if let Some(ct) = content_type.and_then(|c| axum::http::HeaderValue::from_str(&c).ok()) {
        resp.headers_mut().insert(axum::http::header::CONTENT_TYPE, ct);
    }
    Ok(resp)
}

fn control(state: &AppState) -> ApiResult<&lmoxide_control::Client> {
    state.control.as_ref().ok_or_else(|| {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "no_control", "no game server control interface configured")
    })
}

async fn audit(state: &Arc<AppState>, auth: &Auth, action: &'static str, target: Option<String>, details: Option<String>) {
    let actor = auth.actor.clone();
    let r = blocking(state, move |db| Ok(db.audit(&actor, action, target.as_deref(), details.as_deref())?)).await;
    if let Err(e) = r {
        tracing::error!("audit log: {}", e.message);
    }
}

/// [`audit`] for an action on account `id` (by username and id; just the id if it doesn't exist).
async fn audit_account(state: &Arc<AppState>, auth: &Auth, action: &'static str, id: u32, details: Option<String>) {
    let actor = auth.actor.clone();
    let r = blocking(state, move |db| {
        match db.account(id)? {
            Some(a) => db.audit_account(&actor, action, &a, details.as_deref())?,
            None => db.audit(&actor, action, Some(&id.to_string()), details.as_deref())?,
        }
        Ok(())
    })
    .await;
    if let Err(e) = r {
        tracing::error!("audit log: {}", e.message);
    }
}

// ------------------------------------------------------------------ public

/// Server status.
///
/// Public unless the API runs with `--private-status`.
#[utoipa::path(get, path = "/v1/status", tag = "public",
    responses((status = OK, body = Status)), security((), ("bearer" = [])))]
async fn status(State(state): State<Arc<AppState>>, MaybeAuth(auth): MaybeAuth) -> ApiResult<Json<Status>> {
    if state.cfg.private_status {
        auth.ok_or_else(ApiError::unauthorized)?.require(Scope::Read)?;
    }
    let s = match &state.control {
        Some(c) => c.status().await.ok(),
        None => None,
    };
    Ok(Json(Status {
        game_server_online: s.is_some(),
        version: s.as_ref().map(|s| s.version.clone()),
        started_at: s.as_ref().map(|s| s.started_at),
        players_online: s.as_ref().map(|s| s.players_online),
        maintenance: s.as_ref().map(|s| s.maintenance),
        maintenance_message: s.and_then(|s| s.maintenance_message),
    }))
}

/// Characters in play, by in-game name.
///
/// Public unless the API runs with `--private-status`.
#[utoipa::path(get, path = "/v1/online", tag = "public",
    responses((status = OK, body = [PublicPlayer]), (status = BAD_GATEWAY, body = ErrorBody)),
    security((), ("bearer" = [])))]
async fn online_players(State(state): State<Arc<AppState>>, MaybeAuth(auth): MaybeAuth) -> ApiResult<Json<Vec<PublicPlayer>>> {
    if state.cfg.private_status {
        auth.ok_or_else(ApiError::unauthorized)?.require(Scope::Read)?;
    }
    let players = control(&state)?.players().await.map_err(ApiError::game_server)?;
    Ok(Json(players.into_iter().map(|p| PublicPlayer { character_name: p.character_name, playfield: p.playfield }).collect()))
}

/// Who may create game accounts (`--registration`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    /// Anyone, with `POST /v1/auth/register`; the Discord bot too.
    Open,
    /// Only the Discord bot (`POST /v1/discord/accounts`), for members of its server.
    Discord,
    /// Only admins.
    Closed,
}

/// How this API is set up, for clients that adapt to it (always public).
#[derive(Serialize, ToSchema)]
pub struct ServerConfig {
    /// Whether `POST /v1/auth/register` accepts new accounts (`registration` is `open`).
    pub registration_open: bool,
    /// Who may create accounts (`--registration`).
    pub registration: Registration,
    /// Whether `/v1/status` and `/v1/online` need a read-scope credential (`--private-status`).
    pub private_status: bool,
    /// GM level from which an account's login session has the moderate scope.
    pub moderator_gm_level: u32,
    /// GM level from which an account's login session has the admin scope.
    pub admin_gm_level: u32,
    /// GM level from which an account may use the world editor (`/v1/survey`, `/v1/editor`).
    pub editor_gm_level: u32,
}

/// How this API is set up: registration, GM level thresholds.
#[utoipa::path(get, path = "/v1/config", tag = "public",
    responses((status = OK, body = ServerConfig)), security(()))]
async fn config(State(state): State<Arc<AppState>>) -> Json<ServerConfig> {
    let cfg = &state.cfg;
    Json(ServerConfig {
        registration_open: cfg.registration() == Registration::Open,
        registration: cfg.registration(),
        private_status: cfg.private_status,
        moderator_gm_level: cfg.moderator_gm_level,
        admin_gm_level: cfg.admin_gm_level,
        editor_gm_level: cfg.editor_gm_level,
    })
}

/// `Cache-Control` for answers built from the game data, which only change with a new build.
const STATIC_DATA: [(header::HeaderName, &str); 1] = [(header::CACHE_CONTROL, "public, max-age=3600")];

#[derive(Serialize, ToSchema)]
pub struct Playfield {
    pub id: u32,
    pub name: String,
    /// A `!tp` destination (with an arrival point); the others are the tutorial, layers, test maps.
    pub teleport: bool,
}

/// Playfield ids in the other answers, with their names (worlds, dungeons, the LEGO Center).
#[utoipa::path(get, path = "/v1/playfields", tag = "public",
    responses((status = OK, body = [Playfield])), security(()))]
async fn playfields() -> impl axum::response::IntoResponse {
    (STATIC_DATA, Json(playfield_list().iter().map(|p| Playfield { id: p.id, name: p.name.clone(), teleport: p.teleport }).collect::<Vec<_>>()))
}

/// The worlds of the content tree: teleport destinations by their own (player-facing) display
/// name, the rest by the client's playfield name.
fn playfield_list() -> &'static [Playfield] {
    static LIST: std::sync::LazyLock<Vec<Playfield>> = std::sync::LazyLock::new(|| {
        let mut all: Vec<Playfield> = game_content()
            .worlds
            .values()
            .map(|w| {
                let teleport = w.arrival.is_some();
                let name = if teleport { w.display_name.as_ref().unwrap_or(&w.name) } else { &w.name };
                Playfield { id: w.playfield, name: decode_entities(name), teleport }
            })
            .collect();
        all.sort_by_key(|p| p.id);
        all
    });
    &LIST
}

/// Decodes the XML character entities the game data keeps in names (`Natives&apos; Hollow`).
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&end| end <= 10).and_then(|end| {
            let c = match &rest[1..end] {
                "amp" => '&',
                "apos" => '\'',
                "quot" => '"',
                "lt" => '<',
                "gt" => '>',
                e => match e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")) {
                    Some(hex) => char::from_u32(u32::from_str_radix(hex, 16).ok()?)?,
                    None => char::from_u32(e.strip_prefix('#')?.parse().ok()?)?,
                },
            };
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Serialize, ToSchema)]
pub struct Minifigure {
    /// Item template id (as in `Character.team` and `Character.upgrades`).
    pub id: u32,
    pub name: String,
    /// Collectible series.
    pub series: u32,
    /// Name of the normal (left-click) attack.
    pub normal_attack: Option<String>,
    /// Name of the special (right-click) attack.
    pub special_attack: Option<String>,
    /// Upgrades by bit of `Character.upgrades`.
    pub upgrades: Vec<MinifigureUpgrade>,
    /// The minifigure's parts (tokens in `Character.tokens`).
    pub parts: Vec<MinifigurePart>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct MinifigureUpgrade {
    pub bit: u32,
    /// e.g. "Fast", "Lionhearted".
    pub name: String,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct MinifigurePart {
    /// Token id.
    pub token: u32,
    /// Head, Torso or Legs.
    pub location: String,
    pub name: String,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct Token {
    pub id: u32,
    pub name: String,
    /// The minifigure this is a part of.
    pub minifigure: u32,
    /// Head, Torso or Legs.
    pub location: String,
}

/// The game's content tree (docs/CONTENT.md): `LMOXIDE_CONTENT_DIR`, else the checkout's.
fn game_content() -> &'static lmoxide_content::Content {
    static CONTENT: std::sync::LazyLock<lmoxide_content::Content> = std::sync::LazyLock::new(|| {
        let dir = std::env::var_os("LMOXIDE_CONTENT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content")).to_path_buf());
        let options = lmoxide_content::Options { check_client_ids: false, ..Default::default() };
        let (content, problems) = lmoxide_content::load(&dir, &options);
        for p in problems.iter().filter(|p| p.severity == lmoxide_content::Severity::Error).take(20) {
            tracing::error!("content: {p}");
        }
        content
    });
    &CONTENT
}

/// The game's minifigures (content/minifigures, their upgrades' buffs and parts' items).
fn minifigure_list() -> &'static [Minifigure] {
    static LIST: std::sync::LazyLock<Vec<Minifigure>> = std::sync::LazyLock::new(|| {
        let c = game_content();
        let mut list: Vec<Minifigure> = c
            .minifigures
            .values()
            .map(|m| {
                let attack = |a: &lmoxide_content::Attack| (!a.name.is_empty()).then(|| decode_entities(&a.name));
                Minifigure {
                    id: m.template,
                    name: decode_entities(&m.name),
                    series: m.series,
                    normal_attack: attack(&m.normal),
                    special_attack: attack(&m.special),
                    upgrades: m
                        .upgrades
                        .iter()
                        .filter_map(|u| Some((u.bit, &c.buffs.get(&u.buff)?.name)))
                        .filter(|(_, name)| name.as_str() != "No upgrade available")
                        .map(|(bit, name)| MinifigureUpgrade { bit, name: decode_entities(name.strip_prefix("Upgrade: ").unwrap_or(name)) })
                        .collect(),
                    parts: m
                        .parts
                        .iter()
                        .filter_map(|p| {
                            let item = c.items.get(&p.item)?;
                            Some(MinifigurePart { token: item.tokens.first()?.token, location: p.location.clone(), name: decode_entities(&item.name) })
                        })
                        .collect(),
                }
            })
            .collect();
        list.sort_by_key(|m| m.id);
        list
    });
    &LIST
}

/// Every minifigure: name, attacks, upgrade names by bit, parts.
#[utoipa::path(get, path = "/v1/minifigures", tag = "public",
    responses((status = OK, body = [Minifigure])), security(()))]
async fn minifigures() -> impl axum::response::IntoResponse {
    (STATIC_DATA, Json(minifigure_list()))
}

/// Token ids (minifigure parts, as in `Character.tokens`) with their names.
#[utoipa::path(get, path = "/v1/tokens", tag = "public",
    responses((status = OK, body = [Token])), security(()))]
async fn tokens() -> impl axum::response::IntoResponse {
    static LIST: std::sync::LazyLock<Vec<Token>> = std::sync::LazyLock::new(|| {
        let mut list: Vec<Token> = minifigure_list()
            .iter()
            .flat_map(|m| {
                m.parts.iter().map(|p| Token { id: p.token, name: p.name.clone(), minifigure: m.id, location: p.location.clone() })
            })
            .collect();
        list.sort_by_key(|t| t.id);
        list
    });
    (STATIC_DATA, Json(LIST.as_slice()))
}

// ------------------------------------------------------------------ sessions

/// Log in with a game account; returns a session token.
#[utoipa::path(post, path = "/v1/auth/login", tag = "auth", request_body = Login,
    responses((status = OK, body = Session), (status = UNAUTHORIZED, body = ErrorBody), (status = TOO_MANY_REQUESTS, body = ErrorBody)),
    security(()))]
async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(c): Json<Login>,
) -> ApiResult<Json<Session>> {
    let ip_key = format!("ip:{}", client_ip(&state, &headers, peer));
    let user_key = format!("user:{}", c.username.to_ascii_lowercase());
    if !state.lockout.allowed(&ip_key) || !state.lockout.allowed(&user_key) {
        return Err(ApiError::too_many());
    }
    let st = state.clone();
    let ttl = session_ttl(&state, c.remember);
    let result = blocking(&state, move |db| {
        let Some(a) = db.account_by_name(&c.username)? else { return Ok(None) };
        if !db.verify_password(a.id, &c.password)? || a.active_ban().is_some() {
            return Ok(None);
        }
        db.audit(&format!("account:{}", a.username), "api-login", None, c.remember.then_some("remember"))?;
        Ok(Some(new_session(&st, db, a, ttl, false)?))
    })
    .await?;
    match result {
        Some(s) => {
            state.lockout.succeeded(&user_key);
            Ok(Json(s))
        }
        None => {
            state.lockout.failed(&ip_key);
            state.lockout.failed(&user_key);
            Err(ApiError::new(StatusCode::UNAUTHORIZED, "bad_credentials", "wrong username or password"))
        }
    }
}

pub(crate) fn session_ttl(state: &AppState, remember: bool) -> i64 {
    if remember { state.cfg.remember_days as i64 * 86400 } else { state.cfg.session_ttl_hours as i64 * 3600 }
}

pub(crate) fn new_session(
    state: &AppState,
    db: &lmoxide_db::Db,
    a: lmoxide_db::Account,
    ttl: i64,
    password_reset: bool,
) -> ApiResult<Session> {
    let token = db.create_session_with(a.id, ttl, password_reset)?;
    let scope = account_scope(state, a.gm_level).map(ApiScope::from);
    let show_ip = scope.is_some_and(|s| matches!(s, ApiScope::Admin));
    Ok(Session { token, expires_at: lmoxide_db::now() + ttl, scope, account: account_dto(db, a, show_ip)?, password_reset })
}

/// Trade the current session for a new one (a new token and expiry; the old token stops
/// working), so a remembered sign-in lasts while it is used. Banned accounts can't renew.
#[utoipa::path(post, path = "/v1/auth/renew", tag = "auth", request_body(content = Renew, description = "optional"),
    responses((status = OK, body = Session), (status = UNAUTHORIZED, body = ErrorBody)))]
async fn renew(State(state): State<Arc<AppState>>, auth: Auth, body: Option<Json<Renew>>) -> ApiResult<Json<Session>> {
    let (Some(id), Some(old)) = (auth.account, auth.token) else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "not_a_session", "only login sessions can be renewed"));
    };
    let ttl = session_ttl(&state, body.map(|Json(r)| r.remember).unwrap_or_default());
    let st = state.clone();
    blocking(&state, move |db| {
        let a = db.account(id)?.filter(|a| a.active_ban().is_none()).ok_or_else(ApiError::unauthorized)?;
        let s = new_session(&st, db, a, ttl, false)?;
        db.delete_session(&old)?;
        Ok(Json(s))
    })
    .await
}

/// End the current session.
#[utoipa::path(post, path = "/v1/auth/logout", tag = "auth", responses((status = NO_CONTENT)))]
async fn logout(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<StatusCode> {
    if let Some(token) = auth.token {
        blocking(&state, move |db| Ok(db.delete_session(&token)?)).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Create a game account (only when `registration` is `open`).
#[utoipa::path(post, path = "/v1/auth/register", tag = "auth", request_body = Credentials,
    responses((status = CREATED, body = Account), (status = FORBIDDEN, body = ErrorBody), (status = CONFLICT, body = ErrorBody)),
    security(()))]
async fn register(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(c): Json<Credentials>,
) -> ApiResult<(StatusCode, Json<Account>)> {
    if state.cfg.registration() != Registration::Open {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "registration_closed", "registration is closed"));
    }
    // registrations count as failures so one address can't create accounts in bulk
    let ip_key = format!("register:{}", client_ip(&state, &headers, peer));
    if !state.lockout.allowed(&ip_key) {
        return Err(ApiError::too_many());
    }
    state.lockout.failed(&ip_key);
    let account = blocking(&state, move |db| {
        let a = db.create_account(&c.username, Some(&c.password), 0)?;
        db.audit_account("api", "register", &a, None)?;
        Ok(account_dto(db, a, false)?)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(account)))
}

/// The logged-in account.
#[utoipa::path(get, path = "/v1/me", tag = "auth",
    responses((status = OK, body = Account), (status = NOT_FOUND, body = ErrorBody)))]
async fn me(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Account>> {
    let id = auth.account.ok_or_else(|| ApiError::not_found("account (API keys have none)"))?;
    blocking(&state, move |db| {
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        Ok(Json(account_dto(db, a, true)?))
    })
    .await
}

/// Change the logged-in account's password (ends its sessions). `current_password` may be left
/// out in a `password_reset` session (from `/v1/auth/code`).
#[utoipa::path(post, path = "/v1/me/password", tag = "auth", request_body = ChangePassword,
    responses((status = NO_CONTENT), (status = UNAUTHORIZED, body = ErrorBody)))]
async fn change_own_password(State(state): State<Arc<AppState>>, auth: Auth, Json(c): Json<ChangePassword>) -> ApiResult<StatusCode> {
    let id = auth.account.ok_or_else(|| ApiError::not_found("account (API keys have none)"))?;
    blocking(&state, move |db| {
        let ok = match &c.current_password {
            Some(p) => db.verify_password(id, p)?,
            None => auth.password_reset,
        };
        if !ok {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "bad_credentials", "wrong current password"));
        }
        db.set_password(id, Some(&c.new_password))?;
        db.end_game_sessions(id)?;
        db.audit(&auth.actor, "change-own-password", None, None)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

// ------------------------------------------------------------------ live server

/// Everyone logged in, with account details. Scope: read.
#[utoipa::path(get, path = "/v1/players", tag = "players",
    responses((status = OK, body = [Player]), (status = BAD_GATEWAY, body = ErrorBody)))]
async fn players(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<Player>>> {
    auth.require(Scope::Read)?;
    let players = control(&state)?.players().await.map_err(ApiError::game_server)?;
    Ok(Json(players.into_iter().map(Player::from).collect()))
}

/// The logged-in account's characters online (the dev UI's debugger for players without the
/// read scope). Any account.
#[utoipa::path(get, path = "/v1/me/players", tag = "auth",
    responses((status = OK, body = [Player]), (status = BAD_GATEWAY, body = ErrorBody)))]
async fn my_players(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<Player>>> {
    let id = auth.account.ok_or_else(|| ApiError::not_found("account (API keys have none)"))?;
    let players = control(&state)?.players().await.map_err(ApiError::game_server)?;
    Ok(Json(players.into_iter().filter(|p| p.account_id == id).map(Player::from).collect()))
}

/// Run a test command as one's own character online, as if typed in chat: the game server
/// decides by the account's GM level which commands run (`GET /v1/catalog/test-commands`
/// `gm_level`; alpha testers have teleporting, scenarios, minifigures, dialogues, achievements,
/// animations and looks). Any account.
#[utoipa::path(post, path = "/v1/me/command", tag = "auth", request_body = CommandRequest,
    responses((status = OK, body = CommandResult), (status = FORBIDDEN, body = ErrorBody)))]
async fn my_command(State(state): State<Arc<AppState>>, auth: Auth, Json(c): Json<CommandRequest>) -> ApiResult<Json<CommandResult>> {
    let id = auth.account.ok_or_else(|| ApiError::not_found("account (API keys have none)"))?;
    if !(c.command.starts_with('!') || c.command.starts_with('/')) || c.command.len() > 500 {
        return Err(ApiError::invalid("command must start with ! or / (at most 500 characters)"));
    }
    let done = control(&state)?.command(id, &c.command).await.map_err(ApiError::game_server)?;
    audit_account(&state, &auth, "command", id, Some(c.command)).await;
    Ok(Json(CommandResult { affected: done.affected, replies: done.replies }))
}

/// The running copies of worlds (game modes' main copies, team copies, previews) with their
/// players and NPCs, and the AI's tick time. Scope: moderate.
#[utoipa::path(get, path = "/v1/instances", tag = "players",
    responses((status = OK, body = Instances), (status = BAD_GATEWAY, body = ErrorBody)))]
async fn instances(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Instances>> {
    auth.require(Scope::Moderate)?;
    let i = control(&state)?.instances().await.map_err(ApiError::game_server)?;
    Ok(Json(Instances {
        instances: i
            .instances
            .into_iter()
            .map(|c| Instance {
                playfield: c.playfield,
                copy: c.copy,
                key: c.key,
                mode: c.mode,
                kind: c.kind,
                preview_owner: c.preview_owner,
                characters: c.characters,
                players: c.players,
                npcs: c.npcs,
            })
            .collect(),
        ai_tick_ms: i.ai_tick_ms,
        ai_tick_max_ms: i.ai_tick_max_ms,
        open_world_cap: i.open_world_cap,
    }))
}

/// Disconnect a player. Scope: moderate.
#[utoipa::path(post, path = "/v1/players/{account_id}/kick", tag = "players", request_body = KickRequest,
    params(("account_id" = u32, Path)), responses((status = OK, body = Done)))]
async fn kick(State(state): State<Arc<AppState>>, auth: Auth, Path(account_id): Path<u32>, Json(k): Json<KickRequest>) -> ApiResult<Json<Done>> {
    auth.require(Scope::Moderate)?;
    protect_staff(&state, &auth, account_id).await?;
    let done = control(&state)?
        .kick(account_id, &lmoxide_control::Kick { reason: k.reason.clone() })
        .await
        .map_err(ApiError::game_server)?;
    audit_account(&state, &auth, "kick", account_id, k.reason).await;
    Ok(Json(Done { affected: done.affected }))
}

/// Run a test command (the game server's devcmd.rs: `!fx`, `!skin`, `!state`, `!tp`, ...) as the
/// player, as if typed in chat; the player's GM level still decides whether it runs. For tools
/// such as an effect browser that preview things in the game client. Scope: admin.
#[utoipa::path(post, path = "/v1/players/{account_id}/command", tag = "players", request_body = CommandRequest,
    params(("account_id" = u32, Path)), responses((status = OK, body = CommandResult)))]
async fn player_command(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(account_id): Path<u32>,
    Json(c): Json<CommandRequest>,
) -> ApiResult<Json<CommandResult>> {
    auth.require(Scope::Admin)?;
    if !(c.command.starts_with('!') || c.command.starts_with('/')) || c.command.len() > 500 {
        return Err(ApiError::invalid("command must start with ! or / (at most 500 characters)"));
    }
    let done = control(&state)?.command(account_id, &c.command).await.map_err(ApiError::game_server)?;
    audit_account(&state, &auth, "command", account_id, Some(c.command)).await;
    Ok(Json(CommandResult { affected: done.affected, replies: done.replies }))
}

/// Take achievements away from a character (to earn them again, or to fix a mistake): some by
/// id, or all of them. A removed exploration or collectible achievement's finds go too. Through
/// the game server: at once in game when the character is online, else in the database.
/// Scope: admin.
#[utoipa::path(post, path = "/v1/characters/{id}/achievements/remove", tag = "accounts", request_body = RemoveAchievementsRequest,
    params(("id" = u32, Path)), responses((status = OK, body = RemovedAchievements), (status = NOT_FOUND, body = ErrorBody)))]
async fn remove_achievements(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<u32>,
    Json(r): Json<RemoveAchievementsRequest>,
) -> ApiResult<Json<RemovedAchievements>> {
    auth.require(Scope::Admin)?;
    if !r.all && r.ids.is_empty() {
        return Err(ApiError::invalid("name the achievements (ids) or set all"));
    }
    if blocking(&state, move |db| Ok(db.load_character(id)?.is_none())).await? {
        return Err(ApiError::not_found("character"));
    }
    let req = lmoxide_control::RemoveAchievements { ids: r.ids, all: r.all };
    let done = control(&state)?.remove_achievements(id, &req).await.map_err(ApiError::game_server)?;
    let details = format!("character {id}: {} ({})", if req.all { "all".into() } else { format!("{:?}", done.removed) }, done.removed.len());
    let actor = auth.actor.clone();
    let _ = blocking(&state, move |db| Ok(db.audit(&actor, "achievements-remove", Some(&id.to_string()), Some(&details))?)).await;
    Ok(Json(RemovedAchievements { removed: done.removed, online: done.online }))
}

/// What the test commands accept: commands with usage, target kinds with their numbered targets,
/// test scenarios, dialogues, loot kinds, spawnable enemies and smashables, and monster data
/// looks (the game server's devcmd.rs). For tools that offer them as choices. Any account (and
/// read-scope keys).
#[utoipa::path(get, path = "/v1/catalog/test-commands", tag = "catalog", responses((status = OK, body = TestCatalog)))]
async fn test_commands(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<TestCatalog>> {
    if auth.account.is_none() {
        auth.require(Scope::Read)?;
    }
    let catalog = control(&state)?.test_catalog().await.map_err(ApiError::game_server)?;
    Ok(Json(catalog.into()))
}

/// Send a chat line from "Server" to everyone online. Scope: moderate.
#[utoipa::path(post, path = "/v1/broadcast", tag = "players", request_body = BroadcastRequest,
    responses((status = OK, body = Done)))]
async fn broadcast(State(state): State<Arc<AppState>>, auth: Auth, Json(b): Json<BroadcastRequest>) -> ApiResult<Json<Done>> {
    auth.require(Scope::Moderate)?;
    let message = b.message.trim().to_string();
    if message.is_empty() || message.chars().count() > 500 {
        return Err(ApiError::invalid("messages are 1-500 characters"));
    }
    let done = control(&state)?.broadcast(&message).await.map_err(ApiError::game_server)?;
    audit(&state, &auth, "broadcast", None, Some(message)).await;
    Ok(Json(Done { affected: done.affected }))
}

/// Switch maintenance mode: while on, only accounts with a GM level can log in. Scope: admin.
#[utoipa::path(post, path = "/v1/maintenance", tag = "players", request_body = MaintenanceRequest,
    responses((status = OK, body = Status)))]
async fn maintenance(State(state): State<Arc<AppState>>, auth: Auth, Json(m): Json<MaintenanceRequest>) -> ApiResult<Json<Status>> {
    auth.require(Scope::Admin)?;
    let s = control(&state)?
        .maintenance(&lmoxide_control::Maintenance { enabled: m.enabled, message: m.message.clone() })
        .await
        .map_err(ApiError::game_server)?;
    audit(&state, &auth, if m.enabled { "maintenance-on" } else { "maintenance-off" }, None, m.message).await;
    Ok(Json(Status {
        game_server_online: true,
        version: Some(s.version),
        started_at: Some(s.started_at),
        players_online: Some(s.players_online),
        maintenance: Some(s.maintenance),
        maintenance_message: s.maintenance_message,
    }))
}

// ------------------------------------------------------------------ accounts

/// List accounts, by username. Scope: read.
#[utoipa::path(get, path = "/v1/accounts", tag = "accounts", params(AccountQuery),
    responses((status = OK, body = [Account],
        headers(("X-Total-Count" = u64, description = "How many accounts match the search in all")))))]
async fn list_accounts(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Query(q): Query<AccountQuery>,
) -> ApiResult<([(&'static str, HeaderValue); 1], Json<Vec<Account>>)> {
    auth.require(Scope::Read)?;
    let show_ip = auth.require(Scope::Admin).is_ok();
    blocking(&state, move |db| {
        let list = db.list_accounts(q.search.as_deref(), q.offset, q.limit.unwrap_or(50).min(200))?;
        let total = db.count_accounts_matching(q.search.as_deref())?;
        let list = list.into_iter().map(|a| account_dto(db, a, show_ip)).collect::<Result<_, _>>()?;
        Ok(([(TOTAL_COUNT, HeaderValue::from(total))], Json(list)))
    })
    .await
}

/// Response header with the size of a paged list.
pub const TOTAL_COUNT: &str = "x-total-count";

/// Create an account. Scope: admin.
#[utoipa::path(post, path = "/v1/accounts", tag = "accounts", request_body = NewAccount,
    responses((status = CREATED, body = Account), (status = CONFLICT, body = ErrorBody)))]
async fn create_account(State(state): State<Arc<AppState>>, auth: Auth, Json(n): Json<NewAccount>) -> ApiResult<(StatusCode, Json<Account>)> {
    auth.require(Scope::Admin)?;
    let account = blocking(&state, move |db| {
        let a = db.create_account(&n.username, n.password.as_deref(), n.gm_level)?;
        db.audit_account(&auth.actor, "account-create", &a, None)?;
        Ok(account_dto(db, a, true)?)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(account)))
}

/// One account with its characters. Scope: read.
#[utoipa::path(get, path = "/v1/accounts/{id}", tag = "accounts", params(("id" = u32, Path)),
    responses((status = OK, body = Account), (status = NOT_FOUND, body = ErrorBody)))]
async fn get_account(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<u32>) -> ApiResult<Json<Account>> {
    auth.require(Scope::Read)?;
    let show_ip = auth.require(Scope::Admin).is_ok();
    blocking(&state, move |db| {
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        Ok(Json(account_dto(db, a, show_ip)?))
    })
    .await
}

/// Rename an account or change its GM level (from its next login). Scope: admin.
#[utoipa::path(patch, path = "/v1/accounts/{id}", tag = "accounts", params(("id" = u32, Path)),
    request_body = AccountChanges, responses((status = OK, body = Account)))]
async fn update_account(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<u32>, Json(c): Json<AccountChanges>) -> ApiResult<Json<Account>> {
    auth.require(Scope::Admin)?;
    blocking(&state, move |db| {
        let before = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        if let Some(name) = &c.username {
            db.rename_account(id, name)?;
            db.audit_account(&auth.actor, "account-rename", &before, Some(name))?;
        }
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        if let Some(level) = c.gm_level {
            db.set_gm_level(id, level)?;
            db.audit_account(&auth.actor, "account-gm", &a, Some(&level.to_string()))?;
        }
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        Ok(Json(account_dto(db, a, true)?))
    })
    .await
}

/// Delete an account with its character and friendships (kicks it first). Scope: admin.
#[utoipa::path(delete, path = "/v1/accounts/{id}", tag = "accounts", params(("id" = u32, Path)),
    responses((status = NO_CONTENT), (status = NOT_FOUND, body = ErrorBody)))]
async fn delete_account(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<u32>) -> ApiResult<StatusCode> {
    auth.require(Scope::Admin)?;
    kick_quietly(&state, id, "Your account was deleted.").await;
    blocking(&state, move |db| {
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        db.delete_account(id)?;
        db.audit_account(&auth.actor, "account-delete", &a, None)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Set or remove an account's password (ends its sessions). Scope: admin.
#[utoipa::path(post, path = "/v1/accounts/{id}/password", tag = "accounts", params(("id" = u32, Path)),
    request_body = SetPassword, responses((status = NO_CONTENT)))]
async fn set_password(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<u32>, Json(p): Json<SetPassword>) -> ApiResult<StatusCode> {
    auth.require(Scope::Admin)?;
    blocking(&state, move |db| {
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        db.set_password(id, p.password.as_deref())?;
        db.end_game_sessions(id)?;
        db.audit_account(&auth.actor, "account-passwd", &a, None)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Ban an account and disconnect it. Scope: moderate.
#[utoipa::path(post, path = "/v1/accounts/{id}/ban", tag = "accounts", params(("id" = u32, Path)),
    request_body = BanRequest, responses((status = OK, body = Account)))]
async fn ban(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<u32>, Json(b): Json<BanRequest>) -> ApiResult<Json<Account>> {
    auth.require(Scope::Moderate)?;
    protect_staff(&state, &auth, id).await?;
    let reason = b.reason.clone();
    let account = blocking(&state, move |db| {
        let until = b.days.map(|d| lmoxide_db::now() + i64::from(d) * 86400);
        db.ban(id, &Ban { until, reason: b.reason.clone() })?;
        db.end_game_sessions(id)?;
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        db.audit_account(&auth.actor, "account-ban", &a, b.reason.as_deref())?;
        Ok(account_dto(db, a, false)?)
    })
    .await?;
    let message = match reason {
        Some(r) => format!("You were banned: {r}"),
        None => "You were banned.".into(),
    };
    kick_quietly(&state, id, &message).await;
    Ok(Json(account))
}

/// Lift an account's ban. Scope: moderate.
#[utoipa::path(post, path = "/v1/accounts/{id}/unban", tag = "accounts", params(("id" = u32, Path)),
    responses((status = OK, body = Account)))]
async fn unban(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<u32>) -> ApiResult<Json<Account>> {
    auth.require(Scope::Moderate)?;
    blocking(&state, move |db| {
        db.unban(id)?;
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        db.audit_account(&auth.actor, "account-unban", &a, None)?;
        Ok(Json(account_dto(db, a, false)?))
    })
    .await
}

/// Moderators can't act on staff (accounts whose sessions have a scope): that takes admin.
async fn protect_staff(state: &Arc<AppState>, auth: &Auth, target: u32) -> ApiResult<()> {
    if auth.require(Scope::Admin).is_ok() {
        return Ok(());
    }
    let st = state.clone();
    let staff = blocking(state, move |db| Ok(db.account(target)?.is_some_and(|a| account_scope(&st, a.gm_level).is_some()))).await?;
    if staff { Err(ApiError::forbidden()) } else { Ok(()) }
}

/// Kicks an account if the game server is reachable (best effort: bans and deletions also
/// end its sessions in the database).
async fn kick_quietly(state: &AppState, account: u32, reason: &str) {
    if let Some(c) = &state.control
        && let Err(e) = c.kick(account, &lmoxide_control::Kick { reason: Some(reason.into()) }).await
    {
        tracing::warn!(account, "kick after account change failed: {e}");
    }
}

/// A saved character. Scope: read.
#[utoipa::path(get, path = "/v1/characters/{id}", tag = "accounts", params(("id" = u32, Path)),
    responses((status = OK, body = Character), (status = NOT_FOUND, body = ErrorBody)))]
async fn get_character(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<u32>) -> ApiResult<Json<Character>> {
    auth.require(Scope::Read)?;
    blocking(&state, move |db| {
        let c = db.load_character(id)?.ok_or_else(|| ApiError::not_found("character"))?;
        Ok(Json(Character {
            id,
            name: c.name,
            mode: c.mode,

            playfield: c.playfield,
            position: c.pos,
            team: c.team.iter().map(|&(_, template)| template).collect(),
            stars: c.stars,
            upgrades: c.upgrades,
            tokens: c.tokens.into_iter().map(|(t, (gained, spent))| (t, TokenCount { gained, spent })).collect(),
            xp: c.xp,
            level: c.level,
            minifigure_ranks: c.minifigure_levels,
            achievements: c
                .tags
                .into_iter()
                .map(|(id, achieved_at)| {
                    let a = achievement_info(id);
                    EarnedAchievement { id, name: a.name, title: a.title, group: a.group, text: a.text, points: a.points, achieved_at }
                })
                .collect(),
            counters: c.counters,
            defeats: c
                .defeats
                .into_iter()
                .map(|(n, d)| (n, DefeatCount { kind: d.kind, count: d.count, first_at: d.first_at, last_at: d.last_at }))
                .collect(),
            visits: c
                .visits
                .into_iter()
                .map(|(p, v)| (p, VisitCount { count: v.count, first_at: v.first_at, last_at: v.last_at }))
                .collect(),
            finds: c.finds,
        }))
    })
    .await
}

#[derive(Clone, Default)]
struct AchievementInfo {
    name: String,
    title: Option<String>,
    group: Option<String>,
    text: Option<String>,
    points: u32,
}

/// Names and points of an achievement tag (content/achievements; unknown: empty).
fn achievement_info(id: u32) -> AchievementInfo {
    static INFO: std::sync::LazyLock<std::collections::HashMap<u32, AchievementInfo>> = std::sync::LazyLock::new(|| {
        let doc = lmoxide_content::compat::achievements_file(game_content());
        let all = doc["achievements"].as_array().cloned().unwrap_or_default();
        let s = |v: &serde_json::Value| v.as_str().map(str::to_string);
        let titles: std::collections::HashMap<u64, String> = all.iter().filter_map(|a| Some((a["id"].as_u64()?, s(&a["title"])?))).collect();
        all.iter()
            .filter_map(|a| {
                let id = a["id"].as_u64()?;
                let info = AchievementInfo {
                    name: s(&a["name"])?,
                    title: s(&a["title"]),
                    group: a["parent"].as_u64().and_then(|p| titles.get(&p).cloned()),
                    text: s(&a["text"]),
                    points: a["points"].as_u64().unwrap_or(0) as u32,
                };
                Some((id as u32, info))
            })
            .collect()
    });
    INFO.get(&id).cloned().unwrap_or_default()
}

// ------------------------------------------------------------------ API keys, audit

/// List API keys. Scope: admin.
#[utoipa::path(get, path = "/v1/api-keys", tag = "admin", responses((status = OK, body = [ApiKey])))]
async fn list_api_keys(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<ApiKey>>> {
    auth.require(Scope::Admin)?;
    blocking(&state, |db| Ok(Json(db.list_api_keys()?.into_iter().map(api_key).collect()))).await
}

/// Create an API key (e.g. for a Discord bot); the secret is shown once. Scope: admin.
#[utoipa::path(post, path = "/v1/api-keys", tag = "admin", request_body = NewApiKey,
    responses((status = CREATED, body = CreatedApiKey)))]
async fn create_api_key(State(state): State<Arc<AppState>>, auth: Auth, Json(n): Json<NewApiKey>) -> ApiResult<(StatusCode, Json<CreatedApiKey>)> {
    auth.require(Scope::Admin)?;
    let created = blocking(&state, move |db| {
        let (key, secret) = db.create_api_key(&n.name, n.scope.into())?;
        db.audit(&auth.actor, "api-key-create", Some(&key.name), Some(key.scope.as_str()))?;
        Ok(CreatedApiKey { key: api_key(key), secret })
    })
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// Revoke an API key. Scope: admin.
#[utoipa::path(delete, path = "/v1/api-keys/{id}", tag = "admin", params(("id" = i64, Path)),
    responses((status = NO_CONTENT), (status = NOT_FOUND, body = ErrorBody)))]
async fn revoke_api_key(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    auth.require(Scope::Admin)?;
    blocking(&state, move |db| {
        db.revoke_api_key(id)?;
        db.audit(&auth.actor, "api-key-revoke", Some(&id.to_string()), None)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Administrative actions, newest first. Scope: admin.
#[utoipa::path(get, path = "/v1/audit", tag = "admin", params(AuditQuery),
    responses((status = OK, body = [AuditEntry])))]
async fn audit_log(State(state): State<Arc<AppState>>, auth: Auth, Query(q): Query<AuditQuery>) -> ApiResult<Json<Vec<AuditEntry>>> {
    auth.require(Scope::Admin)?;
    blocking(&state, move |db| {
        let entries = db.audit_log(q.limit.unwrap_or(100).min(500), q.before)?;
        Ok(Json(
            entries
                .into_iter()
                .map(|e| AuditEntry {
                    id: e.id,
                    at: e.at,
                    actor: e.actor,
                    action: e.action,
                    target: e.target,
                    target_account_id: e.target_account,
                    details: e.details,
                })
                .collect(),
        ))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities() {
        assert_eq!(decode_entities("Natives&apos; Hollow"), "Natives' Hollow");
        assert_eq!(decode_entities("a &amp; b &#65;&#x42; &bogus; & c &"), "a & b AB &bogus; & c &");
    }

    #[test]
    fn game_data() {
        let figs = minifigure_list();
        assert_eq!(figs.len(), 133);
        let dj = figs.iter().find(|m| m.id == 1922).unwrap();
        assert_eq!(dj.name, "DJ");
        assert_eq!(dj.normal_attack.as_deref(), Some("Throw Record"));
        assert_eq!(dj.upgrades[0].name, "Fast");
        assert_eq!(dj.parts.len(), 3);
        assert!(figs.iter().all(|m| !m.name.is_empty()));
    }
}

#[cfg(test)]
mod content_tests {
    use super::*;

    /// The list as built from data/minifig_parts.json and data/minifigure_stats.json before the
    /// content tree (to compare with).
    fn old_minifigure_list() -> &'static [Minifigure] {
        #[derive(Deserialize)]
        struct Parts {
            minifigures: Vec<PartsEntry>,
        }
        #[derive(Deserialize)]
        struct PartsEntry {
            minifigure_template_id: u32,
            name: Option<String>,
            series: u32,
            parts: Vec<Part>,
        }
        #[derive(Deserialize)]
        struct Part {
            location: String,
            part_token_id: u32,
            part_name: String,
        }
        #[derive(Deserialize)]
        struct Stats {
            minifigures: BTreeMap<u32, StatsEntry>,
        }
        #[derive(Deserialize)]
        struct StatsEntry {
            normal: Option<Attack>,
            special: Option<Attack>,
            #[serde(default)]
            upgrades: Vec<Upgrade>,
        }
        #[derive(Deserialize)]
        struct Attack {
            name: Option<String>,
        }
        #[derive(Deserialize)]
        struct Upgrade {
            bit: u32,
            name: String,
        }
        static LIST: std::sync::LazyLock<Vec<Minifigure>> = std::sync::LazyLock::new(|| {
            let parts: Parts = serde_json::from_str(include_str!("../../../data/archive/minifig_parts.json")).expect("minifig_parts.json");
            let mut stats: Stats =
                serde_json::from_str(include_str!("../../../data/archive/minifigure_stats.json")).expect("minifigure_stats.json");
            let mut list: Vec<Minifigure> = parts
                .minifigures
                .into_iter()
                .map(|m| {
                    let st = stats.minifigures.remove(&m.minifigure_template_id);
                    let attack = |a: Option<Attack>| a.and_then(|a| a.name).map(|n| decode_entities(&n));
                    let (normal, special, upgrades) = match st {
                        Some(s) => (s.normal, s.special, s.upgrades),
                        None => (None, None, Vec::new()),
                    };
                    // the client data has no name for the DJ (1922); its parts are "DJ Head" etc.
                    let name = m.name.or_else(|| {
                        let head = m.parts.iter().find(|p| p.location == "Head")?;
                        Some(head.part_name.strip_suffix(" Head")?.to_string())
                    });
                    Minifigure {
                        id: m.minifigure_template_id,
                        name: decode_entities(&name.unwrap_or_default()),
                        series: m.series,
                        normal_attack: attack(normal),
                        special_attack: attack(special),
                        upgrades: upgrades
                            .into_iter()
                            .filter(|u| u.name != "No upgrade available")
                            .map(|u| MinifigureUpgrade {
                                bit: u.bit,
                                name: decode_entities(u.name.strip_prefix("Upgrade: ").unwrap_or(&u.name)),
                            })
                            .collect(),
                        parts: m
                            .parts
                            .into_iter()
                            .map(|p| MinifigurePart { token: p.part_token_id, location: p.location, name: decode_entities(&p.part_name) })
                            .collect(),
                    }
                })
                .collect();
            list.sort_by_key(|m| m.id);
            list
        });
        &LIST
    }

    #[test]
    fn playfields_same_as_the_old_files() {
        #[derive(Deserialize)]
        struct Destinations {
            destinations: BTreeMap<u32, Dest>,
        }
        #[derive(Deserialize)]
        struct Dest {
            name: String,
        }
        #[derive(Deserialize)]
        struct Names {
            playfields: BTreeMap<u32, String>,
        }
        let d: Destinations = serde_json::from_str(include_str!("../../../data/archive/destinations.json")).unwrap();
        let n: Names = serde_json::from_str(include_str!("../../../data/archive/playfields.json")).unwrap();
        let mut old: BTreeMap<u32, (String, bool)> = n.playfields.into_iter().map(|(id, name)| (id, (decode_entities(&name), false))).collect();
        for (id, dest) in d.destinations {
            old.insert(id, (decode_entities(&dest.name), true));
        }
        let new: BTreeMap<u32, (String, bool)> = playfield_list().iter().map(|p| (p.id, (p.name.clone(), p.teleport))).collect();
        assert_eq!(old, new);
    }

    #[test]
    fn minifigures_same_as_the_old_files() {
        let (old, new) = (old_minifigure_list(), minifigure_list());
        assert_eq!(old.len(), new.len());
        for (o, n) in old.iter().zip(new) {
            assert_eq!(serde_json::to_value(o).unwrap(), serde_json::to_value(n).unwrap(), "{}", o.name);
        }
    }
}
