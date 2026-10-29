//! The LMOxide patcher protocol (`/lmoxide/v1/`, see `lmoxide_patch` and `docs/PATCHING.md`).

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use lmoxide_patch::{Info, PROTOCOL_VERSION, Platform, is_sha256_hex, manifest::public_key_hex};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tracing::debug;

use crate::App;

pub async fn info(State(app): State<Arc<App>>) -> Json<Info> {
    let catalog = app.catalog();
    let mut ready: Vec<Platform> = catalog.manifests.keys().copied().collect();
    ready.sort();
    Json(Info {
        protocol: PROTOCOL_VERSION,
        name: app.cfg.server_name.clone(),
        universe_addr: app.cfg.universe_addr(),
        http_addr: app.cfg.public_http_addr(),
        https_addr: app.cfg.public_https_addr(),
        platforms: app.platforms(),
        ready,
        manifest_key: public_key_hex(&app.signing.verifying_key()),
        login_public_key: catalog.login_key.clone(),
        warnings: catalog.warnings.clone(),
        revisions: catalog.manifests.iter().map(|(p, m)| (*p, m.revision.clone())).collect(),
        install_bytes: catalog.manifests.iter().map(|(p, m)| (*p, m.install_bytes)).collect(),
        api_url: app.cfg.api_url.clone(),
        website: app.cfg.website.clone(),
        patchers: catalog.patchers.clone(),
        kind: app.cfg.server_kind.clone(),
        other_servers: app.cfg.other_servers.clone(),
        modes: app.cfg.game_modes(),
    })
}

/// The game server's login public key as plain text (512 hex characters + newline), for
/// clients patched by hand (`patches/patch-client.py --login-key-url`, the Linux kit). Read
/// from the key file on every request, so it is current even between rebuilds. 404 without
/// `--login-public-key-file`, 503 while the file is missing or invalid.
pub async fn login_key(State(app): State<Arc<App>>) -> Response {
    let Some(path) = app.cfg.login_public_key_file.clone() else {
        return (StatusCode::NOT_FOUND, "no login public key configured").into_response();
    };
    match tokio::task::spawn_blocking(move || crate::app::read_login_key(Some(&path))).await {
        Ok(Ok(Some(key))) => {
            ([(header::CONTENT_TYPE, "text/plain"), (header::CACHE_CONTROL, "no-cache")], key + "\n").into_response()
        }
        _ => (StatusCode::SERVICE_UNAVAILABLE, [(header::RETRY_AFTER, "10")], "login public key not available yet")
            .into_response(),
    }
}

/// The signed manifest. Its `ETag` is the manifest's revision, so a patcher sending the
/// installed one in `If-None-Match` gets `304 Not Modified` while nothing changed.
pub async fn manifest(State(app): State<Arc<App>>, Path(platform): Path<String>, headers: HeaderMap) -> Response {
    let Ok(platform) = platform.parse::<Platform>() else { return StatusCode::NOT_FOUND.into_response() };
    if !app.platforms().contains(&platform) {
        return (StatusCode::NOT_FOUND, "platform not offered by this server").into_response();
    }
    match app.catalog().manifests.get(&platform) {
        Some(m) => {
            let etag = format!("\"{}\"", m.revision);
            let cache = [(header::ETAG, etag.clone()), (header::CACHE_CONTROL, "no-cache".into())];
            let unchanged = headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"));
            if unchanged {
                (StatusCode::NOT_MODIFIED, cache).into_response()
            } else {
                (cache, [(header::CONTENT_TYPE, "application/json")], Body::from(m.json.clone())).into_response()
            }
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "5")],
            "still indexing the client files",
        )
            .into_response(),
    }
}

/// Signed patcher releases for a platform (`[]` if none); 404 without `--patcher-dir`.
pub async fn patchers(State(app): State<Arc<App>>, Path(platform): Path<String>) -> Response {
    let Ok(platform) = platform.parse::<Platform>() else { return StatusCode::NOT_FOUND.into_response() };
    if app.cfg.patcher_dir.is_none() {
        return (StatusCode::NOT_FOUND, "no patcher releases on this server").into_response();
    }
    let body = app.catalog().releases.get(&platform).cloned().unwrap_or_else(|| Bytes::from_static(b"[]"));
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-cache")], Body::from(body)).into_response()
}

/// File bodies by sha256, streamed from disk with Range support (resumable downloads).
pub async fn file(State(app): State<Arc<App>>, Path(sha): Path<String>, req: Request) -> Response {
    if !is_sha256_hex(&sha) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(path) = app.catalog().files.get(&sha).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    debug!(%sha, path = %path.display(), range = ?req.headers().get(header::RANGE), "file");
    let resp = ServeFile::new_with_mime(&path, &mime::APPLICATION_OCTET_STREAM).oneshot(req).await;
    let mut resp = match resp {
        Ok(r) => r.into_response(),
        Err(e) => match e {},
    };
    if resp.status().is_success() {
        // Content-addressed: never changes.
        resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    }
    resp
}
