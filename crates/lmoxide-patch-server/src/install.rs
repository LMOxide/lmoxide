//! What an install of each platform consists of, and its signed manifest.
//!
//! An install is the platform's client dir, with:
//! - `<file>.orig` used in place of `<file>` when both exist (`patches/patch-client.py` and the
//!   Linux kit keep the stock copy there, and dev tools leave modified files next to it), unless
//!   `--no-orig`;
//! - files matching `--exclude` left out (logs, crash dumps, the `.orig` copies themselves);
//! - the overlay dir's files added on top (replacing same-named ones);
//! - the platform's remove list taken out (and listed in the manifest for patchers to delete);
//! - the patched executable and the generated `LocalConfig.xml` (written as blobs to the data
//!   dir, see `build.rs`).
//!
//! Hashing ~60k files takes a while the first time; [`hash_trees`] runs it on all cores and keeps
//! results in a [`HashCache`] keyed by absolute path, size and mtime, so restarts are quick.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use lmoxide_patch::{FileEntry, Manifest, Platform, check_path, hashcache::HashCache};
use tracing::warn;

/// One file of an install: where its content comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub executable: bool,
    /// Known for generated files (blobs); computed by [`hash_trees`] otherwise.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Tree {
    /// Relative path (`/`-separated) -> source.
    pub files: BTreeMap<String, Source>,
    pub remove: Vec<String>,
}

impl Tree {
    /// Looks up a path case-insensitively (the Windows client's paths are).
    pub fn get_ci(&self, rel: &str) -> Option<(&String, &Source)> {
        self.files.get_key_value(rel).or_else(|| self.files.iter().find(|(k, _)| k.eq_ignore_ascii_case(rel)))
    }
}

pub struct ScanOptions<'a> {
    pub platform: Platform,
    pub client_dir: &'a Path,
    pub overlay_dir: Option<&'a Path>,
    pub exclude: &'a GlobSet,
    pub remove: &'a [String],
    pub use_orig: bool,
}

pub fn build_globset(patterns: &[String]) -> anyhow::Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        // A pattern without `/` matches the file name at any depth.
        let p = if p.contains('/') { p.to_string() } else { format!("**/{p}") };
        b.add(Glob::new(&p)?);
    }
    Ok(b.build()?)
}

/// Every regular file under `root` as `(relative path, absolute path, executable)`. Symlinked
/// files are followed; symlinked directories are not (no loops, no escaping the tree).
fn walk(root: &Path) -> io::Result<Vec<(String, PathBuf, bool)>> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        for e in fs::read_dir(&dir)? {
            let e = e?;
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                warn!(path = %e.path().display(), "skipping non-UTF-8 file name");
                continue;
            };
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let ft = e.file_type()?;
            if ft.is_dir() {
                stack.push((e.path(), rel));
                continue;
            }
            let Ok(meta) = fs::metadata(e.path()) else { continue };
            if !meta.is_file() {
                continue;
            }
            if check_path(&rel).is_err() {
                warn!(%rel, "skipping file with an unsafe name");
                continue;
            }
            out.push((rel, e.path(), is_executable(&meta)));
        }
    }
    Ok(out)
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    false
}

pub fn scan(o: &ScanOptions) -> io::Result<Tree> {
    let exec = |rel: &str, disk: bool| match o.platform {
        Platform::Linux => disk,
        Platform::Windows => rel.to_ascii_lowercase().ends_with(".exe"),
    };
    let found = walk(o.client_dir)?;
    let mut files: BTreeMap<String, Source> = found
        .iter()
        .map(|(rel, path, x)| (rel.clone(), Source { path: path.clone(), executable: exec(rel, *x), sha256: None }))
        .collect();
    if o.use_orig {
        for (rel, path, _) in &found {
            if let Some(base) = rel.strip_suffix(".orig")
                && let Some(src) = files.get_mut(base)
            {
                src.path = path.clone();
            }
        }
    }
    files.retain(|rel, _| !o.exclude.is_match(rel));
    if let Some(overlay) = o.overlay_dir {
        for (rel, path, x) in walk(overlay)? {
            if !o.exclude.is_match(&rel) {
                files.insert(rel.clone(), Source { path, executable: exec(&rel, x), sha256: None });
            }
        }
    }
    let mut remove = Vec::new();
    for r in o.remove.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
        if check_path(r).is_err() {
            warn!(path = r, "ignoring unsafe remove entry");
            continue;
        }
        files.remove(r);
        remove.push(r.to_string());
    }
    Ok(Tree { files, remove })
}

