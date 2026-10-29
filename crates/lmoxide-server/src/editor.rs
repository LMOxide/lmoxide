//! The world editor's content editing API on survey servers (`/v1/editor/...`,
//! docs/WORLD-EDITOR-CONTENT.md): the content tree's records, edited in a git working copy of
//! lmoxide-content (`--content-dir`, `var/content-work`), with the schemas the editor builds
//! its forms from and the client catalogs its pickers use. Saves merge with what others saved
//! meanwhile; nothing is committed here (publishing and approval do that).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Json, Router,
    extract::{Path as UrlPath, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use lmoxide_content::work::{EditError, Key, NewRecord, Work};
use lmoxide_content::{Options, Severity, git, kind_of, schema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{info, warn};

struct Editor {
    work: Work,
    /// The client data folder (looks, effects, stats).
    reference: PathBuf,
    /// The whole tree's problems by id, from the last check (none: changed since).
    problems: Option<Arc<Problems>>,
}

#[derive(Default)]
struct Problems {
    by_id: HashMap<String, Vec<ProblemView>>,
    all: Vec<ProblemView>,
}

#[derive(Clone, Serialize)]
struct ProblemView {
    severity: &'static str,
    id: Option<String>,
    file: Option<String>,
    message: String,
}

static EDITOR: LazyLock<Mutex<Option<Editor>>> = LazyLock::new(|| Mutex::new(None));

/// Opens the working copy at `dir` (survey servers).
pub fn init(dir: &Path) {
    match Work::open(dir) {
        Ok(work) => {
            for n in &work.notes {
                warn!("content work copy: {n}");
            }
            if !git::is_repo(dir) {
                warn!("content work copy {} isn't a git checkout: no unpublished changes or history", dir.display());
            }
            info!(dir = %dir.display(), records = work.records().count(), "content editing");
            let reference = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data"));
            *EDITOR.lock().unwrap() = Some(Editor { work, reference, problems: None });
        }
        Err(e) => warn!("content work copy {}: {e:#}; the world editor can't edit content", dir.display()),
    }
}

// ------------------------------------------------------------------ errors

enum Error {
    Off,
    NotFound(String),
    Bad(String),
    Edit(EditError),
    Internal(String),
}

impl From<EditError> for Error {
    fn from(e: EditError) -> Self {
        Error::Edit(e)
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Internal(format!("{e:#}"))
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Error::Off => (StatusCode::NOT_FOUND, json!({ "code": "not_found", "message": "this server doesn't edit content (survey servers with a content working copy do)" })),
            Error::NotFound(m) => (StatusCode::NOT_FOUND, json!({ "code": "not_found", "message": m })),
            Error::Bad(m) => (StatusCode::BAD_REQUEST, json!({ "code": "bad_request", "message": m })),
            Error::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, json!({ "code": "internal", "message": m })),
            Error::Edit(e) => {
                let message = e.to_string();
                match e {
                    EditError::NotFound(m) => (StatusCode::NOT_FOUND, json!({ "code": "not_found", "message": m })),
                    EditError::Invalid(problems) => (StatusCode::UNPROCESSABLE_ENTITY, json!({ "code": "invalid", "message": message, "problems": problems })),
                    EditError::Conflicts(conflicts) => (StatusCode::CONFLICT, json!({ "code": "conflict", "message": message, "conflicts": conflicts })),
                    EditError::Referenced(used_by) => (StatusCode::CONFLICT, json!({ "code": "referenced", "message": message, "used_by": used_by })),
                    EditError::Io(m) => (StatusCode::INTERNAL_SERVER_ERROR, json!({ "code": "internal", "message": m })),
                }
            }
        };
        (status, Json(json!({ "error": body }))).into_response()
    }
}

type R<T> = Result<Json<T>, Error>;

