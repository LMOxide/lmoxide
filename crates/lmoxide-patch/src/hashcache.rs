//! sha256 cache keyed by file identity (size + modification time), used by the server for its
//! client trees and by patchers for the install dir: a client is 2-3 GB in ~60k files, and
//! rehashing all of it on every start or check takes minutes on slow disks.
//!
//! A file whose size and mtime are unchanged is assumed unchanged; `verify --full` style checks
//! bypass the cache. Stored as JSON, written atomically.

use std::{
    collections::HashMap,
    fs::{self, File, Metadata},
    io::{self, Read},
    path::Path,
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    size: u64,
    mtime_s: i64,
    mtime_ns: u32,
    sha256: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HashCache {
    entries: HashMap<String, Entry>,
    #[serde(skip)]
    dirty: bool,
}

/// `(seconds, nanoseconds)` since the epoch, negative seconds for older files.
fn mtime(meta: &Metadata) -> (i64, u32) {
    match meta.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
            Err(e) => (-(e.duration().as_secs() as i64), e.duration().subsec_nanos()),
        },
        Err(_) => (0, 0),
    }
}

impl HashCache {
    /// Loads the cache, or starts empty if it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        fs::read(path).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
    }

    /// Writes the cache if anything changed (temp file + rename).
    pub fn save(&mut self, path: &Path) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec(self).map_err(io::Error::other)?)?;
        fs::rename(&tmp, path)?;
        self.dirty = false;
        Ok(())
    }

    /// The cached hash of `key`, if `meta` still matches what was hashed.
    pub fn get(&self, key: &str, meta: &Metadata) -> Option<&str> {
        let e = self.entries.get(key)?;
        let (s, ns) = mtime(meta);
        (e.size == meta.len() && e.mtime_s == s && e.mtime_ns == ns).then_some(e.sha256.as_str())
    }

    pub fn insert(&mut self, key: &str, meta: &Metadata, sha256: String) {
        let (mtime_s, mtime_ns) = mtime(meta);
        let e = Entry { size: meta.len(), mtime_s, mtime_ns, sha256 };
        if self.entries.get(key) != Some(&e) {
            self.entries.insert(key.to_string(), e);
            self.dirty = true;
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.dirty |= self.entries.remove(key).is_some();
    }

    /// Drops entries for which `keep` is false (files that no longer exist).
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        let before = self.entries.len();
        self.entries.retain(|k, _| keep(k));
        self.dirty |= self.entries.len() != before;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Hash of the file at `path`, from the cache when its metadata is unchanged.
    pub fn hash(&mut self, key: &str, path: &Path) -> io::Result<String> {
        let meta = fs::metadata(path)?;
        if let Some(h) = self.get(key, &meta) {
            return Ok(h.to_string());
        }
        let h = sha256_file(path)?;
        // Metadata from before the read: if the file changed meanwhile, the next lookup misses.
        self.insert(key, &meta, h.clone());
        Ok(h)
    }
}

/// Streams a file through sha256.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hits_and_invalidates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.bin");
        let cache_path = dir.path().join("cache.json");
        fs::write(&file, b"one").unwrap();

        let mut c = HashCache::load(&cache_path);
        assert!(c.is_empty());
        assert_eq!(c.hash("a", &file).unwrap(), crate::sha256_hex(b"one"));
        c.save(&cache_path).unwrap();

        // reloaded: served from the cache without reading (prove it with a bogus entry)
        let mut c = HashCache::load(&cache_path);
        assert_eq!(c.len(), 1);
        let meta = fs::metadata(&file).unwrap();
        c.insert("a", &meta, "cached".into());
        assert_eq!(c.hash("a", &file).unwrap(), "cached");

        // a changed size or mtime misses
        fs::write(&file, b"three").unwrap();
        assert_eq!(c.hash("a", &file).unwrap(), crate::sha256_hex(b"three"));
        let f = File::options().write(true).open(&file).unwrap();
        f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_000_000)).unwrap();
        drop(f);
        let meta = fs::metadata(&file).unwrap();
        assert!(c.get("a", &meta).is_none());

        c.retain(|k| k != "a");
        assert!(c.is_empty());
    }
}
