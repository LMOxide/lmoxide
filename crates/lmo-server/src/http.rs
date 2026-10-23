//! HTTP services: control (LocalConfig.xml, PatchInfoClient.txt), patch (client files and
//! RDB resources) and the funnel tracker.
//!
//! URL layout, relative to `http://<public_host>:<http_port>`:
//! - `/control/<folder>/LocalConfig.xml`, `/control/<folder>/PatchInfoClient.txt`
//! - `/patch/...` - files from the installed client
//! - `/track?category=...` - analytics pings, logged and ignored

use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{Path, RawQuery, State},
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tracing::{debug, info, warn};

use crate::Config;

/// The folder name the client appends to `ControlHttpPatchAddr`/`HttpPatchAddr`.
pub const PATCH_FOLDER: &str = "lmo_live";
/// Patch version of the last live build (matches the shipped client's `LocalConfig.xml`).
const PATCH_VERSION: &str = "xeefee462924c2cb689283928e67e6519";
const PATCH_INFO_CLIENT: &str = include_str!("../../../data/PatchInfoClient.txt");

struct HttpState {
    cfg: Arc<Config>,
    /// Lower-case resource hash -> file in the client's `RDB/` tree.
    rdb: HashMap<String, PathBuf>,
    /// `PatchInfoClient.txt`, with the platform download list hash pointing at `download_list`.
    patch_info: String,
    /// Content-addressed patch files (`/patch/rdb/full/<md5>`): download lists and the RDB
    /// hash index.
    full: HashMap<String, Vec<u8>>,
}

/// Download list the Windows client patches against (see `PatchInfoClient.txt`).
const PLATFORM_DOWNLOAD_LIST: &str = "DownloadList-CLIENT-Windows32-SSE2DXT-modelQ0";