/// Fills in the sha256 of every file (in `jobs` threads), using and updating `cache`.
pub fn hash_trees(trees: &mut [&mut Tree], cache: &mut HashCache, jobs: usize) -> io::Result<()> {
    let mut todo: Vec<(usize, String, PathBuf, fs::Metadata)> = Vec::new();
    for (t, tree) in trees.iter_mut().enumerate() {
        for (rel, src) in tree.files.iter_mut().filter(|(_, s)| s.sha256.is_none()) {
            let meta = fs::metadata(&src.path)?;
            match cache.get(&cache_key(&src.path), &meta) {
                Some(h) => src.sha256 = Some(h.to_string()),
                None => todo.push((t, rel.clone(), src.path.clone(), meta)),
            }
        }
    }
    if !todo.is_empty() {
        let bytes: u64 = todo.iter().map(|t| t.3.len()).sum();
        tracing::info!(files = todo.len(), mb = bytes >> 20, "hashing client files");
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, 32) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((_, _, path, _)) = todo.get(i) else { break };
                    let r = lmoxide_patch::hashcache::sha256_file(path);
                    results.lock().unwrap().push((i, r));
                }
            });
        }
    });
    for (i, r) in results.into_inner().unwrap() {
        let (t, rel, path, meta) = &todo[i];
        let h = r?;
        cache.insert(&cache_key(path), meta, h.clone());
        trees[*t].files.get_mut(rel).unwrap().sha256 = Some(h);
    }
    Ok(())
}

pub fn cache_key(path: &Path) -> String {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned()
}

/// The manifest file list of a hashed tree (needs file sizes from disk).
pub fn file_entries(tree: &Tree) -> io::Result<Vec<FileEntry>> {
    tree.files
        .iter()
        .map(|(rel, src)| {
            Ok(FileEntry {
                path: rel.clone(),
                size: fs::metadata(&src.path)?.len(),
                sha256: src.sha256.clone().ok_or_else(|| io::Error::other(format!("{rel} not hashed")))?,
                executable: src.executable,
            })
        })
        .collect()
}

/// How each platform's client is started (what Funcom's launcher / the Linux kit's `lmo` did).
pub fn launch_info(platform: Platform, tree: &Tree) -> lmoxide_patch::Launch {
    let login_args = ["-username", "{username}", "-password", "{password}"].map(String::from).to_vec();
    match platform {
        Platform::Windows => lmoxide_patch::Launch {
            executable: "LMO.exe".into(),
            args: vec!["-verified".into()],
            login_args,
            env: BTreeMap::new(),
        },
        Platform::Linux => {
            let mut env = BTreeMap::from([
                // The 2016 renderer needs GLX; SDL would pick Wayland/EGL and crash.
                ("SDL_VIDEODRIVER".to_string(), "x11".to_string()),
                ("LD_LIBRARY_PATH".to_string(), ".".to_string()),
            ]);
            if tree.files.contains_key("liblmofixes.so") {
                env.insert("LD_PRELOAD".into(), "liblmofixes.so".into());
            }
            lmoxide_patch::Launch { executable: "LMO".into(), args: vec!["-verified".into()], login_args, env }
        }
    }
}

