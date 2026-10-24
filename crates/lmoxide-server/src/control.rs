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
use lmoxide_control::{Broadcast, Done, Kick, Maintenance, OnlinePlayer, Status};
use tracing::info;

use crate::{sessions, state};

/// Shortest control token accepted.
pub const MIN_TOKEN_LEN: usize = 32;

pub async fn serve(addr: SocketAddr, token: String) -> anyhow::Result<()> {
    let token = Arc::new(token);
    let app = Router::new()
        .route("/v1/status", get(status))
        .route("/v1/players", get(players))
        .route("/v1/players/{account}/kick", post(kick))
        .route("/v1/broadcast", post(broadcast))
        .route("/v1/maintenance", post(maintenance))
        .layer(middleware::from_fn_with_state(token, auth))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024));
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

async fn kick(Path(account): Path<u32>, Json(k): Json<Kick>) -> Json<Done> {
    let kicked = sessions::kick(account, k.reason.or_else(|| Some("You were disconnected by an administrator.".into())));
    info!(account, kicked, "control: kick");
    Json(Done { affected: kicked as u32 })
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
