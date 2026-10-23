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
    extract::{Path, Query, RawQuery, State},
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tracing::{debug, info, warn};

use crate::Config;

/// The folder name the client appends to `ControlHttpPatchAddr`/`HttpPatchAddr`.
pub const PATCH_FOLDER: &str = "lmo_live";
/// Patch version of the last live build (matches the shipped client's `LocalConfig.xml`): the
/// md5 of its LMO.exe, prefixed with "x".
const PATCH_VERSION: &str = "xeefee462924c2cb689283928e67e6519";
const PATCH_INFO_CLIENT: &str = include_str!("../../../data/PatchInfoClient.txt");

struct HttpState {
    cfg: Arc<Config>,
    /// Lower-case resource hash -> file in the client's `RDB/` tree.
    rdb: HashMap<String, PathBuf>,
    /// `PatchInfoClient.txt`, with the platform download list hash pointing at `download_list`
    /// and the client file list hashes at `file_list`.
    patch_info: String,
    /// `PatchVersion` for LocalConfig.xml (follows the local LMO.exe, see `rebuild_file_list`).
    patch_version: String,
    /// Content-addressed patch files (`/patch/rdb/full/<md5>`): download lists and the RDB
    /// hash index.
    full: HashMap<String, Vec<u8>>,
    upstream: Option<Upstream>,
}

/// Fallback for files the archived client lacks: Funcom's patch server (still online as of
/// 2026-10) with a local cache, so each file is downloaded at most once.
struct Upstream {
    base: String,
    cache: PathBuf,
    client: reqwest::Client,
}

impl Upstream {
    /// `rel` is relative to `HttpPatchAddr`, e.g. `rdb/res/1c/7/c6c1...`.
    async fn get(&self, rel: &str) -> Option<Vec<u8>> {
        if rel.split('/').any(|p| p == ".." || p.is_empty()) {
            return None;
        }
        let cached = self.cache.join(rel);
        if let Ok(data) = tokio::fs::read(&cached).await {
            return Some(decode_ioz(data));
        }
        let url = format!("{}/{rel}", self.base);
        let resp = match self.client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                warn!(%url, status = %r.status(), "upstream miss");
                return None;
            }
            Err(e) => {
                warn!(%url, "upstream error: {e}");
                return None;
            }
        };
        let data = decode_ioz(resp.bytes().await.ok()?.to_vec());
        info!(%url, len = data.len(), "fetched from upstream");
        if let Some(parent) = cached.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(&cached, &data).await;
        Some(data)
    }
}

/// Funcom's patch server sends resources as `"IOz1" + u32 LE size + zlib`. The client's
/// on-demand loader expects them uncompressed (as stored in the local RDB, where the file name
/// is the md5 of the uncompressed data), so unwrap them here.
fn decode_ioz(data: Vec<u8>) -> Vec<u8> {
    use std::io::Read;
    if data.len() < 8 || &data[..4] != b"IOz1" {
        return data;
    }
    let size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let mut out = Vec::with_capacity(size);
    match flate2::read::ZlibDecoder::new(&data[8..]).read_to_end(&mut out) {
        Ok(_) if out.len() == size => out,
        r => {
            warn!(?r, expected = size, got = out.len(), "bad IOz1 payload, passing through");
            data
        }
    }
}

