//! World content on play servers (docs/WORLD-CONTENT.md): the content tree as loaded, the world
//! editor's work in it like everything else. At start: its spawns (npc.rs; spawns of script
//! waves wait for their wave), trigger walls (walls.rs), scripts (scripts.rs), which enemies
//! count toward which achievements (achievements.rs), the area quests (quests.rs), trigger areas and
//! walls (triggers.rs); jump pads come with world_data.rs.
//!
//! What editors commit goes on their branches of the world editor's working copy
//! (lmoxide_content::contrib); admins review those branches and merge or reject them here
//! (`/v1/content` on the control interface, through lmoxide-api), on a server that runs the
//! world editor. Merged commits are on the working copy's `main`; play servers take them once
//! they are in the content they run (pushed to the content repository, pulled).

use std::collections::BTreeMap;
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

use axum::{
    Json, Router,
    extract::Path as UrlPath,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use lmoxide_content::id::RecordKind;
use lmoxide_content::contrib::Branch;
use lmoxide_content::views;
use lmoxide_content::work::Work;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::editor::ReviewError;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether this (play) server runs the content's world editor work.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// A play server.
pub fn init() {
    info!("world content");
    ENABLED.store(true, Ordering::Relaxed);
}

/// Starts the content's trigger walls, scripts and smash rules (the worlds' residents spawn
/// with each copy, npc::spawn_world).
pub fn start() {
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
    crate::triggers::set_from_work(&work);
    crate::story::init();
}

// ------------------------------------------------------------------ named points

/// Points saved in game since the content was loaded (`!point`): label, playfield, position.
static SAVED_POINTS: LazyLock<Mutex<Vec<(String, u32, [f32; 3])>>> = LazyLock::new(Default::default);

/// The named points (markers with a point shape) for the test commands: "NAME (area id)",
/// playfield, position.
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    let c = crate::gamedata::get();
    let mut v: Vec<(String, u32, [f32; 3])> = c
        .areas
        .iter()
        .filter(|(_, a)| a.role == lmoxide_content::AreaRole::Marker)
        .filter_map(|(id, a)| {
            let lmoxide_content::Shape::Point { pos, .. } = a.shape.as_ref()? else { return None };
            let pf: u32 = id.split('/').nth(1)?.parse().ok()?;
            Some((format!("{} ({id})", if a.name.is_empty() { id.as_str() } else { a.name.as_str() }), pf, *pos))
        })
        .collect();
    let saved = SAVED_POINTS.lock().unwrap();
    v.retain(|p| !saved.iter().any(|s| s.0 == p.0));
    v.extend(saved.iter().cloned());
    v.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// `!point <name>`: a named point (marker) at `pos` facing `heading` in world `pf`'s content
/// (its areas.json; the one of that name there is moved), tagged `review` for the world
/// editor. What was saved.
pub fn save_point(pf: u32, name: &str, pos: [f32; 3], heading: f32, author: &str) -> Result<String, String> {
    let dir = crate::gamedata::loaded_from().unwrap_or_else(|| crate::gamedata::dir(None));
    let mut work = Work::open(&dir).map_err(|e| format!("{}: {e:#}", dir.display()))?;
    let existing = work
        .shown_in_world(RecordKind::Area, pf)
        .into_iter()
        .find(|k| work.get(k).is_some_and(|e| e.value.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(name))))
        .map(|k| k.id.to_string());
    let id = existing.unwrap_or_else(|| work.new_id("area", name, Some(pf)));
    let round = |x: f32| (x * 100.0).round() / 100.0;
    let record = json!({
        "id": id, "name": name, "role": "marker",
        "shape": { "point": { "pos": [round(pos[0]), round(pos[1]), round(pos[2])], "rot": (heading * 1000.0).round() / 1000.0 } },
        "source": "surveyed", "tags": ["review"],
        "comment": "Saved in game with !point.",
    });
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    work.put(record, author, now).map_err(|e| format!("{e:?}"))?;
    let label = format!("{name} ({id})");
    let mut saved = SAVED_POINTS.lock().unwrap();
    saved.retain(|s| s.0 != label);
    saved.push((label, pf, pos));
    info!(playfield = pf, point = %id, ?pos, heading, "named point saved");
    Ok(format!("saved {id} at {:.2} {:.2} {:.2}, facing {:.0}° (0 north, 90 east); in the content from the next start", pos[0], pos[1], pos[2], heading.to_degrees()))
}

// ------------------------------------------------------------------ control routes

