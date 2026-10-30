//! The internal control interface for `lmoxide-api` (types and routes: `lmoxide_control`).
//! Bound to a private address (`--control-bind`, default 127.0.0.1) and protected by a shared
//! bearer token (`--control-token`); without a token it is not started.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use lmoxide_control::{Broadcast, Command, CommandDone, Done, Kick, Maintenance, OnlinePlayer, Status, TestCatalog};
use tracing::info;

use crate::{sessions, state};

/// Largest survey request (an imported set).
pub const SURVEY_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Shortest control token accepted.
pub const MIN_TOKEN_LEN: usize = 32;

pub async fn serve(addr: SocketAddr, token: String) -> anyhow::Result<()> {
    let token = Arc::new(token);
    let app = Router::new()
        .route("/v1/status", get(status))
        .route("/v1/players", get(players))
        .route("/v1/instances", get(instances))
        .route("/v1/players/{account}/kick", post(kick))
        .route("/v1/players/{account}/command", post(command))
        .route("/v1/test-commands", get(test_commands))
        .route("/v1/characters/{character}/achievements/remove", post(remove_achievements))
        .route("/v1/characters/{character}/name", post(rename_character))
        .route("/v1/broadcast", post(broadcast))
        .route("/v1/maintenance", post(maintenance))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        // survey imports carry whole sets
        .merge(crate::survey::routes().layer(axum::extract::DefaultBodyLimit::max(SURVEY_BODY_LIMIT)))
        .merge(crate::content::routes())
        .merge(crate::editor::routes().layer(axum::extract::DefaultBodyLimit::max(SURVEY_BODY_LIMIT)))
        .layer(middleware::from_fn_with_state(token, auth));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("control interface listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn auth(State(token): State<Arc<String>>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if given.is_some_and(|g| lmoxide_control::token_eq(g, &token)) {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn current_status() -> Status {
    let m = state::maintenance();
    Status {
        version: env!("CARGO_PKG_VERSION").into(),
        started_at: state::started_at(),
        players_online: sessions::players_online(),
        maintenance: m.is_some(),
        maintenance_message: m.flatten(),
    }
}

async fn status() -> Json<Status> {
    Json(current_status())
}

async fn players() -> Json<Vec<OnlinePlayer>> {
    Json(sessions::players())
}

async fn instances() -> Json<lmoxide_control::Instances> {
    let members = crate::world::counts();
    let npcs = crate::npc::counts();
    let mut copies = crate::worlds::spawned();
    copies.extend(members.keys().filter(|pf| !crate::worlds::is_spawned(**pf)));
    let instances = copies
        .into_iter()
        .map(|pf| {
            let preview = crate::preview::of(pf);
            let (characters, players) = members.get(&pf).copied().unwrap_or_default();
            lmoxide_control::Instance {
                playfield: pf.model(),
                copy: pf.copy(),
                key: pf.key(),
                mode: crate::instance::mode_of(pf).id().into(),
                kind: if preview.is_some() { "preview" } else if pf.copy() >= crate::instance::FIRST_EXTRA_COPY { "extra" } else { "main" }.into(),
                preview_owner: preview.map(|p| p.owner.instance),
                characters: characters as u32,
                players: players as u32,
                npcs: npcs.get(&pf).copied().unwrap_or(0) as u32,
            }
        })
        .collect();
    let (ai_tick_ms, ai_tick_max_ms) = crate::npc::tick_stats();
    Json(lmoxide_control::Instances { instances, ai_tick_ms, ai_tick_max_ms, open_world_cap: crate::instance::open_world_cap() })
}

async fn kick(Path(account): Path<u32>, Json(k): Json<Kick>) -> Json<Done> {
    let kicked = sessions::kick(account, k.reason.or_else(|| Some("You were disconnected by an administrator.".into())));
    info!(account, kicked, "control: kick");
    Json(Done { affected: kicked as u32 })
}

/// How long a command may take to be handled before answering without (all) its replies.
const COMMAND_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

async fn command(Path(account): Path<u32>, Json(c): Json<Command>) -> Result<Json<CommandDone>, (StatusCode, &'static str)> {
    if !crate::devcmd::is_command(&c.command) || c.command.len() > 500 {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "not a test command (! or /)"));
    }
    // keep the characters' server lines from before the command is queued
    let characters = crate::sessions::characters_of(account);
    for &ch in &characters {
        crate::chat::capture(ch);
    }
    let sent = crate::sessions::command(account, &c.command);
    let affected = sent.len() as u32;
    for (_, handled) in sent {
        let _ = tokio::time::timeout(COMMAND_WAIT, handled).await;
    }
    let mut replies = Vec::new();
    for ch in characters {
        replies.extend(crate::chat::end_capture(ch));
    }
    info!(account, command = %c.command, affected, "control: command");
    Ok(Json(CommandDone { affected, replies }))
}