fn octets(data: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, "application/octet-stream")], Body::from(data)).into_response()
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
    // Client files (LMO.exe etc.) as they are locally, so a patched LMO.exe (tools/patch-client.py)
    // isn't "repaired" by the patcher.
    let mut patch_version = PATCH_VERSION.to_string();
    let mut patch_info = patch_info;
    if let Some((list, exe_md5)) = cfg.client_dir.as_deref().and_then(rebuild_file_list) {
        let root = hex_md5(&list);
        info!(root_hash = %root, exe = %exe_md5, "client file list from local files");
        patch_version = format!("x{exe_md5}");
        patch_info = patch_info
            .lines()
            .map(|l| match l.split_once('=') {
                Some((k @ ("RootHash" | "RootHash-Win32"), _)) => format!("{k}={root}"),
                Some(("PatchVersion", _)) => format!("PatchVersion={patch_version}"),
                Some(("PatchVersion-Win32", _)) => format!("PatchVersion-Win32={exe_md5}"),
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        full.insert(root, list);
    }
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
    let upstream = cfg.upstream.as_ref().map(|base| Upstream {
        base: base.trim_end_matches('/').to_string(),
        cache: cfg.cache_dir.clone().unwrap_or_else(|| PathBuf::from("rdb-cache")),
        client: reqwest::Client::new(),
    });
    if let Some(u) = &upstream {
        info!(base = %u.base, cache = %u.cache.display(), "upstream fallback enabled");
    }
    let state = Arc::new(HttpState { cfg, rdb, patch_info, patch_version, full, upstream });
    let app = Router::new()
        .route("/control/{folder}/LocalConfig.xml", get(local_config))
        .route("/control/{folder}/PatchInfoClient.txt", get(patch_info_client))
        .route("/patch/rdb/res/{a}/{b}/{rest}", get(rdb_resource))
        .route("/patch/rdb/full/{hash}", get(full_file))
        .route("/patch/{*path}", get(patch_file))
        .route("/track", get(track))
        .route("/rest/chat_filter", get(chat_filter))
        .route("/auth/", get(login_page))
        .fallback(fallback)
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("http listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn local_config_xml(cfg: &Config, patch_version: &str) -> String {
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
      <PatchVersion>{patch_version}</PatchVersion>
      <ChatCompression>false</ChatCompression>
    </Client>
    <ResourceSources>HashedFileSystemSource,HTTPSource,FileSystemSource</ResourceSources>
  </Universe>
</Config>
"#
    )
}

async fn local_config(State(s): State<Arc<HttpState>>, Path(folder): Path<String>) -> Response {
    info!(%folder, "LocalConfig.xml");
    ([(header::CONTENT_TYPE, "text/xml")], local_config_xml(&s.cfg, &s.patch_version)).into_response()
}

/// `FileList.bin` (the client file list, `RootHash` = its md5) with the client folder's top-level
/// files (LMO.exe, DLLs) rehashed from disk. Layout: "CFHL", u32 1, u16 count, then entries:
/// u8 type, u16 name length, name; directories (type 1) are followed by u16 count and their
/// entries, files (other types) by u32 size and md5[16]. Returns the list and LMO.exe's md5, or
/// `None` if the list can't be read. Always served when available: after one patch run the
/// local FileList.bin is already ours, and falling back to the shipped RootHash would make the
/// patcher restore the original files.
fn rebuild_file_list(dir: &std::path::Path) -> Option<(Vec<u8>, String)> {
    let mut list = std::fs::read(dir.join("FileList.bin")).ok()?;
    if list.get(..8)? != b"CFHL\x01\0\0\0" {
        return None;
    }
    let u16_at = |l: &[u8], at: usize| l.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    // Skips one entry (recursively for directories), returning the offset after it.
    fn skip(l: &[u8], at: usize) -> Option<usize> {
        let len = u16::from_le_bytes(l.get(at + 1..at + 3)?.try_into().ok()?) as usize;
        let after_name = at + 3 + len;
        if *l.get(at)? == 1 {
            let n = u16::from_le_bytes(l.get(after_name..after_name + 2)?.try_into().ok()?);
            (0..n).try_fold(after_name + 2, |at, _| skip(l, at))
        } else {
            Some(after_name + 20)
        }
    }
    let count = u16_at(&list, 8)?;
    let (mut at, mut exe_md5) = (10, None);
    for _ in 0..count {
        let next = skip(&list, at)?;
        if list[at] != 1 {
            let len = u16_at(&list, at + 1)?;
            let name = String::from_utf8(list[at + 3..at + 3 + len].to_vec()).ok()?;
            let (size_at, md5_at) = (at + 3 + len, at + 7 + len);
            if let Ok(data) = std::fs::read(dir.join(&name)) {
                let digest = {
                    use md5::{Digest, Md5};
                    Md5::digest(&data)
                };
                if name.eq_ignore_ascii_case("LMO.exe") {
                    exe_md5 = Some(hex_md5(&data));
                }
                if list[md5_at..md5_at + 16] != digest[..] {
                    info!(%name, "client file differs from the shipped file list");
                    list[size_at..size_at + 4].copy_from_slice(&(data.len() as u32).to_le_bytes());
                    list[md5_at..md5_at + 16].copy_from_slice(&digest);
                }
            }
        }
        at = next;
    }
    exe_md5.map(|m| (list, m))
}

async fn patch_info_client(State(s): State<Arc<HttpState>>) -> String {
    s.patch_info.clone()
}

async fn rdb_resource(
    State(s): State<Arc<HttpState>>,
    Path((a, b, rest)): Path<(String, String, String)>,
) -> Response {
    let hash = format!("{a}{b}{rest}").to_ascii_lowercase();
    if let Some(p) = s.rdb.get(&hash) {
        return send_file(p.clone()).await;
    }
    if let Some(u) = &s.upstream
        && let Some(data) = u.get(&format!("rdb/res/{a}/{b}/{rest}")).await
    {
        return octets(data);
    }
    warn!(%hash, "RDB resource not found");
    StatusCode::NOT_FOUND.into_response()
}

async fn full_file(State(s): State<Arc<HttpState>>, Path(hash): Path<String>) -> Response {
    match s.full.get(&hash.to_ascii_lowercase()) {
        Some(data) => {
            info!(%hash, len = data.len(), "serving content-addressed patch file");
            ([(header::CONTENT_TYPE, "application/octet-stream")], data.clone()).into_response()
        }
        None => {
            if let Some(u) = &s.upstream
                && let Some(data) = u.get(&format!("rdb/full/{hash}")).await
            {
                return octets(data);
            }
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
    // client/<h[0:2]>/<h[2:]>: content-addressed client files, e.g. our rebuilt FileList.bin
    if let Some((a, b)) = rel.strip_prefix("client/").and_then(|h| h.split_once('/'))
        && let Some(data) = s.full.get(&format!("{a}{b}").to_ascii_lowercase())
    {
        return octets(data.clone());
    }
    if let Some(p) = resolve_ci(dir, rel) {
        return send_file(p).await;
    }
    if let Some(u) = &s.upstream
        && let Some(data) = u.get(&path).await
    {
        return octets(data);
    }
    warn!(%path, "patch file not found");
    StatusCode::NOT_FOUND.into_response()
}

async fn login_page(RawQuery(q): RawQuery) -> Response {
    debug!(query = q.unwrap_or_default(), "login page");
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], include_str!("../../../data/login.html"))
        .into_response()
}

/// The chat filter the client asks before sending a line (`ProjectChatServerInterface::SendMessage`;
/// the URL is patched into LMO.exe by tools/patch-client.py). The reply is flat JSON read by
/// `ParseChatFilterReply`, which doesn't decode escapes: result 0 = send `filtered`, 1/2 =
/// rejected. Nothing is filtered here.
async fn chat_filter(Query(q): Query<HashMap<String, String>>) -> Response {
    let text = q.get("chat_message").cloned().unwrap_or_default();
    info!(from = q.get("from_char").map(String::as_str).unwrap_or(""), %text, "chat filter");
    let text = text.replace(['"', '\\'], "'");
    let body = format!(r#"{{"result":"0","filtered":"{text}","hash":"x"}}"#);
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
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
    fn ioz1_roundtrip() {
        use std::io::Write;
        let raw = b"hello hello hello hello".to_vec();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&raw).unwrap();
        let mut packed = b"IOz1".to_vec();
        packed.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        packed.extend_from_slice(&z.finish().unwrap());
        assert_eq!(decode_ioz(packed), raw);
        assert_eq!(decode_ioz(b"plain".to_vec()), b"plain");
    }

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
