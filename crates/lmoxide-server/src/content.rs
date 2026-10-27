//! World content on play servers (docs/WORLD-CONTENT.md): what the world editor's editors
//! published (`<--world-editor-set>/published`; the working set beside it isn't read) becomes part of this server's worlds once it is
//! approved. This server keeps its own approved copy and rules in `<data-dir>/world-content/`:
//!
//! - `approved/`: the approved set (a survey set folder);
//! - `rules.json`: which features are active (spawned) and which changes are approved without
//!   asking (by feature or category, and by author);
//! - `rejected.json`: changes an admin rejected (by record id and its version), kept out of the
//!   queue until the record changes again.
//!
//! Every few seconds the working set is read again when its files changed; changes the rules
//! approve are taken, the rest wait in the queue for an admin (`/v1/content` on the control
//! interface, through lmoxide-api). Playfields whose approved, active content changed are
//! spawned again (live reload): their base content with the placements applied, as the survey
//! server shows it (placements of script waves and trigger walls aren't run on play servers yet).
//!
//! Locks: `STATE`, then `npc::NPCS` (spawning and despawning).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use lmoxide_net::types::Id32;
use lmoxide_survey::{
    features,
    review::{self, Change, ChangeKey, SummaryRow},
    set::Set,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::npc::{self, SpawnDef};

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: LazyLock<Mutex<Option<State>>> = LazyLock::new(|| Mutex::new(None));

/// How often the working set's folder is checked for changes.
const POLL: Duration = Duration::from_secs(10);

/// Which approved content is spawned, and which changes are approved without an admin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    /// Features spawned (a feature is on when it or an ancestor is listed): `live` mimics live;
    /// e.g. `feature/christmas-event/2026` for that event only while it runs.
    #[serde(default = "default_active")]
    pub active: Vec<String>,
    /// Changes to these features (or what is nested under them, e.g. the category `live`) are
    /// approved as they come.
    #[serde(default)]
    pub auto_features: Vec<String>,
    /// Changes by these authors (world editor usernames) are approved as they come.
    #[serde(default)]
    pub auto_authors: Vec<String>,
}

fn default_active() -> Vec<String> {
    vec![features::live()]
}

impl Default for Rules {
    fn default() -> Self {
        Rules { active: default_active(), auto_features: Vec::new(), auto_authors: Vec::new() }
    }
}

impl Rules {
    fn approves(&self, key: &ChangeKey) -> bool {
        self.auto_features.iter().any(|f| features::within(&key.feature, f)) || self.auto_authors.iter().any(|a| a.eq_ignore_ascii_case(&key.author))
    }
}

struct State {
    dir: PathBuf,
    /// The world editor's set folder (none: only what was approved before is spawned).
    source: Option<PathBuf>,
    working: Set,
    /// Newest modification time seen in the source folder (to notice changes).
    stamp: Option<std::time::SystemTime>,
    approved: Set,
    rules: Rules,
    /// record id -> the `updated` of the rejected version
    rejected: HashMap<String, u64>,
    bases: BTreeMap<u32, Vec<SpawnDef>>,
    catalog: Vec<(crate::survey::Placeable, SpawnDef)>,
    /// What this module spawned in each playfield (to spawn it again).
    spawned: HashMap<u32, Vec<Id32>>,
    /// Bumped whenever the queue or the approved set changes.
    revision: u64,
    /// The last error reading the source, for the admin page.
    source_error: Option<String>,
}

/// Whether this play server manages its content here (every server but a survey server).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Content from `<data_dir>/world-content`, with the world editor's set `source` if given.
pub fn init(data_dir: &Path, source: Option<PathBuf>) {
    let dir = data_dir.join("world-content");
    let approved = Set::load(&dir.join("approved")).unwrap_or_else(|e| {
        warn!("world content: approved set: {e}; starting empty");
        Set::default()
    });
    let rules: Rules = read_json(&dir.join("rules.json")).unwrap_or_default();
    let rejected = read_json(&dir.join("rejected.json")).unwrap_or_default();
    info!(dir = %dir.display(), source = ?source, active = ?rules.active, "world content");
    ENABLED.store(true, Ordering::Relaxed);
    *STATE.lock().unwrap() = Some(State {
        dir,
        source,
        working: Set::default(),
        stamp: None,
        approved,
        rules,
        rejected,
        bases: BTreeMap::new(),
        catalog: Vec::new(),
        spawned: HashMap::new(),
        revision: 1,
        source_error: None,
    });
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).map_err(|e| warn!(path = %path.display(), "world content: {e}")).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) {
    let r = std::fs::create_dir_all(path.parent().unwrap_or(path)).and_then(|_| {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(value).unwrap_or_default())?;
        std::fs::rename(&tmp, path)
    });
    if let Err(e) = r {
        warn!(path = %path.display(), "world content: not saved: {e}");
    }
}

