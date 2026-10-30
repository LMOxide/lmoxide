//! Account friends (`/v1/me/friends`): an account asks another by username; once accepted, every
//! character of each is friends with every character of the other, later ones too
//! (lmoxide-db account_friends.rs). Online characters' friend lists follow at once through the
//! game server's control interface. The Discord bot sends requests to linked targets by DM with
//! Accept / Reject buttons (`/v1/discord/friend-requests`) and answers with the target's
//! account key.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use lmoxide_db::{FriendState, Requested, Scope};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppState,
    auth::Auth,
    blocking,
    error::{ApiError, ApiResult, ErrorBody},
};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(my_friends, request_friend))
        .routes(routes!(remove_friend))
        .routes(routes!(accept_friend))
        .routes(routes!(reject_friend))
        .routes(routes!(friend_requests))
        .routes(routes!(friend_request_notified))
}

/// How the caller stands with another account.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccountFriendState {
    /// Friends: every character of each is friends with every character of the other.
    Friends,
    /// They asked; waiting for the caller's answer.
    Incoming,
    /// The caller asked; waiting for theirs.
    Outgoing,
}

/// Another account as the caller sees it.
#[derive(Serialize, ToSchema)]
pub struct AccountFriend {
    pub account_id: u32,
    pub username: String,
    pub state: AccountFriendState,
    /// Unix time it was asked.
    pub requested_at: i64,
    /// Unix time it was accepted.
    pub accepted_at: Option<i64>,
}

impl From<lmoxide_db::AccountFriend> for AccountFriend {
    fn from(f: lmoxide_db::AccountFriend) -> Self {
        AccountFriend {
            account_id: f.account_id,
            username: f.username,
            state: match f.state {
                FriendState::Friends => AccountFriendState::Friends,
                FriendState::Incoming => AccountFriendState::Incoming,
                FriendState::Outgoing => AccountFriendState::Outgoing,
            },
            requested_at: f.requested_at,
            accepted_at: f.accepted_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct FriendRequest {
    /// The other account's username.
    pub username: String,
}

/// A request waiting for a target linked to Discord that the bot hasn't sent yet.
#[derive(Serialize, ToSchema)]
pub struct FriendRequestNotice {
    pub requester_id: u32,
    pub requester_username: String,
    pub target_id: u32,
    pub target_username: String,
    /// The target's Discord user id (a snowflake, as a string).
    pub discord_id: String,
    pub requested_at: i64,
}

fn me(auth: &Auth) -> ApiResult<u32> {
    auth.account.ok_or_else(|| ApiError::not_found("account (API keys that don't act as an account have none)"))
}

/// Tells the game server which character friendships changed, so online friend lists follow
/// (without it they do at the next login).
async fn tell_game_server(state: &AppState, added: Vec<(u32, u32)>, removed: Vec<(u32, u32)>) {
    if added.is_empty() && removed.is_empty() {
        return;
    }
    if let Some(c) = &state.control
        && let Err(e) = c.friends_changed(&lmoxide_control::FriendsChanged { added, removed }).await
    {
        tracing::warn!("friends: the game server wasn't told: {e}");
    }
}

/// The caller's account friends and requests either way, by username. Any account.
#[utoipa::path(get, path = "/v1/me/friends", tag = "friends",
    responses((status = OK, body = [AccountFriend]), (status = NOT_FOUND, body = ErrorBody)))]
async fn my_friends(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<AccountFriend>>> {
    let id = me(&auth)?;
    blocking(&state, move |db| Ok(Json(db.account_friends(id)?.into_iter().map(Into::into).collect()))).await
}

/// Ask another account to be friends (all your characters with all theirs). If they had asked
/// you already, you are friends at once (`state: friends`). Any account.
#[utoipa::path(post, path = "/v1/me/friends", tag = "friends", request_body = FriendRequest,
    responses((status = CREATED, body = AccountFriend), (status = NOT_FOUND, body = ErrorBody),
        (status = CONFLICT, description = "Already friends or already asked", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "Yourself", body = ErrorBody)))]
async fn request_friend(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(r): Json<FriendRequest>,
) -> ApiResult<(StatusCode, Json<AccountFriend>)> {
    let id = me(&auth)?;
    let (friend, added) = blocking(&state, move |db| {
        let other = db.account_by_name(r.username.trim())?.ok_or_else(|| ApiError::not_found("account"))?;
        let added = match db.request_account_friend(id, other.id)? {
            Requested::Sent => Vec::new(),
            Requested::Accepted(added) => added,
        };
        db.audit_account(&auth.actor, "friend-request", &other, None)?;
        let friend = db.account_friend(id, other.id)?.ok_or_else(|| ApiError::not_found("account friend"))?;
        Ok((friend, added))
    })
    .await?;
    tell_game_server(&state, added, Vec::new()).await;
    Ok((StatusCode::CREATED, Json(friend.into())))
}

