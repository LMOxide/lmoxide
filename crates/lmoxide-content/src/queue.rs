//! What editors published and nobody approved yet (docs/WORLD-EDITOR-CONTENT.md, phase 3): one
//! entry per publish, the records it changes before and after, in a folder beside the working
//! copy (`var/content-queue/<seq>.json`). Approval (committing an entry to the content
//! repository) reads these.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::git::Change;
use crate::work::Key;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedRecord {
    #[serde(flatten)]
    pub key: Key,
    /// `added`, `changed` or `removed`.
    pub change: String,
    pub file: PathBuf,
    pub author: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

impl From<Change> for QueuedRecord {
    fn from(c: Change) -> Self {
        QueuedRecord {
            key: c.key,
            change: c.change.into(),
            file: c.file,
            author: c.author,
            before: c.before,
            after: c.after,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    /// Unix time it was published.
    pub created: u64,
    /// Who published it (for themselves, or others' changes too).
    pub by: String,
    /// "Isle of Yarr: 3 spawns, 1 trigger (feature live)".
    pub summary: String,
    /// `queued`, `rejected` (with `reason`) or `approved`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub records: Vec<QueuedRecord>,
}

pub struct Queue {
    dir: PathBuf,
}

impl Queue {
    pub fn new(dir: &Path) -> Self {
        Queue {
            dir: dir.to_path_buf(),
        }
    }

    /// Every entry, oldest first.
    pub fn entries(&self) -> Vec<Entry> {
        let mut out: Vec<Entry> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
            .collect();
        out.sort_by_key(|e: &Entry| e.seq);
        out
    }

    pub fn get(&self, seq: u64) -> Option<Entry> {
        self.entries().into_iter().find(|e| e.seq == seq)
    }

    /// Adds an entry (its `seq` the next one): the entry as stored.
    pub fn add(&self, mut e: Entry) -> std::io::Result<Entry> {
        std::fs::create_dir_all(&self.dir)?;
        e.seq = self.entries().last().map_or(1, |l| l.seq + 1);
        self.write(&e)?;
        Ok(e)
    }

    pub fn write(&self, e: &Entry) -> std::io::Result<()> {
        let text = crate::canon::to_string(e).map_err(std::io::Error::other)?;
        let path = self.dir.join(format!("{}.json", e.seq));
        std::fs::write(path.with_extension("json.tmp"), text)?;
        std::fs::rename(path.with_extension("json.tmp"), path)
    }

    pub fn remove(&self, seq: u64) -> std::io::Result<()> {
        std::fs::remove_file(self.dir.join(format!("{seq}.json")))
    }

    /// Whether a queued entry has this record as it is now (published, waiting).
    pub fn has(&self, key: &Key, after: Option<&Value>) -> bool {
        self.entries()
            .iter()
            .filter(|e| e.status == "queued")
            .any(|e| {
                e.records
                    .iter()
                    .any(|r| r.key == *key && r.after.as_ref() == after)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entries_queue_up() {
        let dir = std::env::temp_dir().join(format!("lmoxide-queue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let q = Queue::new(&dir);
        let rec = |v: i64| QueuedRecord {
            key: Key::live("npc/a"),
            change: "changed".into(),
            file: "npcs/a.json".into(),
            author: "ann".into(),
            before: None,
            after: Some(json!({ "id": "npc/a", "level": v })),
        };
        let e = |r| Entry {
            seq: 0,
            created: 1,
            by: "ann".into(),
            summary: String::new(),
            status: "queued".into(),
            reason: None,
            records: vec![r],
        };
        assert_eq!(q.add(e(rec(1))).unwrap().seq, 1);
        assert_eq!(q.add(e(rec(2))).unwrap().seq, 2);
        assert!(q.has(
            &Key::live("npc/a"),
            Some(&json!({ "id": "npc/a", "level": 2 }))
        ));
        assert!(!q.has(
            &Key::live("npc/a"),
            Some(&json!({ "id": "npc/a", "level": 3 }))
        ));
        q.remove(1).unwrap();
        assert_eq!(q.entries().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
