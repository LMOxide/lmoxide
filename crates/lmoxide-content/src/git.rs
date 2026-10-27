//! The content tree as a git checkout (the world editor's working copy, `var/content-work`):
//! what changed since the last commit, record by record, and a record's history.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use crate::work::{Key, Work};

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    anyhow::ensure!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether `dir` is a git checkout.
pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--git-dir"]).is_ok()
}

/// A file's text at a commit (`HEAD`, a hash), if it was there.
pub fn file_at(dir: &Path, rev: &str, rel: &Path) -> Option<String> {
    git(dir, &["show", &format!("{rev}:{}", rel.to_string_lossy())]).ok()
}

/// The records of a content file's text, by key.
pub fn records_of(text: &str) -> BTreeMap<Key, Value> {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return BTreeMap::new();
    };
    let list: Vec<Value> = match v {
        Value::Object(ref o) if o.contains_key("id") => vec![v],
        Value::Object(o) => o
            .into_values()
            .filter_map(|l| l.as_array().cloned())
            .flatten()
            .collect(),
        _ => Vec::new(),
    };
    list.into_iter()
        .filter_map(|r| {
            let id = r.get("id")?.as_str()?.to_string();
            let feature = r
                .get("feature")
                .and_then(Value::as_str)
                .filter(|f| !f.is_empty())
                .unwrap_or("live")
                .to_string();
            Some((Key::new(id, feature), r))
        })
        .collect()
}

/// Files changed, added or deleted since the last commit (relative to the tree).
pub fn changed_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let out = git(
        dir,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )?;
    let mut files = Vec::new();
    let mut parts = out.split('\0').filter(|p| !p.is_empty());
    while let Some(p) = parts.next() {
        let (status, path) = p.split_at(3.min(p.len()));
        files.push(PathBuf::from(path));
        if status.starts_with('R') || status.starts_with('C') {
            // the old name follows
            if let Some(old) = parts.next() {
                files.push(PathBuf::from(old));
            }
        }
    }
    // paths are relative to the repository's top; the tree may be a folder in it
    let prefix = git(dir, &["rev-parse", "--show-prefix"])?
        .trim()
        .to_string();
    Ok(files
        .into_iter()
        .filter_map(|f| f.strip_prefix(&prefix).ok().map(Path::to_path_buf))
        .filter(|f| f.extension().is_some_and(|x| x == "json") && !f.starts_with("schemas"))
        .collect())
}

/// A record changed since the last commit.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    #[serde(flatten)]
    pub key: Key,
    /// `added`, `changed` or `removed`.
    pub change: &'static str,
    pub file: PathBuf,
    /// Who saved it last (removed: who last had it).
    pub author: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