/// Spawns the base content (`npc::start`'s list) with the approved, active content applied, then
/// watches the world editor's set.
pub fn start(all: Vec<(u32, SpawnDef)>) {
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    for (pf, def) in all {
        st.bases.entry(pf).or_default().push(def);
    }
    st.catalog = crate::survey::catalog_of(&st.bases);
    let pfs: BTreeSet<u32> = st.bases.keys().chain(st.approved.placements.keys()).copied().collect();
    for pf in pfs {
        spawn_playfield(st, pf);
    }
    drop(guard);
    tokio::spawn(watch());
}

/// Spawns playfield `pf` from its base content and the approved, active content (what this
/// module spawned there before goes first).
fn spawn_playfield(st: &mut State, pf: u32) {
    for id in st.spawned.remove(&pf).unwrap_or_default() {
        npc::despawn(id);
    }
    let active = review::only_features(&st.approved, &st.rules.active);
    let placements = active.placements.get(&pf).map(|f| f.placements.clone()).unwrap_or_default();
    let empty = Vec::new();
    let (defs, waves) = crate::survey::applied_defs(pf, st.bases.get(&pf).unwrap_or(&empty), &placements, &st.catalog);
    if waves > 0 {
        info!(playfield = pf, waves, "world content: placements of script waves aren't spawned on play servers yet");
    }
    let mut ids = Vec::with_capacity(defs.len());
    for def in defs {
        let id = npc::new_id(&def);
        npc::spawn_as(id, def, pf);
        ids.push(id);
    }
    st.spawned.insert(pf, ids);
}

/// The newest modification time of the files in `dir`.
fn stamp_of(dir: &Path) -> std::io::Result<Option<std::time::SystemTime>> {
    let mut newest = None;
    for e in std::fs::read_dir(dir)? {
        let t = e?.metadata()?.modified()?;
        newest = newest.max(Some(t));
    }
    Ok(newest)
}

async fn watch() {
    loop {
        tokio::task::spawn_blocking(check).await.ok();
        tokio::time::sleep(POLL).await;
    }
}

/// Reads the working set again if it changed, and approves what the rules approve.
fn check() {
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    // only what the world editor's editors published
    let Some(source) = st.source.as_deref().map(lmoxide_survey::set::published_dir) else { return };
    let stamp = match stamp_of(&source) {
        Ok(s) => s,
        Err(e) => {
            st.source_error = Some(format!("{}: {e}", source.display()));
            return;
        }
    };
    if stamp == st.stamp && st.stamp.is_some() {
        return;
    }
    match Set::load(&source) {
        Ok(set) => {
            st.working = set;
            st.stamp = stamp;
            st.source_error = None;
            st.revision += 1;
        }
        Err(e) => {
            // (a file half written: try again next time)
            st.source_error = Some(e.to_string());
            return;
        }
    }
    let auto: Vec<Change> = queue(st).into_iter().filter(|c| st.rules.approves(&c.key)).collect();
    if !auto.is_empty() {
        let n: usize = auto.iter().map(|c| c.items.len()).sum();
        info!(changes = auto.len(), records = n, "world content: approved by the rules");
        approve(st, &auto, None);
    }
}

/// The working set's changes waiting for approval (rejected versions left out).
fn queue(st: &State) -> Vec<Change> {
    review::changes(&st.working, &st.approved)
        .into_iter()
        .filter_map(|mut c| {
            c.items.retain(|i| st.rejected.get(&i.id) != Some(&i.updated));
            (!c.items.is_empty()).then_some(c)
        })
        .collect()
}

