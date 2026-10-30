use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

/// Error answer: `{"error": {"code": "...", "message": "..."}}`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorDetail {
    /// Stable machine-readable code, e.g. `unauthorized`, `forbidden`, `not_found`.
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid credentials")
    }
    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "this credential's scope doesn't allow that")
    }
    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", format!("{what} not found"))
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", message)
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }
    pub fn too_many() -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "too many requests, slow down")
    }
    pub fn game_server(e: lmoxide_control::Error) -> Self {
        tracing::warn!("control interface: {e}");
        Self::new(StatusCode::BAD_GATEWAY, "game_server_unavailable", "the game server can't be reached")
    }
    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!("internal error: {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
    }
}

impl From<lmoxide_db::Error> for ApiError {
    fn from(e: lmoxide_db::Error) -> Self {
        match e {
            lmoxide_db::Error::NotFound(what) => Self::not_found(what),
            lmoxide_db::Error::Invalid(m) => Self::invalid(m),
            lmoxide_db::Error::UsernameTaken => Self::conflict("username already taken"),
            lmoxide_db::Error::Conflict(m) => Self::conflict(m),
            e => Self::internal(e),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: ErrorDetail { code: self.code.into(), message: self.message } };
        (self.status, Json(body)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
