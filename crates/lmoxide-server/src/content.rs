//! World content on play servers (docs/WORLD-CONTENT.md): the content tree as loaded, the world
//! editor's work in it like everything else. At start: its spawns (npc.rs; spawns of script
//! waves wait for their wave), trigger walls (walls.rs), scripts (scripts.rs), which enemies
//! count toward which achievements (achievements.rs), the area quests (quests.rs); jump pads come with world_data.rs.
//!
//! What editors publish waits in the world editor's publish queue (`--content-queue`, the
//! survey server's `var/content-queue`, lmoxide_content::queue); admins see it and reject it
//! here (`/v1/content` on the control interface, through lmoxide-api). Approving commits the
//! entry to the content repository, which servers then pull: not built yet
//! (docs/WORLD-EDITOR-CONTENT.md, phase 3).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

use axum::{
    Json, Router,
    extract::Path as UrlPath,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use lmoxide_content::id::RecordKind;
use lmoxide_content::queue::{Entry, Queue};
use lmoxide_content::views;
use lmoxide_content::work::Work;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::npc::{self, SpawnDef};

static ENABLED: AtomicBool = AtomicBool::new(false);
/// The publish queue's folder, if this server has one.
static QUEUE: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(Mutex::default);

/// Whether this (play) server runs the content's world editor work.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// A play server, with the world editor's publish queue in `queue` if given.
pub fn init(queue: Option<PathBuf>) {
    info!(queue = ?queue, "world content");
    *QUEUE.lock().unwrap() = queue;
    ENABLED.store(true, Ordering::Relaxed);
}

/// Spawns `all` (npc::start's list: what the content spawns at start) and starts the content's
/// trigger walls, scripts and smash rules.
pub fn start(all: Vec<(u32, SpawnDef)>) {
    for (pf, def) in all {
        npc::spawn_as(npc::new_id(&def), def, pf);
    }
    let dir = crate::gamedata::dir(None);
    let dir = crate::gamedata::loaded_from().unwrap_or(dir);
    let work = match Work::open(&dir) {
        Ok(w) => w,
        Err(e) => {
            warn!("world content: {}: {e:#}; no trigger walls, scripts or smash rules", dir.display());
            return;
        }
    };
    let pfs: std::collections::BTreeSet<u32> =
        work.records().filter(|(k, _)| matches!(k.id.kind, RecordKind::Trigger | RecordKind::Script | RecordKind::Spawn)).filter_map(|(k, _)| k.id.world).collect();
    let mut walls = BTreeMap::new();
    let mut scripts = BTreeMap::new();
    for pf in pfs {
        let w = views::walls_of(&work, pf);
        if !w.is_empty() {
            walls.insert(pf, w);
        }
        let n = views::notes_of(&work, pf);
        let spawns = crate::survey::wave_spawns(&work, pf);
        if !n.scripts.is_empty() || !spawns.is_empty() {
            scripts.insert(pf, crate::scripts::Playfield { scripts: n.scripts, notes: n.notes, spawns });
        }
    }
    crate::walls::set(walls);
    crate::scripts::set(scripts);
    crate::achievements::set_smash_rules(&views::rules(&work));
    crate::quests::set_from_work(&work);
}

// ------------------------------------------------------------------ control routes

pub fn routes() -> Router {
    Router::new()
        .route("/v1/content/state", get(state))
        .route("/v1/content/queue", get(queue))
        .route("/v1/content/queue/{seq}", get(entry))
        .route("/v1/content/approve", post(approve))
        .route("/v1/content/reject", post(reject))
}

struct Error(StatusCode, String);

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": { "code": "content", "message": self.1 } }))).into_response()
    }
}

fn the_queue() -> Result<Queue, Error> {
    if !enabled() {
        return Err(Error(StatusCode::NOT_FOUND, "this server doesn't run world content (a survey server?)".into()));
    }
    QUEUE.lock().unwrap().as_deref().map(Queue::new).ok_or_else(|| Error(StatusCode::NOT_FOUND, "no publish queue here (--content-queue)".into()))
}

/// The content this server runs and how many publishes wait.
async fn state() -> Result<Json<Value>, Error> {
    let queued = the_queue().map(|q| q.entries().iter().filter(|e| e.status == "queued").count()).ok();
    Ok(Json(json!({ "content": crate::gamedata::loaded_from(), "queue": *QUEUE.lock().unwrap(), "queued": queued })))
}

/// Every publish waiting (and rejected ones), oldest first.
async fn queue() -> Result<Json<Vec<Entry>>, Error> {
    Ok(Json(the_queue()?.entries()))
}

async fn entry(UrlPath(seq): UrlPath<u64>) -> Result<Json<Entry>, Error> {
    the_queue()?.get(seq).map(Json).ok_or_else(|| Error(StatusCode::NOT_FOUND, format!("no queue entry {seq}")))
}

#[derive(Deserialize)]
struct Decision {
    seq: u64,
    #[serde(default)]
    reason: Option<String>,
}

/// Approving commits the entry to the content repository: not built yet.
async fn approve(Json(_): Json<Decision>) -> Result<Json<Value>, Error> {
    Err(Error(StatusCode::NOT_IMPLEMENTED, "approving (committing to the content repository) isn't built yet".into()))
}

/// Rejects an entry: it stays in the queue as rejected, with the reason, until its author
/// publishes those records again.
async fn reject(Json(d): Json<Decision>) -> Result<Json<Entry>, Error> {
    let q = the_queue()?;
    let mut e = q.get(d.seq).ok_or_else(|| Error(StatusCode::NOT_FOUND, format!("no queue entry {}", d.seq)))?;
    e.status = "rejected".into();
    e.reason = d.reason;
    q.write(&e).map_err(|err| Error(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(e))
}
