//! The Discord bot's routes (scope `discord`): game accounts linked to Discord accounts, the keys
//! that act as them, and one-time codes the bot sends in DMs; and the two routes where those
//! codes are used, from the web UI (`/v1/me/discord`, `/v1/auth/code`).

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
};
use lmoxide_db::{CodePurpose, Scope, validate_discord_id};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppState,
    auth::{Auth, client_ip},
    blocking,
    error::{ApiError, ApiResult, ErrorBody},
    routes::{Account, CreatedApiKey, Registration, Session, account_dto, api_key, new_session, session_ttl},
};

/// How long a one-time code lasts.
const CODE_TTL_SECS: i64 = 600;
/// The name of the keys made for linked accounts.
const ACCOUNT_KEY_NAME: &str = "discord-bot";

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(links))
        .routes(routes!(get_link, update_link, unlink))
        .routes(routes!(link_api_key))
        .routes(routes!(register))
        .routes(routes!(create_code))
        .routes(routes!(link_own_account))
        .routes(routes!(code_login))
}

/// A game account linked to a Discord account.
#[derive(Serialize, ToSchema)]
pub struct DiscordLink {
    /// The Discord user id (a snowflake, as a string).
    pub discord_id: String,
    pub account_id: u32,
    pub username: String,
    pub gm_level: u32,
    /// Whether a ban is in force.
    pub banned: bool,
}

impl From<lmoxide_db::DiscordLink> for DiscordLink {
    fn from(l: lmoxide_db::DiscordLink) -> Self {
        DiscordLink { discord_id: l.discord_id, account_id: l.account_id, username: l.username, gm_level: l.gm_level, banned: l.banned }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct DiscordRegistration {
    /// The Discord user id (a snowflake, as a string).
    #[schema(pattern = "^[0-9]{1,20}$")]
    pub discord_id: String,
    #[schema(min_length = 3, max_length = 32, pattern = "^[A-Za-z0-9_.-]+$")]
    pub username: String,
    #[schema(min_length = 8, max_length = 128)]
    pub password: String,
    /// Default 0.
    #[serde(default)]
    pub gm_level: u32,
}

#[derive(Serialize, ToSchema)]
pub struct DiscordRegistered {
    pub link: DiscordLink,
    /// The new account's key (it acts as the account).
    pub key: CreatedApiKey,
}

#[derive(Deserialize, ToSchema)]
pub struct LinkChanges {
    pub gm_level: u32,
}

/// What a one-time code is for.
#[derive(Deserialize, ToSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
#[schema(as = CodePurpose)]
pub enum CodePurposeDto {
    /// Link a game account to this (not yet linked) Discord account: `POST /v1/me/discord`.
    Link,
    /// Sign in to the linked game account to set a new password: `POST /v1/auth/code`.
    Login,
}

#[derive(Deserialize, ToSchema)]
pub struct NewCode {
    #[schema(pattern = "^[0-9]{1,20}$")]
    pub discord_id: String,
    pub purpose: CodePurposeDto,
}

#[derive(Serialize, ToSchema)]
pub struct DiscordCode {
    /// `XXXX-XXXX`; works once.
    pub code: String,
    /// Unix time.
    pub expires_at: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct RedeemCode {
    /// As sent by the bot; any case, the dash optional.
    pub code: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CodeLogin {
    /// As sent by the bot; any case, the dash optional.
    pub code: String,
    /// As in the login: a long session or a short one.
    #[serde(default)]
    pub remember: bool,
}

fn checked_id(id: &str) -> ApiResult<()> {
    Ok(validate_discord_id(id)?)
}

fn bad_code() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "bad_code", "wrong or expired code")
}

/// Every game account linked to a Discord account. Scope: discord.
#[utoipa::path(get, path = "/v1/discord/links", tag = "discord", responses((status = OK, body = [DiscordLink])))]
async fn links(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<DiscordLink>>> {
    auth.require(Scope::Discord)?;
    blocking(&state, |db| Ok(Json(db.discord_links()?.into_iter().map(Into::into).collect()))).await
}

/// The game account linked to a Discord account. Scope: discord.
#[utoipa::path(get, path = "/v1/discord/links/{discord_id}", tag = "discord", params(("discord_id" = String, Path)),
    responses((status = OK, body = DiscordLink), (status = NOT_FOUND, body = ErrorBody)))]
async fn get_link(State(state): State<Arc<AppState>>, auth: Auth, Path(discord_id): Path<String>) -> ApiResult<Json<DiscordLink>> {
    auth.require(Scope::Discord)?;
    checked_id(&discord_id)?;
    blocking(&state, move |db| {
        Ok(Json(db.discord_link(&discord_id)?.ok_or_else(|| ApiError::not_found("Discord link"))?.into()))
    })
    .await
}

/// Change the GM level of a linked account (from its next login). Scope: discord.
#[utoipa::path(patch, path = "/v1/discord/links/{discord_id}", tag = "discord", params(("discord_id" = String, Path)),
    request_body = LinkChanges, responses((status = OK, body = DiscordLink), (status = NOT_FOUND, body = ErrorBody)))]