/// Builds the (unsigned) manifest of a hashed tree.
pub fn manifest(
    platform: Platform,
    tree: &Tree,
    server: &str,
    universe_addr: &str,
    login_public_key: Option<&str>,
) -> io::Result<Manifest> {
    let files = file_entries(tree)?;
    Ok(Manifest {
        format: lmoxide_patch::manifest::MANIFEST_FORMAT,
        server: server.to_string(),
        platform,
        version: Manifest::version_of(&files),
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        login_public_key: login_public_key.map(str::to_string),
        universe_addr: universe_addr.to_string(),
        files,
        remove: tree.remove.clone(),
        launch: launch_info(platform, tree),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, data: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    #[test]
    fn scan_applies_orig_overlay_excludes_and_removes() {
        let client = tempfile::tempdir().unwrap();
        let overlay = tempfile::tempdir().unwrap();
        let c = client.path();
        write(c, "LMO", b"patched by hand");
        write(c, "LMO.orig", b"stock");
        write(c, "libstdc++.so.6", b"old c++");
        write(c, "ClientLog.txt", b"log");
        write(c, "OldLogs/1.txt", b"log");
        write(c, "RDB/1/AB.rdbres", b"res");
        write(c, "libgconf-2.so.4", b"from client");
        write(overlay.path(), "libgconf-2.so.4", b"stub");
        write(overlay.path(), "lmo", b"#!/bin/sh");
        let exclude = build_globset(&crate::DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        let remove = vec!["libstdc++.so.6".to_string()];
        let tree = scan(&ScanOptions {
            platform: Platform::Linux,
            client_dir: c,
            overlay_dir: Some(overlay.path()),
            exclude: &exclude,
            remove: &remove,
            use_orig: true,
        })
        .unwrap();
        let names: Vec<&str> = tree.files.keys().map(String::as_str).collect();
        assert_eq!(names, ["LMO", "RDB/1/AB.rdbres", "libgconf-2.so.4", "lmo"]);
        assert_eq!(tree.files["LMO"].path, c.join("LMO.orig"));
        assert_eq!(tree.files["libgconf-2.so.4"].path, overlay.path().join("libgconf-2.so.4"));
        assert_eq!(tree.remove, ["libstdc++.so.6"]);
        assert_eq!(tree.get_ci("lmo").unwrap().0, "lmo");
        assert_eq!(tree.get_ci("rdb/1/ab.rdbres").unwrap().0, "RDB/1/AB.rdbres");
    }

    #[test]
    fn hashing_uses_the_cache() {
        let client = tempfile::tempdir().unwrap();
        write(client.path(), "a", b"aaa");
        write(client.path(), "d/b", b"bbb");
        let exclude = build_globset(&[]).unwrap();
        let opts = ScanOptions {
            platform: Platform::Windows,
            client_dir: client.path(),
            overlay_dir: None,
            exclude: &exclude,
            remove: &[],
            use_orig: true,
        };
        let mut tree = scan(&opts).unwrap();
        let mut cache = HashCache::default();
        hash_trees(&mut [&mut tree], &mut cache, 2).unwrap();
        assert_eq!(tree.files["d/b"].sha256.as_deref(), Some(lmoxide_patch::sha256_hex(b"bbb").as_str()));
        assert_eq!(cache.len(), 2);

        // A second scan is answered from the cache: plant a fake hash to prove the file isn't read.
        let path = client.path().join("a");
        cache.insert(&cache_key(&path), &fs::metadata(&path).unwrap(), "f".repeat(64));
        let mut tree = scan(&opts).unwrap();
        hash_trees(&mut [&mut tree], &mut cache, 2).unwrap();
        assert_eq!(tree.files["a"].sha256.as_deref(), Some("f".repeat(64).as_str()));

        let m = manifest(Platform::Windows, &tree, "s", "h:1", None).unwrap();
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.files[1].size, 3);
        assert_eq!(m.launch.executable, "LMO.exe");
    }
}