pub fn routes() -> Router {
    Router::new()
        .route("/v1/content/state", get(state))
        .route("/v1/content/push", post(push))
        .route("/v1/content/branches", get(branches))
        .route("/v1/content/branches/{owner}/{name}", get(branch))
        .route("/v1/content/branches/{owner}/{name}/merge", post(merge))
        .route("/v1/content/branches/{owner}/{name}/reject", post(reject))
}

struct Error(StatusCode, String, Vec<String>);

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let mut body = json!({ "code": "content", "message": self.1 });
        if !self.2.is_empty() {
            body["records"] = json!(self.2);
        }
        (self.0, Json(json!({ "error": body }))).into_response()
    }
}

impl From<ReviewError> for Error {
    fn from(e: ReviewError) -> Self {
        match e {
            ReviewError::Off => Error(StatusCode::NOT_FOUND, "this server has no world editor content (a working copy, --editor-content-dir)".into(), Vec::new()),
            ReviewError::NotFound(m) => Error(StatusCode::NOT_FOUND, m, Vec::new()),
            ReviewError::Refused(m) => Error(StatusCode::CONFLICT, m, Vec::new()),
            ReviewError::Conflict(m, ids) => Error(StatusCode::CONFLICT, format!("{m}: {}", ids.join(", ")), ids),
            ReviewError::Internal(m) => Error(StatusCode::INTERNAL_SERVER_ERROR, m, Vec::new()),
        }
    }
}

/// The content this server runs, how many contributor branches wait for review, how many merged
/// commits the push target doesn't have yet (null: unknown), and that target.
async fn state() -> Json<Value> {
    let review = crate::editor::review(|ed| Ok(crate::editor::review_state(ed))).await.ok();
    Json(json!({
        "content": crate::gamedata::loaded_from().map(|d| std::fs::canonicalize(&d).unwrap_or(d)),
        "pending": review.as_ref().map(|r| r.0),
        "unpushed": review.as_ref().and_then(|r| r.1),
        "push_target": review.map(|r| r.2),
    }))
}

/// Pushes the merged branches to the push target (default: the content this server plays,
/// fast-forwarded; play servers take it in at their next start). `{target, commits, from, to}`.
async fn push(headers: HeaderMap) -> Result<Json<lmoxide_content::contrib::Pushed>, Error> {
    let by = crate::survey::author_of(&headers);
    Ok(Json(crate::editor::review(move |ed| crate::editor::push(ed, &by)).await?))
}

/// Every contributor branch: open ones (pending, rejected, open), then closed ones (merged,
/// reset, deleted).
async fn branches() -> Result<Json<Vec<Branch>>, Error> {
    Ok(Json(crate::editor::review(crate::editor::branches).await?))
}

async fn branch(UrlPath((owner, name)): UrlPath<(String, String)>) -> Result<Json<Branch>, Error> {
    let want = format!("contrib/{owner}/{name}");
    let all = crate::editor::review(crate::editor::branches).await?;
    all.into_iter().find(|b| b.branch == want && b.closed.is_none()).map(Json).ok_or_else(|| Error(StatusCode::NOT_FOUND, format!("no branch {want}"), Vec::new()))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MergeRequest {
    /// The merge commit's message (default: the branch's commits').
    message: Option<String>,
    /// Put the branch's versions over records the content changed since it started.
    force: bool,
}

/// Merges a branch into the content (the working copy's `main`) as one commit by its editor,
/// with `Approved-By:` the caller; the branch starts again from there. 409 with `records` when
/// the content changed some of its records since (`force` to merge anyway).
async fn merge(headers: HeaderMap, UrlPath((owner, name)): UrlPath<(String, String)>, body: Option<Json<MergeRequest>>) -> Result<Json<Value>, Error> {
    let by = crate::survey::author_of(&headers);
    let r = body.map(|b| b.0).unwrap_or_default();
    let branch = format!("contrib/{owner}/{name}");
    let (b, merged) = crate::editor::review(move |ed| crate::editor::merge_branch(ed, &branch, &by, r.message, r.force)).await?;
    Ok(Json(json!({ "merged": merged, "branch": b })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Rejection {
    reason: Option<String>,
}

/// Rejects a branch with a reason: rejected until its editor commits to it again.
async fn reject(headers: HeaderMap, UrlPath((owner, name)): UrlPath<(String, String)>, body: Option<Json<Rejection>>) -> Result<Json<Branch>, Error> {
    let by = crate::survey::author_of(&headers);
    let reason = body.and_then(|b| b.0.reason).filter(|r| !r.trim().is_empty());
    let branch = format!("contrib/{owner}/{name}");
    Ok(Json(crate::editor::review(move |ed| crate::editor::reject_branch(ed, &branch, &by, reason)).await?))
}