pub async fn serve(cfg: Arc<Config>, addr: SocketAddr) -> anyhow::Result<()> {
    let rdb = match &cfg.client_dir {
        Some(dir) => index_rdb(&dir.join("RDB")),
        None => HashMap::new(),
    };
    info!("indexed {} RDB resources", rdb.len());
    let download_list = cfg.client_dir.as_ref().and_then(|dir| {
        let xml = std::fs::read_to_string(dir.join("RDB/DownloadList-Platform.xml")).ok()?;
        Some(if cfg.no_filter_download_list { xml } else { filter_download_list(&xml, &rdb) })
    });
    let patch_info = match &download_list {
        Some(list) => {
            let md5 = hex_md5(list.as_bytes());
            PATCH_INFO_CLIENT
                .lines()
                .map(|l| match l.split_once('=') {
                    Some((k, _)) if k == PLATFORM_DOWNLOAD_LIST => format!("{k}={md5}"),
                    _ => l.to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        None => PATCH_INFO_CLIENT.to_string(),
    };
    let mut full = HashMap::new();
    if let Some(list) = download_list {
        full.insert(hex_md5(list.as_bytes()), list.into_bytes());
    }
    if let Some(dir) = &cfg.client_dir {
        for f in ["RDB/DownloadList-Client.xml", "RDB/RDBHashIndex.bin"] {
            if let Ok(data) = std::fs::read(dir.join(f)) {
                full.insert(hex_md5(&data), data);
            }
        }
    }
    let state = Arc::new(HttpState { cfg, rdb, patch_info, full });
    let app = Router::new()
        .route("/control/{folder}/LocalConfig.xml", get(local_config))
        .route("/control/{folder}/PatchInfoClient.txt", get(patch_info_client))
        .route("/patch/rdb/res/{a}/{b}/{rest}", get(rdb_resource))
        .route("/patch/rdb/full/{hash}", get(full_file))
        .route("/patch/{*path}", get(patch_file))
        .route("/track", get(track))
        .route("/auth/", get(login_page))
        .fallback(fallback)
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("http listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn local_config_xml(cfg: &Config) -> String {
    let http = cfg.public_addr(cfg.http_port);
    let universe = cfg.public_addr(cfg.universe_port);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Config>
  <Self>
    <ConfigKey>Universe/Client/</ConfigKey>
  </Self>
  <Universe>
    <Client>
      <ClientFileName>LegoDX9.exe</ClientFileName>
      <ClientFileNameDX11>Lego.exe</ClientFileNameDX11>
      <HttpPatchFolder>{PATCH_FOLDER}</HttpPatchFolder>
      <HttpPatchAddr>http://{http}/patch</HttpPatchAddr>
      <ControlHttpPatchAddr>{http}/control</ControlHttpPatchAddr>
      <LoginURL>http://{http}/auth/</LoginURL>
      <UniverseAddr>{universe}</UniverseAddr>
      <FunnelTrackerAddr>http://{http}/track</FunnelTrackerAddr>
      <PatchVersion>{PATCH_VERSION}</PatchVersion>
    </Client>
    <ResourceSources>HashedFileSystemSource,HTTPSource,FileSystemSource</ResourceSources>
  </Universe>
</Config>
"#
    )
}

async fn local_config(State(s): State<Arc<HttpState>>, Path(folder): Path<String>) -> Response {
    info!(%folder, "LocalConfig.xml");
    ([(header::CONTENT_TYPE, "text/xml")], local_config_xml(&s.cfg)).into_response()
}

async fn patch_info_client(State(s): State<Arc<HttpState>>) -> String {
    s.patch_info.clone()
}

async fn rdb_resource(
    State(s): State<Arc<HttpState>>,
    Path((a, b, rest)): Path<(String, String, String)>,
) -> Response {
    let hash = format!("{a}{b}{rest}").to_ascii_lowercase();
    match s.rdb.get(&hash) {
        Some(p) => send_file(p.clone()).await,
        None => {
            warn!(%hash, "RDB resource not found");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn full_file(State(s): State<Arc<HttpState>>, Path(hash): Path<String>) -> Response {
    match s.full.get(&hash.to_ascii_lowercase()) {
        Some(data) => {
            info!(%hash, len = data.len(), "serving content-addressed patch file");
            ([(header::CONTENT_TYPE, "application/octet-stream")], data.clone()).into_response()
        }
        None => {
            warn!(%hash, "content-addressed patch file not found");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn patch_file(State(s): State<Arc<HttpState>>, Path(path): Path<String>) -> Response {
    let Some(dir) = &s.cfg.client_dir else {
        warn!(%path, "patch request but no --client-dir");
        return StatusCode::NOT_FOUND.into_response();
    };
    // Requests may be prefixed with the patch folder; resolve case-insensitively.
    let rel = path.strip_prefix(&format!("{PATCH_FOLDER}/")).unwrap_or(&path);
    match resolve_ci(dir, rel) {
        Some(p) => send_file(p).await,
        None => {
            warn!(%path, "patch file not found");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn login_page(RawQuery(q): RawQuery) -> Response {
    debug!(query = q.unwrap_or_default(), "login page");
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], include_str!("../../../data/login.html"))
        .into_response()
}

async fn track(RawQuery(q): RawQuery) -> &'static str {
    debug!(query = q.unwrap_or_default(), "track");
    "OK"
}

async fn fallback(uri: Uri) -> StatusCode {
    warn!(%uri, "unhandled http request");
    StatusCode::NOT_FOUND
}

async fn send_file(p: PathBuf) -> Response {
    match tokio::fs::read(&p).await {
        Ok(data) => {
            debug!(path = %p.display(), len = data.len(), "serving");
            ([(header::CONTENT_TYPE, "application/octet-stream")], Body::from(data))
                .into_response()
        }
        Err(e) => {
            warn!(path = %p.display(), "read failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Drops `<Hash>` entries for resources missing from the local RDB (the archived client is
/// incomplete), so the patcher doesn't fail trying to download them.
///
/// The client rejects the list unless the counters are consistent, so they're recomputed:
/// per `<ResourceList>`, `ResourceCount` = entries, `DownloadSize` = sum of `size`, and
/// `SubstanceCount` = entries of rdbtype 1010061; on `<DownloadList>`, `ResourceCount` and
/// `DownloadSize` count each unique hash once.
fn filter_download_list(xml: &str, rdb: &HashMap<String, PathBuf>) -> String {
    const SUBSTANCE_TYPE: &str = "1010061";
    struct Entry<'a> {
        line: &'a str,
        hash: String,
        size: u64,
        substance: bool,
    }
    fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let i = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        Some(&tag[i..i + tag[i..].find('"')?])
    }
    fn set_attr(tag: &str, name: &str, value: u64) -> String {
        match attr(tag, name) {
            Some(old) => tag.replacen(&format!(" {name}=\"{old}\""), &format!(" {name}=\"{value}\""), 1),
            None => tag.to_string(),
        }
    }

    let mut root: Option<&str> = None;
    let mut lists: Vec<(&str, Vec<Entry>)> = Vec::new();
    let mut dropped = 0usize;
    for line in xml.lines() {
        let t = line.trim_start();
        if t.starts_with("<DownloadList ") {
            root = Some(line);
        } else if t.starts_with("<ResourceList ") {
            lists.push((line, Vec::new()));
        } else if t.starts_with("<Hash ") {
            let hash = t
                .split_once('>')
                .and_then(|(_, h)| h.split_once('<'))
                .map(|(h, _)| h.to_ascii_lowercase())
                .unwrap_or_default();
            if !rdb.contains_key(&hash) {
                dropped += 1;
                continue;
            }
            let entry = Entry {
                line,
                size: attr(t, "size").and_then(|v| v.parse().ok()).unwrap_or(0),
                substance: attr(t, "rdbtype") == Some(SUBSTANCE_TYPE),
                hash,
            };
            if let Some((_, entries)) = lists.last_mut() {
                entries.push(entry);
            }
        }
    }

    let mut unique: HashMap<&str, u64> = HashMap::new();
    let mut out = vec![r#"<?xml version="1.0" ?>"#.to_string()];
    let mut body = Vec::new();
    for (header, entries) in &lists {
        let size: u64 = entries.iter().map(|e| e.size).sum();
        let substances = entries.iter().filter(|e| e.substance).count() as u64;
        let h = set_attr(header, "DownloadSize", size);
        let h = set_attr(&h, "ResourceCount", entries.len() as u64);
        body.push(set_attr(&h, "SubstanceCount", substances));
        for e in entries {
            unique.insert(&e.hash, e.size);
            body.push(e.line.to_string());
        }
        body.push("\t</ResourceList>".to_string());
    }
    let root = root.unwrap_or(r#"<DownloadList DownloadSize="0" ResourceCount="0">"#);
    let root = set_attr(root, "DownloadSize", unique.values().sum());
    out.push(set_attr(&root, "ResourceCount", unique.len() as u64));
    out.extend(body);
    out.push("</DownloadList>".to_string());
    info!(kept = unique.len(), dropped, "filtered platform download list");
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_recomputes_counts() {
        let xml = r#"<?xml version="1.0" ?>
<DownloadList DownloadSize="6" Name="X" ResourceCount="3">
	<ResourceList DownloadSize="6" Order="0" ResourceCount="3" SubstanceCount="1">
		<Hash rdbid="1" rdbtype="1010061" size="1">aa</Hash>
		<Hash rdbid="2" rdbtype="5" size="2">bb</Hash>
		<Hash rdbid="3" rdbtype="5" size="3">cc</Hash>
	</ResourceList>
</DownloadList>
"#;
        let unchanged: HashMap<String, PathBuf> =
            ["aa", "bb", "cc"].iter().map(|h| (h.to_string(), PathBuf::new())).collect();
        assert_eq!(filter_download_list(xml, &unchanged), xml);

        let partial: HashMap<String, PathBuf> =
            ["bb"].iter().map(|h| (h.to_string(), PathBuf::new())).collect();
        let out = filter_download_list(xml, &partial);
        assert!(out.contains(r#"<DownloadList DownloadSize="2" Name="X" ResourceCount="1">"#));
        assert!(out.contains(r#"DownloadSize="2" Order="0" ResourceCount="1" SubstanceCount="0""#));
        assert!(!out.contains(">aa<"));
    }
}

fn hex_md5(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    Md5::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Resolves `rel` (slash-separated) under `root`, matching each component case-insensitively.
fn resolve_ci(root: &std::path::Path, rel: &str) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        if part == ".." {
            return None;
        }
        let exact = cur.join(part);
        if exact.exists() {
            cur = exact;
            continue;
        }
        let found = std::fs::read_dir(&cur).ok()?.filter_map(|e| e.ok()).find(|e| {
            e.file_name().to_string_lossy().eq_ignore_ascii_case(part)
        })?;
        cur = found.path();
    }
    cur.is_file().then_some(cur)
}

/// Maps each `RDB/<type>/<HASH>.rdbres` file by its lower-case hash.
fn index_rdb(rdb: &std::path::Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let Ok(types) = std::fs::read_dir(rdb) else { return out };
    for t in types.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(t.path()) else { continue };
        for f in files.filter_map(|e| e.ok()) {
            let p = f.path();
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                out.insert(stem.to_ascii_lowercase(), p);
            }
        }
    }
    out
}