async fn update_link(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(discord_id): Path<String>,
    Json(c): Json<LinkChanges>,
) -> ApiResult<Json<DiscordLink>> {
    auth.require(Scope::Discord)?;
    checked_id(&discord_id)?;
    blocking(&state, move |db| {
        let a = db.account_by_discord(&discord_id)?.ok_or_else(|| ApiError::not_found("Discord link"))?;
        if a.gm_level != c.gm_level {
            db.set_gm_level(a.id, c.gm_level)?;
            db.audit_account(&auth.actor, "account-gm", &a, Some(&c.gm_level.to_string()))?;
        }
        Ok(Json(db.discord_link(&discord_id)?.ok_or_else(|| ApiError::not_found("Discord link"))?.into()))
    })
    .await
}

/// Unlink a Discord account; the keys that act as its game account are revoked, the account
/// stays. Scope: discord.
#[utoipa::path(delete, path = "/v1/discord/links/{discord_id}", tag = "discord", params(("discord_id" = String, Path)),
    responses((status = NO_CONTENT), (status = NOT_FOUND, body = ErrorBody)))]
async fn unlink(State(state): State<Arc<AppState>>, auth: Auth, Path(discord_id): Path<String>) -> ApiResult<StatusCode> {
    auth.require(Scope::Discord)?;
    checked_id(&discord_id)?;
    blocking(&state, move |db| {
        let a = db.unlink_discord(&discord_id)?;
        db.audit_account(&auth.actor, "discord-unlink", &a, Some(&discord_id))?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// A new key that acts as the linked account (named `discord-bot`; it replaces the account's
/// earlier one). Its scope follows the account's GM level. Scope: discord.
#[utoipa::path(post, path = "/v1/discord/links/{discord_id}/api-key", tag = "discord", params(("discord_id" = String, Path)),
    responses((status = CREATED, body = CreatedApiKey), (status = NOT_FOUND, body = ErrorBody)))]
async fn link_api_key(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(discord_id): Path<String>,
) -> ApiResult<(StatusCode, Json<CreatedApiKey>)> {
    auth.require(Scope::Discord)?;
    checked_id(&discord_id)?;
    let created = blocking(&state, move |db| {
        let a = db.account_by_discord(&discord_id)?.ok_or_else(|| ApiError::not_found("Discord link"))?;
        let (key, secret) = db.create_account_key(a.id, ACCOUNT_KEY_NAME)?;
        db.audit_account(&auth.actor, "api-key-create", &a, Some(&format!("account key {}", key.name)))?;
        Ok(CreatedApiKey { key: api_key(key), secret })
    })
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// Create a game account linked to a Discord account, with a key that acts as it. Not while
/// registration is `closed`. Scope: discord.
#[utoipa::path(post, path = "/v1/discord/accounts", tag = "discord", request_body = DiscordRegistration,
    responses((status = CREATED, body = DiscordRegistered), (status = FORBIDDEN, body = ErrorBody), (status = CONFLICT, body = ErrorBody)))]
async fn register(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(r): Json<DiscordRegistration>,
) -> ApiResult<(StatusCode, Json<DiscordRegistered>)> {
    auth.require(Scope::Discord)?;
    if state.cfg.registration() == Registration::Closed {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "registration_closed", "registration is closed"));
    }
    checked_id(&r.discord_id)?;
    let created = blocking(&state, move |db| {
        if db.account_by_discord(&r.discord_id)?.is_some() {
            return Err(ApiError::conflict("this Discord account is already linked to a game account"));
        }
        let a = db.create_account(&r.username, Some(&r.password), r.gm_level)?;
        if let Err(e) = db.link_discord(a.id, &r.discord_id) {
            // linked by someone else meanwhile
            db.delete_account(a.id)?;
            return Err(e.into());
        }
        db.audit_account(&auth.actor, "discord-register", &a, Some(&r.discord_id))?;
        let (key, secret) = db.create_account_key(a.id, ACCOUNT_KEY_NAME)?;
        let link = db.discord_link(&r.discord_id)?.ok_or_else(|| ApiError::not_found("Discord link"))?;
        Ok(DiscordRegistered { link: link.into(), key: CreatedApiKey { key: api_key(key), secret } })
    })
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// A one-time code (`XXXX-XXXX`, 10 minutes, works once) for the bot to send to that Discord
/// user in a DM; it replaces their last one for the same purpose. `link`: the Discord account
/// must not be linked yet (409); `login`: it must be (404). Scope: discord.
#[utoipa::path(post, path = "/v1/discord/codes", tag = "discord", request_body = NewCode,
    responses((status = CREATED, body = DiscordCode), (status = NOT_FOUND, body = ErrorBody), (status = CONFLICT, body = ErrorBody)))]