/// Every record that differs from the last commit.
pub fn changes(work: &Work) -> anyhow::Result<Vec<Change>> {
    let dir = work.dir();
    let mut out = Vec::new();
    for file in changed_files(dir)? {
        let before = file_at(dir, "HEAD", &file)
            .map(|t| records_of(&t))
            .unwrap_or_default();
        let after: BTreeMap<Key, Value> = std::fs::read_to_string(dir.join(&file))
            .map(|t| records_of(&t))
            .unwrap_or_default();
        let author = |v: &Value| {
            v.get("author")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        for (k, a) in &after {
            match before.get(k) {
                Some(b) if b == a => {}
                b => out.push(Change {
                    key: k.clone(),
                    change: if b.is_some() { "changed" } else { "added" },
                    file: file.clone(),
                    author: author(a),
                    before: b.cloned(),
                    after: Some(a.clone()),
                }),
            }
        }
        for (k, b) in before.iter().filter(|(k, _)| !after.contains_key(*k)) {
            // moved to another file: changed there, not removed
            if work.get(k).is_none() {
                out.push(Change {
                    key: k.clone(),
                    change: "removed",
                    file: file.clone(),
                    author: author(b),
                    before: Some(b.clone()),
                    after: None,
                });
            }
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

/// One commit that changed a record.
#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub hash: String,
    /// Unix time.
    pub date: u64,
    /// The editor (`Content-Author:`), else the commit's author.
    pub author: String,
    /// `Approved-By:`, when the server committed an approval.
    pub approved_by: Option<String>,
    pub subject: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

/// The commits that changed a record (newest first, at most `limit`), with the record before
/// and after each.
pub fn history(work: &Work, key: &Key, limit: usize) -> anyhow::Result<Vec<Commit>> {
    let dir = work.dir();
    let Some(file) = work.get(key).map(|e| e.file.clone()) else {
        return Ok(Vec::new());
    };
    let format = "--format=%H%x1f%at%x1f%an%x1f%s%x1f%(trailers:key=Content-Author,valueonly,separator=%x2C)%x1f%(trailers:key=Approved-By,valueonly,separator=%x2C)%x1e";
    let log = git(dir, &["log", format, "--", &file.to_string_lossy()])?;
    let mut out = Vec::new();
    for entry in log.split('\x1e').map(str::trim).filter(|e| !e.is_empty()) {
        let f: Vec<&str> = entry.split('\x1f').collect();
        let [hash, date, git_author, subject, author, approver] = f[..] else {
            continue;
        };
        let at = |rev: &str| file_at(dir, rev, &file).and_then(|t| records_of(&t).remove(key));
        let (before, after) = (at(&format!("{hash}^")), at(hash));
        if before == after {
            continue;
        }
        out.push(Commit {
            hash: hash.to_string(),
            date: date.parse().unwrap_or(0),
            author: if author.trim().is_empty() {
                git_author.to_string()
            } else {
                author.trim().to_string()
            },
            approved_by: Some(approver.trim())
                .filter(|a| !a.is_empty())
                .map(str::to_string),
            subject: subject.to_string(),
            before,
            after,
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon;
    use serde_json::json;

    #[test]
    fn changes_and_history() {
        let dir = std::env::temp_dir().join(format!("lmoxide-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("npcs")).unwrap();
        std::fs::write(
            dir.join("content.json"),
            canon::to_string(&json!({ "format": 1, "reference": "../data" })).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("npcs/a.json"),
            canon::to_string(&json!({ "id": "npc/a", "level": 1 })).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("npcs/b.json"),
            canon::to_string(&json!({ "id": "npc/b" })).unwrap(),
        )
        .unwrap();
        let g = |args: &[&str]| git(&dir, args).unwrap();
        g(&["init", "-q"]);
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "add", "."]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "start",
        ]);
        let mut work = Work::open(&dir).unwrap();
        let a = Key::live("npc/a");
        let mut v = work.get(&a).unwrap().value.clone();
        v["level"] = json!(2);
        work.save(&a, v, None, "ann", 1).unwrap();
        work.delete(&Key::live("npc/b"), None, "ann", 2).unwrap();
        work.create(
            crate::work::NewRecord {
                kind: "npc".into(),
                name: "C".into(),
                ..Default::default()
            },
            "bob",
            3,
        )
        .unwrap();
        let c = changes(&work).unwrap();
        let list: Vec<_> = c
            .iter()
            .map(|c| (c.key.id.as_str(), c.change, c.author.as_str()))
            .collect();
        assert_eq!(
            list,
            [
                ("npc/a", "changed", "ann"),
                ("npc/b", "removed", ""),
                ("npc/c", "added", "bob")
            ]
        );
        g(&[
            "-c",
            "user.name=server",
            "-c",
            "user.email=s@s",
            "commit",
            "-qam",
            "Level 2\n\nContent-Author: ann\nApproved-By: root",
        ]);
        let h = history(&work, &a, 10).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(
            (
                h[0].author.as_str(),
                h[0].approved_by.as_deref(),
                h[0].after.as_ref().unwrap()["level"].clone()
            ),
            ("ann", Some("root"), json!(2))
        );
        assert_eq!((h[1].author.as_str(), h[1].before.is_none()), ("t", true));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
