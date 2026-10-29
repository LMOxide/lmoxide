//! Bringing an install dir in line with a verified [`Manifest`]: [`plan`] compares local files
//! by size and sha256, [`apply`] downloads what differs and deletes what the manifest removes.
//!
//! Safety rules: every path goes through [`crate::check_path`] and [`safe_join`], which also
//! refuses to follow symlinked directories inside the install dir, so a manifest can't make the
//! patcher write anywhere else. Files are downloaded to `.lmoxide/partial/<sha256>` (resumed
//! with HTTP Range after an interruption), checked against the signed hash, and only then
//! renamed over the old file, so a file is never half-written and a broken download never
//! replaces a working one. Files not in the manifest are left alone (logs, user settings),
//! except those it lists under `remove`.
//!
//! Progress goes to a [`ProgressFn`] callback; a GUI can forward it into a channel.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::{StreamExt, TryStreamExt};
use tokio::io::AsyncWriteExt;

use crate::{
    Error, FileEntry, Manifest, Result, STATE_DIR, SignedManifest, check_path,
    client::{Client, stored_key},
    hashcache::{HashCache, sha256_file},
};

/// What the sync engine is doing, for progress displays.
#[derive(Debug, Clone)]
pub enum Progress {
    /// Comparing local files with the manifest; `done` of `total` files checked.
    Checking { done: usize, total: usize },
    /// The plan: what [`apply`] is about to do.
    Planned { download_files: usize, download_bytes: u64, remove_files: usize },
    /// A download started (`resumed_from` bytes were already there).
    FileStarted { path: String, size: u64, resumed_from: u64 },
    /// More bytes arrived (a delta, across all parallel downloads), including resumed bytes
    /// once per file when it starts, so the sum reaches [`Progress::Planned`]'s total.
    Bytes(u64),
    /// A file was verified and put in place.
    FileDone { path: String },
    Removed { path: String },
    Done,
}

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

/// A [`ProgressFn`] that ignores everything.
pub fn no_progress() -> ProgressFn {
    Arc::new(|_| {})
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Parallel downloads (and hashing threads while checking).
    pub jobs: usize,
    /// Rehash every file instead of trusting the hash cache for unchanged size+mtime.
    pub full_check: bool,
    /// Attempts per file before giving up (network errors resume where they stopped).
    pub attempts: u32,
    /// Set it to stop [`plan`] and [`apply`] soon, with [`Error::Cancelled`] (between files
    /// while hashing, between chunks while downloading; partial downloads resume later).
    pub cancel: Option<Arc<AtomicBool>>,
    /// The game mode to install ([`Manifest::for_mode`]: its LocalConfig.xml); `None`: the
    /// server's default.
    pub mode: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self { jobs: 4, full_check: false, attempts: 4, cancel: None, mode: None }
    }
}