/// Runs `f` on the editor, off the async threads (reads files, runs git).
async fn with<T: Send + 'static>(f: impl FnOnce(&mut Editor) -> Result<T, Error> + Send + 'static) -> R<T> {
    tokio::task::spawn_blocking(move || {
        let mut guard = EDITOR.lock().unwrap();
        let ed = guard.as_mut().ok_or(Error::Off)?;
        f(ed).map(Json)
    })
    .await
    .map_err(|e| Error::Internal(e.to_string()))?
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn key_of(q: &KeyQuery) -> Result<Key, Error> {
    Key::parse(&q.id, q.feature.clone().filter(|f| !f.is_empty()).unwrap_or_else(|| "live".into())).map_err(Error::Bad)
}

/// The tree's problems, checking it again when it changed since (about a second).
fn problems(ed: &mut Editor) -> Arc<Problems> {
    if let Some(p) = &ed.problems {
        return p.clone();
    }
    let options = Options { check_client_ids: false, ..Default::default() };
    let (_, list) = lmoxide_content::load(ed.work.dir(), &options);
    let mut out = Problems::default();
    for p in list {
        let view = ProblemView {
            severity: if p.severity == Severity::Error { "error" } else { "warning" },
            id: p.id.clone(),
            file: p.file.as_ref().map(|f| f.strip_prefix(ed.work.dir()).unwrap_or(f).display().to_string()),
            message: p.message,
        };
        if let Some(id) = &p.id {
            out.by_id.entry(id.clone()).or_default().push(view.clone());
        }
        out.all.push(view);
    }
    let out = Arc::new(out);
    ed.problems = Some(out.clone());
    out
}

/// The publish queue's folder, beside the working copy (`var/content-queue`).
pub(crate) fn queue_dir(work: &Work) -> PathBuf {
    work.dir().with_file_name("content-queue")
}

/// Runs `f` on the working copy (survey.rs: in-game edits). Lock order: the working copy, then
/// survey.rs's state. The tree's problems are checked again when next asked for.
pub(crate) fn work<T>(f: impl FnOnce(&mut Work) -> T) -> Option<T> {
    let mut guard = EDITOR.lock().unwrap();
    let ed = guard.as_mut()?;
    let out = f(&mut ed.work);
    ed.problems = None;
    Some(out)
}

/// After a write: the tree's problems are checked again when next asked for, and the survey
/// world respawns what changed.
fn changed(ed: &mut Editor, keys: &[Key]) {
    ed.problems = None;
    crate::survey::content_changed(&ed.work, keys);
}

// ------------------------------------------------------------------ routes

pub fn routes() -> Router {
    Router::new()
        .route("/v1/editor/kinds", get(kinds))
        .route("/v1/editor/records", get(list).post(create))
        .route("/v1/editor/record", get(record).put(save).delete(delete))
        .route("/v1/editor/history", get(history))
        .route("/v1/editor/problems", get(all_problems))
        .route("/v1/editor/changes", get(changes))
        .route("/v1/editor/catalog/{name}", get(catalog))
        .route("/v1/editor/reload", axum::routing::post(reload))
        .route("/v1/editor/loot/simulate", axum::routing::post(simulate_loot))
        .route("/v1/editor/queue", get(queue))
        .route("/v1/editor/queue/{seq}", axum::routing::delete(withdraw))
}

/// The kinds index and every schema.
async fn kinds() -> R<Value> {
    with(|_| {
        let schemas: serde_json::Map<String, Value> = lmoxide_content::schemas().into_iter().map(|(k, s)| (k.to_string(), s)).collect();
        Ok(json!({ "index": schema::index(), "schemas": schemas }))
    })
    .await
}

#[derive(Deserialize, Default)]
struct ListQuery {
    kind: Option<String>,
    /// A playfield or a world folder (`4200`, `4200-isle-of-yarr`).
    world: Option<String>,
    feature: Option<String>,
    tag: Option<String>,
    source: Option<String>,
    author: Option<String>,
    /// Words in the id or name.
    q: Option<String>,
    /// Only unpublished ones.
    #[serde(default)]
    changed: bool,
    /// Only ones with problems.
    #[serde(default)]
    problems: bool,
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
}

#[derive(Serialize)]
struct Summary {
    id: String,
    feature: String,
    kind: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    world: Option<String>,
    file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<Value>,
    #[serde(skip_serializing_if = "String::is_empty")]
    author: String,
    rev: u64,
    /// The kind's list fields (schemas/index.json).
    fields: BTreeMap<&'static str, Value>,
    problems: usize,
    changed: bool,
}

#[derive(Serialize)]
struct List {
    total: usize,
    records: Vec<Summary>,
}

fn str_field(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The unpublished changes' keys (empty without git).
fn changed_keys(work: &Work) -> HashSet<Key> {
    git::changes(work).map(|c| c.into_iter().map(|c| c.key).collect()).unwrap_or_default()
}

async fn list(Query(q): Query<ListQuery>) -> R<List> {
    with(move |ed| {
        let problems = problems(ed);
        let changed = changed_keys(&ed.work);
        let words: Vec<String> = q.q.as_deref().unwrap_or("").to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut out = Vec::new();
        for (k, e) in ed.work.records() {
            let kind = k.id.kind.as_str();
            let id = k.id.to_string();
            let v = &e.value;
            let world = ed.work.world_of(k);
            let keep = q.kind.as_deref().is_none_or(|x| x == kind)
                && q.feature.as_deref().is_none_or(|f| f == k.feature)
                && q.world.as_deref().is_none_or(|w| world.as_deref().is_some_and(|x| x == w || x.split('-').next() == Some(w)))
                && q.tag.as_deref().is_none_or(|t| v.get("tags").and_then(Value::as_array).is_some_and(|l| l.iter().any(|x| x.as_str() == Some(t))))
                && q.source.as_deref().is_none_or(|s| v.get("source").and_then(Value::as_str) == Some(s))
                && q.author.as_deref().is_none_or(|a| str_field(v, "author").eq_ignore_ascii_case(a))
                && (!q.changed || changed.contains(k))
                && (!q.problems || problems.by_id.contains_key(&id))
                && words.iter().all(|w| id.to_lowercase().contains(w) || str_field(v, "name").to_lowercase().contains(w) || str_field(v, "title").to_lowercase().contains(w));
            if !keep {
                continue;
            }
            let fields = schema::KIND_INFO
                .iter()
                .find(|i| i.kind == kind)
                .map(|i| i.list.iter().filter_map(|f| v.get(*f).map(|x| (*f, x.clone()))).collect())
                .unwrap_or_default();
            out.push(Summary {
                id: id.clone(),
                feature: k.feature.clone(),
                kind,
                name: str_field(v, "name"),
                world,
                file: e.file.display().to_string(),
                source: v.get("source").cloned(),
                tags: v.get("tags").and_then(Value::as_array).cloned().unwrap_or_default(),
                author: str_field(v, "author"),
                rev: v.get("rev").and_then(Value::as_u64).unwrap_or(0),
                fields,
                problems: problems.by_id.get(&id).map_or(0, Vec::len),
                changed: changed.contains(k),
            });
        }
        let total = out.len();
        let records = out.into_iter().skip(q.offset).take(q.limit.unwrap_or(1000)).collect();
        Ok(List { total, records })
    })
    .await
}

#[derive(Deserialize)]
struct KeyQuery {
    id: String,
    feature: Option<String>,
    /// Features to resolve with (comma-separated; default: the record's own).
    with: Option<String>,
}

/// A record as written, as resolved (with each field's origin), a spawn's template with its
/// overrides, its problems, who uses it, its other features' versions, and how it was at the
/// last commit.
async fn record(Query(q): Query<KeyQuery>) -> R<Value> {
    with(move |ed| {
        let key = key_of(&q)?;
        let id = key.id.to_string();
        let entry = ed.work.get(&key).ok_or_else(|| Error::NotFound(format!("{} ({}) doesn't exist", key.id, key.feature)))?.clone();
        let mut features: Vec<String> = q.with.as_deref().unwrap_or("").split(',').filter(|f| !f.is_empty()).map(str::to_string).collect();
        if key.feature != "live" {
            features.push(key.feature.clone());
        }
        let (resolved, origin) = ed.work.resolve(&id, &features).unwrap_or_default();
        let template = match (kind_of(&id), resolved.get("template").and_then(Value::as_str)) {
            (Some("spawn"), Some(t)) => ed.work.resolve(t, &features).map(|(mut t, _)| {
                if let Some(o) = resolved.get("overrides") {
                    lmoxide_content::load::merge(&mut t, o);
                }
                t
            }),
            _ => None,
        };
        let mut problems: Vec<ProblemView> = problems(ed).by_id.get(&id).cloned().unwrap_or_default();
        // its own check too (what the whole tree's check found before its last save may be gone)
        for m in ed.work.check(&entry.value) {
            if !problems.iter().any(|x| x.message == m) {
                problems.push(ProblemView { severity: "error", id: Some(id.clone()), file: None, message: m });
            }
        }
        let committed = git::file_at(ed.work.dir(), "HEAD", &entry.file).and_then(|t| git::records_of(&t).remove(&key));
        let versions: Vec<&String> = ed.work.versions(&id).into_iter().map(|(k, _)| &k.feature).collect();
        Ok(json!({
            "id": key.id,
            "feature": key.feature,
            "file": entry.file,
            "world": ed.work.world_of(&key),
            "record": entry.value,
            "resolved": resolved,
            "origin": origin,
            "template": template,
            "problems": problems,
            "used_by": ed.work.used_by(&id),
            "versions": versions,
            "committed": committed,
            "changed": committed.as_ref() != Some(&entry.value),
        }))
    })
    .await
}

#[derive(Deserialize)]
struct SaveBody {
    record: Value,
    /// The version the editor started from (needed when it was saved since: the edits merge).
    base: Option<Value>,
}

async fn save(headers: HeaderMap, Query(q): Query<KeyQuery>, Json(body): Json<SaveBody>) -> R<Value> {
    let author = crate::survey::author_of(&headers);
    with(move |ed| {
        let key = key_of(&q)?;
        let record = ed.work.save(&key, body.record, body.base.as_ref(), &author, now())?;
        changed(ed, std::slice::from_ref(&key));
        Ok(json!({ "record": record }))
    })
    .await
}

async fn create(headers: HeaderMap, Json(new): Json<NewRecord>) -> R<Value> {
    let author = crate::survey::author_of(&headers);
    with(move |ed| {
        let (key, record) = ed.work.create(new, &author, now())?;
        changed(ed, std::slice::from_ref(&key));
        Ok(json!({ "id": key.id, "feature": key.feature, "record": record }))
    })
    .await
}

#[derive(Deserialize)]
struct DeleteQuery {
    id: String,
    feature: Option<String>,
    replace_with: Option<String>,
}

async fn delete(headers: HeaderMap, Query(q): Query<DeleteQuery>) -> R<Value> {
    let author = crate::survey::author_of(&headers);
    with(move |ed| {
        let key = key_of(&KeyQuery { id: q.id, feature: q.feature, with: None })?;
        let mut keys = ed.work.delete(&key, q.replace_with.as_deref(), &author, now())?;
        let replaced = keys.clone();
        keys.push(key);
        changed(ed, &keys);
        Ok(json!({ "replaced": replaced }))
    })
    .await
}

#[derive(Deserialize)]
struct HistoryQuery {
    id: String,
    feature: Option<String>,
    limit: Option<usize>,
}

async fn history(Query(q): Query<HistoryQuery>) -> R<Vec<git::Commit>> {
    with(move |ed| {
        let key = key_of(&KeyQuery { id: q.id, feature: q.feature, with: None })?;
        Ok(git::history(&ed.work, &key, q.limit.unwrap_or(50))?)
    })
    .await
}

async fn all_problems() -> R<Value> {
    with(|ed| {
        let p = problems(ed);
        let errors = p.all.iter().filter(|p| p.severity == "error").count();
        Ok(json!({ "errors": errors, "warnings": p.all.len() - errors, "problems": p.all }))
    })
    .await
}

#[derive(Deserialize)]
struct ChangesQuery {
    author: Option<String>,
}

/// The working copy's unpublished changes, per record, with who made them.
async fn changes(Query(q): Query<ChangesQuery>) -> R<Vec<git::Change>> {
    with(move |ed| {
        let list = git::changes(&ed.work)?;
        Ok(list.into_iter().filter(|c| q.author.as_deref().is_none_or(|a| c.author.eq_ignore_ascii_case(a))).collect())
    })
    .await
}

/// What editors published, waiting for approval (newest last).
async fn queue() -> R<Vec<lmoxide_content::queue::Entry>> {
    with(|ed| Ok(lmoxide_content::queue::Queue::new(&queue_dir(&ed.work)).entries())).await
}

/// Takes an entry out of the queue (its changes are unpublished again).
async fn withdraw(UrlPath(seq): UrlPath<u64>) -> R<Value> {
    with(move |ed| {
        let q = lmoxide_content::queue::Queue::new(&queue_dir(&ed.work));
        q.get(seq).ok_or_else(|| Error::NotFound(format!("no queue entry {seq}")))?;
        q.remove(seq).map_err(|e| Error::Internal(e.to_string()))?;
        Ok(json!({ "withdrawn": seq }))
    })
    .await
}

#[derive(Deserialize)]
struct SimulateQuery {
    id: String,
    rolls: Option<u32>,
    /// The playfield it drops in (entries limited to worlds); none: no world's.
    playfield: Option<u32>,
    /// Features on (comma-separated).
    with: Option<String>,
    /// The loot power level it rolls at (tunables `loot.power`); none: 1000.
    power: Option<u32>,
}

/// Rolls a loot table of the working copy many times (as saved, nested tables too): how often
/// each thing drops.
async fn simulate_loot(Query(q): Query<SimulateQuery>) -> R<lmoxide_content::roll::Simulation> {
    with(move |ed| {
        let features: Vec<String> = q.with.as_deref().unwrap_or("").split(',').filter(|f| !f.is_empty()).map(str::to_string).collect();
        let work = &ed.work;
        let table = |id: &str| work.resolve(id, &features).and_then(|(v, _)| serde_json::from_value::<lmoxide_content::LootTable>(v).ok());
        let in_worlds = |worlds: &[String]| {
            q.playfield.is_some_and(|pf| worlds.iter().any(|w| work.resolve(w, &features).is_some_and(|(v, _)| v.get("playfield").and_then(Value::as_u64) == Some(pf as u64))))
        };
        if table(&q.id).is_none() {
            return Err(Error::NotFound(format!("{} isn't a loot table that loads", q.id)));
        }
        let rules = &crate::gamedata::get().tunables.loot.power;
        Ok(lmoxide_content::roll::simulate(&table, &in_worlds, &q.id, q.rolls.unwrap_or(10_000).min(200_000), q.power.unwrap_or(1000), rules))
    })
    .await
}

/// Reads the working copy again (after it changed on disk: a git pull, an edit by hand).
async fn reload() -> R<Value> {
    with(|ed| {
        let work = Work::open(ed.work.dir())?;
        let keys: Vec<Key> = work.records().map(|(k, _)| k.clone()).collect();
        ed.work = work;
        changed(ed, &keys);
        Ok(json!({ "records": keys.len(), "notes": ed.work.notes }))
    })
    .await
}

#[derive(Deserialize)]
struct CatalogQuery {
    /// Words in the name, or the id.
    q: Option<String>,
    /// These ids only (comma-separated).
    ids: Option<String>,
    limit: Option<usize>,
}

async fn catalog(UrlPath(name): UrlPath<String>, Query(q): Query<CatalogQuery>) -> R<Value> {
    with(move |ed| {
        let list = lmoxide_content::catalog::catalog(&name, &ed.work, &ed.reference).ok_or_else(|| Error::Bad(format!("no catalog {name:?} (there are {})", lmoxide_content::catalog::CATALOGS.join(", "))))?;
        let ids: Option<HashSet<u64>> = q.ids.as_deref().map(|s| s.split(',').filter_map(|i| i.trim().parse().ok()).collect());
        let words: Vec<String> = q.q.as_deref().unwrap_or("").to_lowercase().split_whitespace().map(str::to_string).collect();
        let hits: Vec<_> = list
            .into_iter()
            .filter(|e| ids.as_ref().is_none_or(|ids| ids.contains(&e.id)))
            .filter(|e| words.iter().all(|w| e.name.to_lowercase().contains(w) || e.id.to_string() == *w || e.record.as_deref().is_some_and(|r| r.contains(w))))
            .collect();
        let total = hits.len();
        let entries: Vec<_> = hits.into_iter().take(q.limit.unwrap_or(200)).collect();
        Ok(json!({ "total": total, "entries": entries }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_errors_map_to_statuses() {
        let status = |e: EditError| Error::Edit(e).into_response().status();
        assert_eq!(status(EditError::Invalid(vec![])), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(status(EditError::Conflicts(vec![])), StatusCode::CONFLICT);
        assert_eq!(status(EditError::Referenced(vec![])), StatusCode::CONFLICT);
        assert_eq!(status(EditError::NotFound(String::new())), StatusCode::NOT_FOUND);
    }
}