/// Accept another account's request: every character of each becomes friends with every
/// character of the other. Any account (the request's target).
#[utoipa::path(post, path = "/v1/me/friends/{account_id}/accept", tag = "friends", params(("account_id" = u32, Path)),
    responses((status = OK, body = AccountFriend), (status = NOT_FOUND, description = "No request from them", body = ErrorBody)))]
async fn accept_friend(State(state): State<Arc<AppState>>, auth: Auth, Path(account_id): Path<u32>) -> ApiResult<Json<AccountFriend>> {
    let id = me(&auth)?;
    let (friend, added) = blocking(&state, move |db| {
        let added = db.accept_account_friend(id, account_id)?;
        let friend = db.account_friend(id, account_id)?.ok_or_else(|| ApiError::not_found("account friend"))?;
        if let Some(a) = db.account(account_id)? {
            db.audit_account(&auth.actor, "friend-accept", &a, None)?;
        }
        Ok((friend, added))
    })
    .await?;
    tell_game_server(&state, added, Vec::new()).await;
    Ok(Json(friend.into()))
}

/// Turn down another account's request (it is deleted; they may ask again). Any account (the
/// request's target).
#[utoipa::path(post, path = "/v1/me/friends/{account_id}/reject", tag = "friends", params(("account_id" = u32, Path)),
    responses((status = NO_CONTENT), (status = NOT_FOUND, description = "No request from them", body = ErrorBody)))]
async fn reject_friend(State(state): State<Arc<AppState>>, auth: Auth, Path(account_id): Path<u32>) -> ApiResult<StatusCode> {
    let id = me(&auth)?;
    blocking(&state, move |db| {
        db.reject_account_friend(id, account_id)?;
        if let Some(a) = db.account(account_id)? {
            db.audit_account(&auth.actor, "friend-reject", &a, None)?;
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Stop being friends with another account (their characters and yours stop being friends), or
/// withdraw a request either way. Any account.
#[utoipa::path(delete, path = "/v1/me/friends/{account_id}", tag = "friends", params(("account_id" = u32, Path)),
    responses((status = NO_CONTENT), (status = NOT_FOUND, body = ErrorBody)))]
async fn remove_friend(State(state): State<Arc<AppState>>, auth: Auth, Path(account_id): Path<u32>) -> ApiResult<StatusCode> {
    let id = me(&auth)?;
    let removed = blocking(&state, move |db| {
        let removed = db.remove_account_friend(id, account_id)?;
        if let Some(a) = db.account(account_id)? {
            db.audit_account(&auth.actor, "friend-remove", &a, None)?;
        }
        Ok(removed)
    })
    .await?;
    tell_game_server(&state, Vec::new(), removed).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Requests waiting for a target linked to Discord that the bot hasn't sent by DM yet, oldest
/// first. The bot polls this, DMs each target and marks it sent. Scope: discord.
#[utoipa::path(get, path = "/v1/discord/friend-requests", tag = "discord",
    responses((status = OK, body = [FriendRequestNotice])))]
async fn friend_requests(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<FriendRequestNotice>>> {
    auth.require(Scope::Discord)?;
    blocking(&state, |db| {
        Ok(Json(
            db.unnotified_friend_requests()?
                .into_iter()
                .map(|n| FriendRequestNotice {
                    requester_id: n.requester_id,
                    requester_username: n.requester_username,
                    target_id: n.target_id,
                    target_username: n.target_username,
                    discord_id: n.discord_id,
                    requested_at: n.requested_at,
                })
                .collect(),
        ))
    })
    .await
}

/// The bot sent the target this request (it isn't listed again). Scope: discord.
#[utoipa::path(post, path = "/v1/discord/friend-requests/{requester_id}/{target_id}/notified", tag = "discord",
    params(("requester_id" = u32, Path), ("target_id" = u32, Path)),
    responses((status = NO_CONTENT), (status = NOT_FOUND, description = "No such request waiting", body = ErrorBody)))]
async fn friend_request_notified(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((requester_id, target_id)): Path<(u32, u32)>,
) -> ApiResult<StatusCode> {
    auth.require(Scope::Discord)?;
    blocking(&state, move |db| {
        if db.mark_friend_request_notified(requester_id, target_id)? { Ok(StatusCode::NO_CONTENT) } else { Err(ApiError::not_found("friend request")) }
    })
    .await
}