impl Options {
    fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
    }

    fn check_cancel(&self) -> Result<()> {
        if self.cancelled() { Err(Error::Cancelled) } else { Ok(()) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    Missing,
    /// Different size or content (or not a regular file).
    Changed,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub entry: FileEntry,
    pub reason: Reason,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub download: Vec<Change>,
    /// Up to date but missing executable bits.
    pub fix_mode: Vec<FileEntry>,
    /// Local files the manifest removes.
    pub remove: Vec<String>,
    pub unchanged: usize,
}

impl Plan {
    pub fn download_bytes(&self) -> u64 {
        self.download.iter().map(|c| c.entry.size).sum()
    }

    pub fn is_up_to_date(&self) -> bool {
        self.download.is_empty() && self.fix_mode.is_empty() && self.remove.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub downloaded_files: usize,
    pub downloaded_bytes: u64,
    pub removed_files: usize,
    pub unchanged: usize,
}

fn state_dir(dir: &Path) -> PathBuf {
    dir.join(STATE_DIR)
}

fn cache_path(dir: &Path) -> PathBuf {
    state_dir(dir).join("hash-cache.json")
}

/// `dir/rel` after checking `rel` and that no directory on the way is a symlink (which could
/// point outside `dir`). The final component may be a symlink: it is replaced, not followed.
pub fn safe_join(dir: &Path, rel: &str) -> Result<PathBuf> {
    check_path(rel)?;
    let mut cur = dir.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        cur.push(part);
        if i + 1 < parts.len() {
            match fs::symlink_metadata(&cur) {
                Ok(m) if m.file_type().is_symlink() => return Err(Error::UnsafePath(rel.to_string())),
                Ok(m) if !m.is_dir() => {
                    return Err(Error::Other(format!("{rel}: {} is not a directory", cur.display())));
                }
                _ => {}
            }
        }
    }
    Ok(cur)
}

fn manifest_path(dir: &Path) -> PathBuf {
    state_dir(dir).join("manifest.json")
}

/// The game mode the install is for (`.lmoxide/mode`, written by [`save_mode`]); `None`: the
/// server's default.
pub fn load_mode(dir: &Path) -> Option<String> {
    let mode = fs::read_to_string(state_dir(dir).join("mode")).ok()?;
    let mode = mode.trim();
    (!mode.is_empty()).then(|| mode.to_string())
}

/// Remembers the game mode the install is for (`None`: the default), so a later update or check
/// keeps it.
pub fn save_mode(dir: &Path, mode: Option<&str>) -> Result<()> {
    let path = state_dir(dir).join("mode");
    match mode {
        Some(m) => {
            fs::create_dir_all(state_dir(dir))?;
            fs::write(path, format!("{m}\n"))?;
        }
        None => {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

/// Saves the verified manifest envelope as `.lmoxide/manifest.json` (for launchers and as a
/// record of what was installed).
pub fn save_manifest(dir: &Path, signed: &SignedManifest) -> Result<()> {
    let path = manifest_path(dir);
    fs::create_dir_all(state_dir(dir))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(signed)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

/// The manifest [`save_manifest`] saved, verified with the install's trusted key
/// ([`stored_key`]). `None` if there is no saved manifest or no trusted key.
pub fn load_manifest(dir: &Path) -> Result<Option<Manifest>> {
    let data = match fs::read(manifest_path(dir)) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let Some(key) = stored_key(dir)? else { return Ok(None) };
    let signed: SignedManifest = serde_json::from_slice(&data)?;
    signed.verify(&key).map(Some)
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    true
}

/// Adds the executable bits wherever the file is readable (`chmod +x`, respecting the mode).
#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)?.permissions().mode();
    fs::set_permissions(path, fs::Permissions::from_mode(mode | (mode & 0o444) >> 2))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> io::Result<()> {
    Ok(())
}

/// Compares `dir` with `manifest`. Hashes files whose size matches (in `opts.jobs` threads),
/// using the install's hash cache unless `opts.full_check`.
pub async fn plan(dir: &Path, manifest: &Manifest, opts: &Options, progress: &ProgressFn) -> Result<Plan> {
    let (dir, manifest, opts, progress) = (dir.to_path_buf(), manifest.clone(), opts.clone(), progress.clone());
    tokio::task::spawn_blocking(move || plan_blocking(&dir, &manifest, &opts, &progress))
        .await
        .map_err(|e| Error::Other(e.to_string()))?
}

fn plan_blocking(dir: &Path, manifest: &Manifest, opts: &Options, progress: &ProgressFn) -> Result<Plan> {
    manifest.validate()?;
    let manifest = &manifest.for_mode(opts.mode.as_deref());
    let mut cache = HashCache::load(&cache_path(dir));
    let mut plan = Plan::default();
    let total = manifest.files.len();
    // Files whose content must be compared: (entry, path, metadata).
    let mut to_hash = Vec::new();
    for entry in &manifest.files {
        opts.check_cancel()?;
        let path = safe_join(dir, &entry.path)?;
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                plan.download.push(Change { entry: entry.clone(), reason: Reason::Missing })
            }
            Err(e) => return Err(e.into()),
            Ok(m) if !m.is_file() || m.len() != entry.size => {
                plan.download.push(Change { entry: entry.clone(), reason: Reason::Changed })
            }
            Ok(m) => match cache.get(&entry.path, &m).filter(|_| !opts.full_check) {
                Some(h) if h == entry.sha256 => check_mode(&mut plan, entry, &m),
                Some(_) => plan.download.push(Change { entry: entry.clone(), reason: Reason::Changed }),
                None => to_hash.push((entry, path, m)),
            },
        }
    }
    let done_without_hashing = total - to_hash.len();
    progress(Progress::Checking { done: done_without_hashing, total });

    // Hash the rest in parallel.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, io::Result<String>)>> = Mutex::new(Vec::with_capacity(to_hash.len()));
    std::thread::scope(|s| {
        for _ in 0..opts.jobs.clamp(1, 16).min(to_hash.len().max(1)) {
            s.spawn(|| {
                while !opts.cancelled() {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((_, path, _)) = to_hash.get(i) else { break };
                    let r = sha256_file(path);
                    let mut results = results.lock().unwrap();
                    results.push((i, r));
                    progress(Progress::Checking { done: done_without_hashing + results.len(), total });
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    if opts.cancelled() {
        // keep what was hashed for next time
        for (i, r) in results {
            if let Ok(h) = r {
                cache.insert(&to_hash[i].0.path, &to_hash[i].2, h);
            }
        }
        let _ = cache.save(&cache_path(dir));
        return Err(Error::Cancelled);
    }
    results.sort_by_key(|(i, _)| *i);
    for (i, r) in results {
        let (entry, _, meta) = &to_hash[i];
        let h = r?;
        let same = h == entry.sha256;
        cache.insert(&entry.path, meta, h);
        if same {
            check_mode(&mut plan, entry, meta);
        } else {
            plan.download.push(Change { entry: (*entry).clone(), reason: Reason::Changed });
        }
    }

    let listed: HashSet<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    for rel in &manifest.remove {
        if listed.contains(rel.as_str()) {
            continue;
        }
        let path = safe_join(dir, rel)?;
        if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_dir()) {
            plan.remove.push(rel.clone());
        }
    }
    plan.download.sort_by(|a, b| a.entry.path.cmp(&b.entry.path));
    let _ = cache.save(&cache_path(dir));
    Ok(plan)
}

fn check_mode(plan: &mut Plan, entry: &FileEntry, meta: &fs::Metadata) {
    if entry.executable && !is_executable(meta) {
        plan.fix_mode.push(entry.clone());
    } else {
        plan.unchanged += 1;
    }
}

/// Carries out `plan`: downloads (`opts.jobs` at a time), fixes modes, removes files.
pub async fn apply(client: &Client, dir: &Path, plan: &Plan, opts: &Options, progress: &ProgressFn) -> Result<Report> {
    progress(Progress::Planned {
        download_files: plan.download.len(),
        download_bytes: plan.download_bytes(),
        remove_files: plan.remove.len(),
    });
    let partial_dir = state_dir(dir).join("partial");
    fs::create_dir_all(&partial_dir)?;
    let cache = Arc::new(Mutex::new(HashCache::load(&cache_path(dir))));

    // Identical files (same sha256) are downloaded once.
    let mut groups: BTreeMap<&str, Vec<&FileEntry>> = BTreeMap::new();
    for c in &plan.download {
        groups.entry(c.entry.sha256.as_str()).or_default().push(&c.entry);
    }
    let result = futures::stream::iter(groups.into_values().map(Ok::<_, Error>))
        .try_for_each_concurrent(opts.jobs.max(1), |entries| {
            let (partial_dir, cache) = (partial_dir.clone(), cache.clone());
            async move {
                opts.check_cancel()?;
                download_group(client, dir, &partial_dir, &entries, opts, progress).await?;
                let mut cache = cache.lock().unwrap();
                for e in entries {
                    if let Ok(m) = fs::metadata(dir.join(&e.path)) {
                        cache.insert(&e.path, &m, e.sha256.clone());
                    }
                }
                Ok(())
            }
        })
        .await;
    // Keep what was verified so far even if something failed.
    let _ = cache.lock().unwrap().save(&cache_path(dir));
    result?;

    for e in &plan.fix_mode {
        make_executable(&safe_join(dir, &e.path)?)?;
    }
    let mut cache = Arc::try_unwrap(cache).map(|m| m.into_inner().unwrap()).unwrap_or_default();
    for rel in &plan.remove {
        fs::remove_file(safe_join(dir, rel)?)?;
        cache.remove(rel);
        progress(Progress::Removed { path: rel.clone() });
    }
    let _ = cache.save(&cache_path(dir));
    let _ = fs::remove_dir(&partial_dir);
    progress(Progress::Done);
    Ok(Report {
        downloaded_files: plan.download.len(),
        downloaded_bytes: plan.download_bytes(),
        removed_files: plan.remove.len(),
        unchanged: plan.unchanged,
    })
}

/// [`plan`] then [`apply`].
pub async fn sync(
    client: &Client,
    dir: &Path,
    manifest: &Manifest,
    opts: &Options,
    progress: &ProgressFn,
) -> Result<(Plan, Report)> {
    fs::create_dir_all(dir)?;
    let plan = plan(dir, manifest, opts, progress).await?;
    let report = apply(client, dir, &plan, opts, progress).await?;
    Ok((plan, report))
}

/// Downloads one body to the partial dir, verifies it and installs it at every path that
/// has this content.
async fn download_group(
    client: &Client,
    dir: &Path,
    partial_dir: &Path,
    entries: &[&FileEntry],
    opts: &Options,
    progress: &ProgressFn,
) -> Result<()> {
    let first = entries[0];
    let partial = partial_dir.join(&first.sha256);
    let mut attempt = 0;
    loop {
        attempt += 1;
        match download_once(client, &partial, first, opts, progress).await {
            Ok(()) => break,
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e @ Error::HashMismatch { .. }) | Err(e @ Error::Status { .. }) if attempt >= 2 => return Err(e),
            Err(Error::HashMismatch { .. }) => {
                // Corrupt partial (or changed server file): start over.
                let _ = tokio::fs::remove_file(&partial).await;
            }
            Err(e) if attempt >= opts.attempts.max(1) => return Err(e),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await,
        }
    }
    let partial2 = partial.clone();
    let entries: Vec<FileEntry> = entries.iter().map(|e| (*e).clone()).collect();
    let dir = dir.to_path_buf();
    let placed = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        // Copies first; the partial itself is renamed into the last place.
        let mut placed = Vec::new();
        for (i, e) in entries.iter().enumerate().rev() {
            let dest = safe_join(&dir, &e.path)?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let src = if i == 0 {
                partial2.clone()
            } else {
                let copy = partial2.with_extension(format!("copy{i}"));
                fs::copy(&partial2, &copy)?;
                copy
            };
            if e.executable {
                make_executable(&src)?;
            }
            // A directory in the way can't be replaced atomically; a file or symlink can.
            if fs::symlink_metadata(&dest).is_ok_and(|m| m.is_dir()) {
                return Err(Error::Other(format!("{}: a directory is in the way", e.path)));
            }
            fs::rename(&src, &dest)?;
            placed.push(e.path.clone());
        }
        Ok(placed)
    })
    .await
    .map_err(|e| Error::Other(e.to_string()))??;
    for path in placed {
        progress(Progress::FileDone { path });
    }
    Ok(())
}

/// One attempt: resume from what `partial` has, then check size and hash.
async fn download_once(
    client: &Client,
    partial: &Path,
    entry: &FileEntry,
    opts: &Options,
    progress: &ProgressFn,
) -> Result<()> {
    let mut have = match tokio::fs::metadata(partial).await {
        Ok(m) if m.len() <= entry.size => m.len(),
        Ok(_) => {
            tokio::fs::remove_file(partial).await?;
            0
        }
        Err(_) => 0,
    };
    if have < entry.size {
        let resp = client.file(&entry.sha256, have).await?;
        let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(partial).await?;
        if have > 0 && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            // Range ignored: the whole body follows.
            file.set_len(0).await?;
            have = 0;
        }
        progress(Progress::FileStarted { path: entry.path.clone(), size: entry.size, resumed_from: have });
        if have > 0 {
            progress(Progress::Bytes(have));
        }
        let mut stream = resp.bytes_stream();
        let mut got = have;
        while let Some(chunk) = stream.next().await {
            if opts.cancelled() {
                file.flush().await?;
                return Err(Error::Cancelled);
            }
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    file.flush().await?;
                    return Err(e.into());
                }
            };
            got += chunk.len() as u64;
            if got > entry.size {
                drop(file);
                tokio::fs::remove_file(partial).await?;
                return Err(Error::Other(format!("{}: server sent more than {} bytes", entry.path, entry.size)));
            }
            file.write_all(&chunk).await?;
            progress(Progress::Bytes(chunk.len() as u64));
        }
        file.flush().await?;
        if got != entry.size {
            return Err(Error::Other(format!("{}: connection closed at {got} of {} bytes", entry.path, entry.size)));
        }
    } else {
        // Complete from an earlier attempt, or an empty file.
        if entry.size == 0 {
            tokio::fs::File::create(partial).await?;
        }
        progress(Progress::FileStarted { path: entry.path.clone(), size: entry.size, resumed_from: have });
        progress(Progress::Bytes(have));
    }
    let p = partial.to_path_buf();
    let got = tokio::task::spawn_blocking(move || sha256_file(&p)).await.map_err(|e| Error::Other(e.to_string()))??;
    if got != entry.sha256 {
        return Err(Error::HashMismatch { path: entry.path.clone(), expected: entry.sha256.clone(), got });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Launch, MANIFEST_FORMAT};

    fn manifest(files: &[(&str, &[u8], bool)], remove: &[&str]) -> Manifest {
        let files: Vec<FileEntry> = files
            .iter()
            .map(|(p, d, x)| FileEntry {
                path: p.to_string(),
                size: d.len() as u64,
                sha256: crate::sha256_hex(d),
                executable: *x,
            })
            .collect();
        Manifest {
            format: MANIFEST_FORMAT,
            server: "t".into(),
            platform: crate::Platform::Linux,
            version: Manifest::version_of(&files),
            generated_at: 0,
            login_public_key: None,
            universe_addr: "h:1".into(),
            files,
            remove: remove.iter().map(|s| s.to_string()).collect(),
            launch: Launch::default(),
            modes: Vec::new(),
        }
    }

    #[test]
    fn plans_against_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::write(d.join("same"), b"same").unwrap();
        fs::write(d.join("diff"), b"DIFF").unwrap();
        fs::write(d.join("short"), b"x").unwrap();
        fs::write(d.join("old.so"), b"old").unwrap();
        fs::write(d.join("mine.txt"), b"user file").unwrap();
        let m = manifest(
            &[("same", b"same", false), ("diff", b"diff", false), ("short", b"longer", false), ("sub/new", b"n", true)],
            &["old.so", "gone.so"],
        );
        let p = plan_blocking(d, &m, &Options::default(), &no_progress()).unwrap();
        let mut got: Vec<_> = p.download.iter().map(|c| (c.entry.path.as_str(), c.reason)).collect();
        got.sort();
        assert_eq!(got, [("diff", Reason::Changed), ("short", Reason::Changed), ("sub/new", Reason::Missing)]);
        assert_eq!(p.unchanged, 1);
        assert_eq!(p.remove, ["old.so"]);
        // the hash cache now remembers "same" and "diff"
        let cache = HashCache::load(&cache_path(d));
        assert_eq!(cache.get("same", &fs::metadata(d.join("same")).unwrap()), Some(crate::sha256_hex(b"same").as_str()));
    }

    #[test]
    fn cancelled_plan() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), b"a").unwrap();
        let m = manifest(&[("a", b"a", false)], &[]);
        let opts = Options { cancel: Some(Arc::new(AtomicBool::new(true))), ..Default::default() };
        assert!(matches!(plan_blocking(dir.path(), &m, &opts, &no_progress()), Err(Error::Cancelled)));
    }

    #[test]
    fn saved_manifest_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_manifest(dir.path()).unwrap().is_none());
        let key = crate::manifest::generate_signing_key();
        let m = manifest(&[("a", b"a", false)], &[]);
        let signed = SignedManifest::sign(&m, &key).unwrap();
        save_manifest(dir.path(), &signed).unwrap();
        // no trusted key yet
        assert!(load_manifest(dir.path()).unwrap().is_none());
        crate::client::Trust::FirstUse.verify(dir.path(), &signed, m.platform).unwrap();
        assert_eq!(load_manifest(dir.path()).unwrap(), Some(m));
        // signed by another key than the trusted one
        let other = SignedManifest::sign(&manifest(&[], &[]), &crate::manifest::generate_signing_key()).unwrap();
        save_manifest(dir.path(), &other).unwrap();
        assert!(load_manifest(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("Data")).unwrap();
        assert!(matches!(safe_join(dir.path(), "Data/x"), Err(Error::UnsafePath(_))));
        let m = manifest(&[("Data/x", b"x", false)], &[]);
        assert!(plan_blocking(dir.path(), &m, &Options::default(), &no_progress()).is_err());
        // a symlink as the file itself is replaced, not followed
        std::os::unix::fs::symlink(outside.path().join("f"), dir.path().join("f")).unwrap();
        assert!(safe_join(dir.path(), "f").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn executable_bits() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("LMO");
        fs::write(&f, b"elf").unwrap();
        let m = manifest(&[("LMO", b"elf", true)], &[]);
        let p = plan_blocking(dir.path(), &m, &Options::default(), &no_progress()).unwrap();
        assert_eq!(p.fix_mode.len(), 1);
        make_executable(&f).unwrap();
        let p = plan_blocking(dir.path(), &m, &Options::default(), &no_progress()).unwrap();
        assert!(p.is_up_to_date());
    }
}