async fn create_code(State(state): State<Arc<AppState>>, auth: Auth, Json(n): Json<NewCode>) -> ApiResult<(StatusCode, Json<DiscordCode>)> {
    auth.require(Scope::Discord)?;
    checked_id(&n.discord_id)?;
    let code = blocking(&state, move |db| {
        let linked = db.account_by_discord(&n.discord_id)?.is_some();
        let purpose = match n.purpose {
            CodePurposeDto::Link if linked => {
                return Err(ApiError::conflict("this Discord account is already linked to a game account"));
            }
            CodePurposeDto::Login if !linked => return Err(ApiError::not_found("Discord link")),
            CodePurposeDto::Link => CodePurpose::Link,
            CodePurposeDto::Login => CodePurpose::Login,
        };
        let (code, expires_at) = db.create_discord_code(&n.discord_id, purpose, CODE_TTL_SECS)?;
        Ok(DiscordCode { code, expires_at })
    })
    .await?;
    Ok((StatusCode::CREATED, Json(code)))
}

/// Link the logged-in account to the Discord account that got this `link` code from the bot.
/// 401 `bad_code` for a wrong or expired code (counts as a failed login); 409 if either is
/// linked already.
#[utoipa::path(post, path = "/v1/me/discord", tag = "auth", request_body = RedeemCode,
    responses((status = OK, body = Account), (status = UNAUTHORIZED, body = ErrorBody), (status = CONFLICT, body = ErrorBody),
        (status = TOO_MANY_REQUESTS, body = ErrorBody)))]
async fn link_own_account(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    auth: Auth,
    Json(r): Json<RedeemCode>,
) -> ApiResult<Json<Account>> {
    let id = auth.account.ok_or_else(|| ApiError::not_found("account (API keys have none)"))?;
    let ip_key = format!("ip:{}", client_ip(&state, &headers, peer));
    if !state.lockout.allowed(&ip_key) {
        return Err(ApiError::too_many());
    }
    let result = blocking(&state, move |db| {
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        if a.discord_id.is_some() {
            return Err(ApiError::conflict("this account is already linked to a Discord account"));
        }
        let Some(discord_id) = db.take_discord_code(&r.code, CodePurpose::Link)? else { return Ok(None) };
        db.link_discord(id, &discord_id)?;
        db.audit_account(&auth.actor, "discord-link", &a, Some(&discord_id))?;
        let a = db.account(id)?.ok_or_else(|| ApiError::not_found("account"))?;
        Ok(Some(account_dto(db, a, true)?))
    })
    .await?;
    match result {
        Some(a) => Ok(Json(a)),
        None => {
            state.lockout.failed(&ip_key);
            Err(bad_code())
        }
    }
}

/// Sign in with a `login` code the Discord bot sent: a session for the linked account with
/// `password_reset`, in which `/v1/me/password` takes a new password without the current one.
/// 401 `bad_code` for a wrong or expired code, or a banned account (counts as a failed login).
#[utoipa::path(post, path = "/v1/auth/code", tag = "auth", request_body = CodeLogin,
    responses((status = OK, body = Session), (status = UNAUTHORIZED, body = ErrorBody), (status = TOO_MANY_REQUESTS, body = ErrorBody)),
    security(()))]
async fn code_login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(c): Json<CodeLogin>,
) -> ApiResult<Json<Session>> {
    let ip_key = format!("ip:{}", client_ip(&state, &headers, peer));
    if !state.lockout.allowed(&ip_key) {
        return Err(ApiError::too_many());
    }
    let st = state.clone();
    let ttl = session_ttl(&state, c.remember);
    let result = blocking(&state, move |db| {
        let Some(discord_id) = db.take_discord_code(&c.code, CodePurpose::Login)? else { return Ok(None) };
        let Some(a) = db.account_by_discord(&discord_id)?.filter(|a| a.active_ban().is_none()) else { return Ok(None) };
        db.audit(&format!("account:{}", a.username), "api-login", None, Some("discord code"))?;
        Ok(Some(new_session(&st, db, a, ttl, true)?))
    })
    .await?;
    match result {
        Some(s) => Ok(Json(s)),
        None => {
            state.lockout.failed(&ip_key);
            Err(bad_code())
        }
    }
}