/// Takes these changes (only the records `only` names, if given) into the approved set, saves
/// it and spawns the playfields again whose active content changed.
fn approve(st: &mut State, changes: &[Change], only: Option<&[String]>) -> usize {
    let before = review::only_features(&st.approved, &st.rules.active);
    let mut n = 0;
    for c in changes {
        n += review::approve(&mut st.approved, &st.working, c, only);
    }
    if let Err(e) = st.approved.save(&st.dir.join("approved")) {
        warn!("world content: approved set not saved: {e}");
    }
    st.revision += 1;
    let after = review::only_features(&st.approved, &st.rules.active);
    respawn_changed(st, &before, &after);
    n
}

/// Spawns again the playfields whose placements differ between `before` and `after`.
fn respawn_changed(st: &mut State, before: &Set, after: &Set) {
    let pfs: BTreeSet<u32> = before.placements.keys().chain(after.placements.keys()).copied().collect();
    for pf in pfs {
        let placements = |s: &Set| s.placements.get(&pf).map(|f| f.placements.clone()).unwrap_or_default();
        if placements(before) != placements(after) {
            info!(playfield = pf, "world content: playfield spawned again");
            spawn_playfield(st, pf);
        }
    }
}

// ------------------------------------------------------------------ control routes

pub fn routes() -> Router {
    Router::new()
        .route("/v1/content/state", get(state))
        .route("/v1/content/queue", get(queue_route))
        .route("/v1/content/approve", post(approve_route))
        .route("/v1/content/reject", post(reject_route))
        .route("/v1/content/rules", get(rules).put(save_rules))
        .route("/v1/content/summary", get(summary))
        .route("/v1/content/features", get(features_route))
        .route("/v1/content/refresh", post(refresh))
}

struct Error(StatusCode, String);

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": { "code": "content", "message": self.1 } }))).into_response()
    }
}

fn with<T>(f: impl FnOnce(&mut State) -> Result<T, Error>) -> Result<T, Error> {
    let mut guard = STATE.lock().unwrap();
    let st = guard.as_mut().ok_or_else(|| Error(StatusCode::NOT_FOUND, "this server doesn't manage world content (a survey server?)".into()))?;
    f(st)
}

fn pf_name(pf: u32) -> Option<String> {
    crate::teleport::destination(pf).map(|d| d.name.clone())
}

#[derive(Serialize)]
struct StateView {
    source: Option<String>,
    source_error: Option<String>,
    rules: Rules,
    revision: u64,
    /// changes waiting, records in them
    pending: usize,
    pending_records: usize,
}

async fn state() -> Result<Json<StateView>, Error> {
    with(|st| {
        let q = queue(st);
        Ok(Json(StateView {
            source: st.source.as_ref().map(|p| p.display().to_string()),
            source_error: st.source_error.clone(),
            rules: st.rules.clone(),
            revision: st.revision,
            pending: q.len(),
            pending_records: q.iter().map(|c| c.items.len()).sum(),
        }))
    })
}

#[derive(Serialize)]
struct QueuedChange {
    #[serde(flatten)]
    change: Change,
    playfield_name: Option<String>,
    /// "New content > Christmas Event > 2026"
    feature_path: String,
    /// whether the feature is spawned once approved
    active: bool,
}

async fn queue_route() -> Result<Json<Vec<QueuedChange>>, Error> {
    with(|st| {
        Ok(Json(
            queue(st)
                .into_iter()
                .map(|c| QueuedChange {
                    playfield_name: pf_name(c.key.playfield),
                    feature_path: st.working.features.path_title(&c.key.feature),
                    active: st.rules.active.iter().any(|a| features::within(&c.key.feature, a)),
                    change: c,
                })
                .collect(),
        ))
    })
}

#[derive(Deserialize)]
struct Decision {
    #[serde(flatten)]
    key: ChangeKey,
    /// Only these records of the change (default all).
    #[serde(default)]
    ids: Option<Vec<String>>,
}

#[derive(Serialize)]
struct Decided {
    records: usize,
    revision: u64,
}

fn find(st: &State, key: &ChangeKey) -> Result<Change, Error> {
    queue(st).into_iter().find(|c| &c.key == key).ok_or_else(|| Error(StatusCode::NOT_FOUND, "no such change waiting (approved or changed meanwhile?)".into()))
}