/// Takes achievements away from a character: through its connection when it is online (its
/// client sees it at once), else in the database.
async fn remove_achievements(Path(character): Path<u32>, Json(r): Json<lmoxide_control::RemoveAchievements>) -> Result<Json<lmoxide_control::RemovedAchievements>, (StatusCode, String)> {
    if !r.all && r.ids.is_empty() {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "name the achievements (ids) or all".into()));
    }
    let which = (!r.all).then_some(r.ids);
    if let Some(answer) = crate::sessions::remove_achievements(character, which.clone()) {
        let removed = tokio::time::timeout(COMMAND_WAIT, answer).await.map_err(|_| (StatusCode::GATEWAY_TIMEOUT, "the character's connection didn't answer".to_string()))?.unwrap_or_default();
        return Ok(Json(lmoxide_control::RemovedAchievements { removed, online: true }));
    }
    let store = crate::store::Store;
    let mut c = store.load(character).ok_or((StatusCode::NOT_FOUND, format!("no character {character}")))?;
    let (removed, _) = crate::achievements::remove(&mut c, which.as_deref());
    store.save(character, &c).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    info!(character, ?removed, "control: achievements removed (offline)");
    Ok(Json(lmoxide_control::RemovedAchievements { removed, online: false }))
}

async fn rename_character(Path(character): Path<u32>, Json(r): Json<lmoxide_control::RenameCharacter>) -> Result<Json<lmoxide_control::CharacterRenamed>, (StatusCode, String)> {
    let (name, online) = rename(character, &r.name).await?;
    Ok(Json(lmoxide_control::CharacterRenamed { name, online }))
}

/// Renames a character: through its connection when it is online (it is sent again where it
/// stands, so every client shows the new name), else in the database. Returns the name as
/// stored and whether it was online; refusals as an HTTP status and why.
pub async fn rename(character: u32, name: &str) -> Result<(String, bool), (StatusCode, String)> {
    let refused = |e: &lmoxide_db::Error| match e {
        lmoxide_db::Error::NotFound(_) => (StatusCode::NOT_FOUND, format!("no character {character}")),
        lmoxide_db::Error::NameTaken => (StatusCode::CONFLICT, "another account has a character with this name".to_string()),
        lmoxide_db::Error::Invalid(m) => (StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
        e => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if let Some(answer) = sessions::rename(character, name) {
        let answer = tokio::time::timeout(COMMAND_WAIT, answer).await;
        let result = answer
            .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, "the character's connection didn't answer".to_string()))?
            .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, "the character went offline".to_string()))?;
        return result.map(|n| (n, true)).map_err(|e| refused(&e));
    }
    let name = state::db().rename_character(character, name).map_err(|e| refused(&e))?;
    info!(character, %name, "control: character renamed (offline)");
    Ok((name, false))
}

async fn test_commands() -> Json<TestCatalog> {
    Json(crate::devcmd::catalog())
}

async fn broadcast(Json(b): Json<Broadcast>) -> Result<Json<Done>, (StatusCode, &'static str)> {
    let message = b.message.trim();
    if message.is_empty() || message.len() > 500 {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "message must be 1-500 characters"));
    }
    let affected = crate::chat::broadcast(crate::game::SERVER_INSTANCE, message);
    info!(affected, %message, "control: broadcast");
    Ok(Json(Done { affected }))
}

async fn maintenance(Json(m): Json<Maintenance>) -> Json<Status> {
    state::set_maintenance(m.enabled.then_some(m.message));
    info!(enabled = m.enabled, "control: maintenance");
    Json(current_status())
}