async fn approve_route(Json(d): Json<Decision>) -> Result<Json<Decided>, Error> {
    with(|st| {
        let c = find(st, &d.key)?;
        let n = approve(st, std::slice::from_ref(&c), d.ids.as_deref());
        info!(playfield = d.key.playfield, feature = %d.key.feature, author = %d.key.author, records = n, "world content: approved");
        Ok(Json(Decided { records: n, revision: st.revision }))
    })
}

async fn reject_route(Json(d): Json<Decision>) -> Result<Json<Decided>, Error> {
    with(|st| {
        let c = find(st, &d.key)?;
        let mut n = 0;
        for i in c.items.iter().filter(|i| d.ids.as_ref().is_none_or(|ids| ids.contains(&i.id))) {
            st.rejected.insert(i.id.clone(), i.updated);
            n += 1;
        }
        write_json(&st.dir.join("rejected.json"), &st.rejected);
        st.revision += 1;
        info!(playfield = d.key.playfield, feature = %d.key.feature, author = %d.key.author, records = n, "world content: rejected");
        Ok(Json(Decided { records: n, revision: st.revision }))
    })
}

async fn rules() -> Result<Json<Rules>, Error> {
    with(|st| Ok(Json(st.rules.clone())))
}

/// New rules: what they now approve is approved, and playfields whose active content changed
/// are spawned again.
async fn save_rules(Json(r): Json<Rules>) -> Result<Json<Rules>, Error> {
    with(|st| {
        for f in r.active.iter().chain(&r.auto_features) {
            if !features::valid(f) {
                return Err(Error(StatusCode::BAD_REQUEST, format!("{f} isn't a feature id (live, feature/..., other/...)")));
            }
        }
        let before = review::only_features(&st.approved, &st.rules.active);
        st.rules = r;
        write_json(&st.dir.join("rules.json"), &st.rules);
        let after = review::only_features(&st.approved, &st.rules.active);
        respawn_changed(st, &before, &after);
        let auto: Vec<Change> = queue(st).into_iter().filter(|c| st.rules.approves(&c.key)).collect();
        if !auto.is_empty() {
            approve(st, &auto, None);
        }
        st.revision += 1;
        Ok(Json(st.rules.clone()))
    })
}

#[derive(Deserialize)]
struct SummaryQuery {
    /// `approved` (default) or `working`
    #[serde(default)]
    of: Option<String>,
}

#[derive(Serialize)]
struct SummaryView {
    rows: Vec<SummaryRowView>,
}

#[derive(Serialize)]
struct SummaryRowView {
    #[serde(flatten)]
    row: SummaryRow,
    playfield_name: Option<String>,
    feature_path: String,
    active: bool,
}

/// The whole approved (or working) set at a glance, per playfield and feature.
async fn summary(Query(q): Query<SummaryQuery>) -> Result<Json<SummaryView>, Error> {
    with(|st| {
        let set = if q.of.as_deref() == Some("working") { &st.working } else { &st.approved };
        let rows = review::summary(set)
            .into_iter()
            .map(|row| SummaryRowView {
                playfield_name: pf_name(row.playfield),
                feature_path: st.working.features.path_title(&row.feature),
                active: st.rules.active.iter().any(|a| features::within(&row.feature, a)),
                row,
            })
            .collect();
        Ok(Json(SummaryView { rows }))
    })
}

#[derive(Serialize)]
struct FeatureRow {
    id: String,
    path: String,
    comment: String,
    copied_from: Option<String>,
}

/// The categories and the world editor's features (to pick rules from).
async fn features_route() -> Result<Json<Vec<FeatureRow>>, Error> {
    with(|st| {
        let f = &st.working.features;
        let cats = features::CATEGORIES.iter().map(|(id, title)| FeatureRow { id: id.to_string(), path: title.to_string(), comment: String::new(), copied_from: None });
        let rest = f.features.iter().map(|x| FeatureRow { id: x.id.clone(), path: f.path_title(&x.id), comment: x.comment.clone(), copied_from: x.copied_from.clone() });
        Ok(Json(cats.chain(rest).collect()))
    })
}

/// Reads the world editor's set now instead of within the next seconds.
async fn refresh() -> Result<Json<StateView>, Error> {
    with(|st| {
        st.stamp = None;
        Ok(())
    })?;
    tokio::task::spawn_blocking(check).await.ok();
    state().await
}
