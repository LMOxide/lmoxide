//! Contributors' commits (docs/WORLD-CONTENT.md): what a world editor commits of their changes in
//! the working copy goes on a branch of their own, `contrib/<name>`, made in a temporary worktree
//! so the shared working copy (and everyone's unsaved work in it) stays as it is. An admin merges
//! a commit into the working copy's branch (`main`) or rejects it; contributors download their
//! branch as patches or a git bundle.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::git::{Change, file_at, git, records_of};
use crate::work::{Key, Work};

/// A contributor's branch: `contrib/<slug of their name>`.
pub fn branch_of(author: &str) -> String {
    format!("contrib/{}", crate::slug(author))
}

/// Where a branch's decided commits are kept when the next commit starts again from `HEAD`.
const HISTORY: &str = "refs/contrib-history/";

/// The address a contributor's commits carry (`Content-Author:`).
fn email_of(author: &str) -> String {
    format!("{}@users.noreply.lmoxide", crate::slug(author))
}

/// `git` with extra environment (author and committer).
fn git_env(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).envs(env.iter().copied()).output()?;
    anyhow::ensure!(out.status.success(), "git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

const COMMITTER: [(&str, &str); 2] = [("GIT_COMMITTER_NAME", "lmoxide"), ("GIT_COMMITTER_EMAIL", "lmoxide@localhost")];

fn rev(dir: &Path, name: &str) -> Option<String> {
    git(dir, &["rev-parse", "--verify", "-q", &format!("{name}^{{commit}}")]).ok().map(|h| h.trim().to_string())
}

/// Commits `changes` (records of the working copy `work`, as they are now) to `author`'s branch:
/// on top of their last commit, else of the working copy's `HEAD`. `by`: who asked (an admin
/// may commit someone's changes; `Committed-By:` then). The new commit's hash.
pub fn commit(work: &Work, changes: &[Change], author: &str, by: &str, message: &str) -> anyhow::Result<String> {
    anyhow::ensure!(!changes.is_empty(), "nothing to commit");
    anyhow::ensure!(!message.trim().is_empty(), "a commit needs a message");
    anyhow::ensure!(!crate::slug(author).is_empty(), "no author");
    let dir = work.dir();
    let branch = format!("refs/heads/{}", branch_of(author));
    let old = rev(dir, &branch);
    let head = rev(dir, "HEAD").ok_or_else(|| anyhow::anyhow!("the working copy has no commit"))?;
    // every commit of the branch merged or rejected: start again from HEAD (on a rejected one,
    // the next could never merge), the old ones kept for the lists
    let fresh = match &old {
        Some(tip) => {
            let decided = decisions(dir);
            let all_decided = git(dir, &["cherry", "HEAD", tip])?.lines().all(|l| l.starts_with("- ") || l.split_once(' ').is_some_and(|(_, h)| decided.contains_key(h)));
            if all_decided && *tip != head {
                git(dir, &["update-ref", &format!("{HISTORY}{}/{}", crate::slug(author), &tip[..12]), tip])?;
            }
            all_decided
        }
        None => true,
    };
    let base = if fresh { head } else { old.clone().expect("checked") };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let tmp = std::env::temp_dir().join(format!("lmoxide-contrib-{}-{nanos}", std::process::id()));
    git(dir, &["worktree", "add", "-q", "--detach", &tmp.to_string_lossy(), &base])?;
    let made = (|| -> anyhow::Result<String> {
        let mut t = Work::open(&tmp)?;
        for c in changes {
            // a new record goes where it is in the working copy's file
            let before = if c.after.is_some() && t.get(&c.key).is_none() { work.next_in_file(&c.key).filter(|k| t.get(k).is_some()) } else { None };
            t.reset_before(&c.key, c.after.clone(), before.as_ref()).map_err(|e| anyhow::anyhow!("{} ({}): {e}", c.key.id, c.key.feature))?;
        }
        git(&tmp, &["add", "-A"])?;
        anyhow::ensure!(git(&tmp, &["diff", "--cached", "--quiet"]).is_err(), "nothing to commit: {} has these records as they are", branch_of(author));
        let mut msg = format!("{}\n\nContent-Author: {author} <{}>", message.trim(), email_of(author));
        if by != author {
            msg.push_str(&format!("\nCommitted-By: {by}"));
        }
        let email = email_of(author);
        let env = [COMMITTER[0], COMMITTER[1], ("GIT_AUTHOR_NAME", author), ("GIT_AUTHOR_EMAIL", email.as_str())];
        git_env(&tmp, &env, &["commit", "-q", "-m", &msg])?;
        rev(&tmp, "HEAD").ok_or_else(|| anyhow::anyhow!("no commit made"))
    })();
    let _ = git(dir, &["worktree", "remove", "--force", &tmp.to_string_lossy()]);
    let _ = std::fs::remove_dir_all(&tmp);
    let hash = made?;
    git(dir, &["update-ref", &branch, &hash, old.as_deref().unwrap_or("")])?;
    Ok(hash)
}

/// The records `author`'s branch changed and nobody merged yet: their value there (none:
/// removed). A change of the working copy that has this value is committed, not unpublished.
pub fn committed(dir: &Path, author: &str) -> BTreeMap<Key, Option<Value>> {
    let branch = branch_of(author);
    let mut out = BTreeMap::new();
    let (Some(tip), Some(head)) = (rev(dir, &branch), rev(dir, "HEAD")) else { return out };
    let Ok(base) = git(dir, &["merge-base", &head, &tip]) else { return out };
    let base = base.trim();
    for file in changed_json(dir, base, &tip) {
        let at_base = file_at(dir, base, &file).map(|t| records_of(&t)).unwrap_or_default();
        let at_tip = file_at(dir, &tip, &file).map(|t| records_of(&t)).unwrap_or_default();
        // (merged: HEAD has it so too)
        let at_head = file_at(dir, &head, &file).map(|t| records_of(&t)).unwrap_or_default();
        for k in at_base.keys().chain(at_tip.keys()) {
            if at_base.get(k) != at_tip.get(k) && at_head.get(k) != at_tip.get(k) {
                out.insert(k.clone(), at_tip.get(k).cloned());
            }
        }
    }
    out
}

/// The content files (not schemas) that differ between two commits.
fn changed_json(dir: &Path, from: &str, to: &str) -> Vec<PathBuf> {
    git(dir, &["diff", "--name-only", "--no-renames", from, to, "--", "."])
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|f| f.extension().is_some_and(|x| x == "json") && !f.starts_with("schemas"))
        .collect()
}

/// The records a commit changed, before and after (a record moved to another file is changed).
pub fn records_of_commit(dir: &Path, hash: &str) -> Vec<Change> {
    let parent = format!("{hash}^");
    let (mut before, mut after) = (BTreeMap::new(), BTreeMap::new());
    let mut file_of = BTreeMap::new();
    for file in changed_json(dir, &parent, hash) {
        for (k, v) in file_at(dir, &parent, &file).map(|t| records_of(&t)).unwrap_or_default() {
            file_of.entry(k.clone()).or_insert_with(|| file.clone());
            before.insert(k, v);
        }
        for (k, v) in file_at(dir, hash, &file).map(|t| records_of(&t)).unwrap_or_default() {
            file_of.insert(k.clone(), file.clone());
            after.insert(k, v);
        }
    }
    let author = |v: &Value| v.get("author").and_then(Value::as_str).unwrap_or("").to_string();
    let keys: BTreeSet<&Key> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter(|k| before.get(*k) != after.get(*k))
        .map(|k| {
            let (b, a) = (before.get(k).cloned(), after.get(k).cloned());
            Change {
                key: k.clone(),
                change: match (&b, &a) {
                    (None, _) => "added",
                    (_, None) => "removed",
                    _ => "changed",
                },
                file: file_of[k].clone(),
                author: a.as_ref().or(b.as_ref()).map(author).unwrap_or_default(),
                before: b,
                after: a,
            }
        })
        .collect()
}

/// What an admin decided about a commit (kept beside the working copy, [`review_file`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    /// `merged` or `rejected`.
    pub status: String,
    pub by: String,
    /// Unix time.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The commit it became on `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_as: Option<String>,
}

/// `var/content-work` -> `var/content-work-review.json`.
pub fn review_file(dir: &Path) -> PathBuf {
    let name = dir.file_name().map_or("content-work".into(), |n| n.to_string_lossy().into_owned());
    dir.with_file_name(format!("{name}-review.json"))
}

fn decisions(dir: &Path) -> BTreeMap<String, Decision> {
    std::fs::read_to_string(review_file(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn decide(dir: &Path, hash: &str, d: Decision) -> anyhow::Result<()> {
    let mut all = decisions(dir);
    all.insert(hash.to_string(), d);
    let path = review_file(dir);
    std::fs::write(path.with_extension("json.tmp"), crate::canon::to_string(&all)?)?;
    std::fs::rename(path.with_extension("json.tmp"), path)?;
    Ok(())
}

/// A contributor's commit, as admins review it.
#[derive(Debug, Clone, Serialize)]
pub struct Contribution {
    pub hash: String,
    pub branch: String,
    /// The editor whose changes these are (`Content-Author:`).
    pub author: String,
    /// Who committed them (the author, or an admin for them).
    pub by: String,
    /// Unix time it was committed.
    pub created: u64,
    /// The commit message's first line.
    pub subject: String,
    /// The whole commit message, without its trailers.
    pub message: String,
    /// `pending`, `merged` or `rejected`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    pub records: Vec<Change>,
}

fn describe(dir: &Path, hash: &str, branch: &str, status: &str, decision: Option<Decision>) -> anyhow::Result<Contribution> {
    let f = git(dir, &["show", "-s", "--format=%at%x1f%an%x1f%(trailers:key=Content-Author,valueonly,separator=%x2C)%x1f%(trailers:key=Committed-By,valueonly,separator=%x2C)%x1f%s%x1f%B", hash])?;
    let parts: Vec<&str> = f.splitn(6, '\x1f').collect();
    let [at, git_author, author, by, subject, body] = parts[..] else { anyhow::bail!("{hash}: unreadable commit") };
    let author = author.trim().split(" <").next().unwrap_or("").trim();
    let author = if author.is_empty() { git_author.trim() } else { author };
    let message: Vec<&str> = body.trim_end().lines().filter(|l| !l.starts_with("Content-Author:") && !l.starts_with("Committed-By:")).collect();
    Ok(Contribution {
        hash: hash.to_string(),
        branch: branch.to_string(),
        author: author.to_string(),
        by: Some(by.trim()).filter(|b| !b.is_empty()).unwrap_or(author).to_string(),
        created: at.trim().parse().unwrap_or(0),
        subject: subject.to_string(),
        message: message.join("\n").trim().to_string(),
        status: status.to_string(),
        decision,
        records: records_of_commit(dir, hash),
    })
}

/// Every contributor's commits not in `HEAD` (and the ones merged or rejected), oldest first.
pub fn list(dir: &Path) -> anyhow::Result<Vec<Contribution>> {
    let decided = decisions(dir);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for refname in git(dir, &["for-each-ref", "--format=%(refname)", "refs/heads/contrib/", HISTORY])?.lines() {
        let slug = refname.strip_prefix("refs/heads/contrib/").or_else(|| refname.strip_prefix(HISTORY).and_then(|r| r.split('/').next())).unwrap_or("");
        let branch = format!("contrib/{slug}");
        let branch = branch.as_str();
        // `+ hash`: not in HEAD; `- hash`: the same change is (merged)
        for line in git(dir, &["cherry", "HEAD", refname])?.lines() {
            let Some((sign, hash)) = line.split_once(' ') else { continue };
            if !seen.insert(hash.to_string()) {
                continue;
            }
            let d = decided.get(hash).cloned();
            let status = d.as_ref().map_or(if sign == "-" { "merged" } else { "pending" }, |d| d.status.as_str()).to_string();
            out.push(describe(dir, hash, branch, &status, d)?);
        }
    }
    // (stable: a branch's commits of the same second stay in their order)
    out.sort_by_key(|c| c.created);
    Ok(out)
}

/// One contributor commit (as [`list`] has it).
pub fn get(dir: &Path, hash: &str) -> anyhow::Result<Option<Contribution>> {
    Ok(list(dir)?.into_iter().find(|c| c.hash == hash || (hash.len() >= 7 && c.hash.starts_with(hash))))
}

/// Why a merge didn't happen.
#[derive(Debug)]
pub enum MergeError {
    NotFound,
    /// Merged or rejected already.
    Decided(String),
    /// Files both the commit and `main` changed since its base.
    Conflicts(Vec<String>),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for MergeError {
    fn from(e: anyhow::Error) -> Self {
        MergeError::Other(e)
    }
}

/// Merges a pending contributor commit into the working copy's branch (`HEAD`, `main`): its
/// change applied on top (as a cherry-pick, without touching the working tree), authored as the
/// contributor, with `Approved-By:` and `Lmoxide-Contribution:` (the commit's hash). The index
/// follows; the files of the working tree stay as they are (the editor brings the records it
/// didn't change in line). The commit as merged, and the new commit's hash.
pub fn merge(dir: &Path, hash: &str, approver: &str, now: u64) -> Result<(Contribution, String), MergeError> {
    let c = get(dir, hash)?.ok_or(MergeError::NotFound)?;
    if c.status != "pending" {
        return Err(MergeError::Decided(c.status));
    }
    let head = rev(dir, "HEAD").ok_or_else(|| anyhow::anyhow!("the working copy has no commit"))?;
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["merge-tree", "--write-tree", "--name-only", &format!("--merge-base={}^", c.hash), &head, &c.hash])
        .output()
        .map_err(anyhow::Error::from)?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines = text.lines();
    let tree = lines.next().unwrap_or("").trim().to_string();
    if out.status.code() == Some(1) {
        let files: Vec<String> = lines.take_while(|l| !l.is_empty()).map(str::to_string).collect();
        return Err(MergeError::Conflicts(files));
    }
    if !out.status.success() || tree.is_empty() {
        return Err(anyhow::anyhow!("git merge-tree: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    let info = git(dir, &["show", "-s", "--format=%an%x1f%ae%x1f%ad%x1f%B", &c.hash])?;
    let parts: Vec<&str> = info.splitn(4, '\x1f').collect();
    let [name, email, date, body] = parts[..] else { return Err(anyhow::anyhow!("{}: unreadable commit", c.hash).into()) };
    let msg = format!("{}\nApproved-By: {approver}\nLmoxide-Contribution: {}", body.trim_end(), c.hash);
    let env = [COMMITTER[0], COMMITTER[1], ("GIT_AUTHOR_NAME", name), ("GIT_AUTHOR_EMAIL", email), ("GIT_AUTHOR_DATE", date)];
    let new = git_env(dir, &env, &["commit-tree", &tree, "-p", &head, "-m", &msg])?.trim().to_string();
    git(dir, &["update-ref", "-m", &format!("merge {}", c.hash), "HEAD", &new, &head])?;
    // the index as HEAD has it; the files stay
    git(dir, &["reset", "-q"])?;
    decide(dir, &c.hash, Decision { status: "merged".into(), by: approver.into(), at: now, reason: None, merged_as: Some(new.clone()) })?;
    Ok((c, new))
}

/// Rejects a pending contributor commit, with the reason. It stays on its branch; what the
/// contributor commits next goes on top.
pub fn reject(dir: &Path, hash: &str, by: &str, reason: Option<String>, now: u64) -> Result<Contribution, MergeError> {
    let mut c = get(dir, hash)?.ok_or(MergeError::NotFound)?;
    if c.status != "pending" {
        return Err(MergeError::Decided(c.status));
    }
    let d = Decision { status: "rejected".into(), by: by.into(), at: now, reason, merged_as: None };
    decide(dir, &c.hash, d.clone())?;
    c.status = "rejected".into();
    c.decision = Some(d);
    Ok(c)
}

/// How a contributor's commits download.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    /// `git format-patch` of the commits not in `HEAD` (`git am` applies them).
    Patch,
    /// A git bundle of the branch (`git fetch <file> contrib/<name>`), needing `HEAD`'s history.
    Bundle,
}

/// `author`'s commits not merged yet, to download.
pub fn download(dir: &Path, author: &str, format: Format) -> anyhow::Result<Vec<u8>> {
    let branch = branch_of(author);
    anyhow::ensure!(rev(dir, &branch).is_some(), "{author} has no commits ({branch})");
    let range = format!("HEAD..{branch}");
    match format {
        Format::Patch => {
            let out = Command::new("git").arg("-C").arg(dir).args(["format-patch", "--stdout", "--ignore-if-in-upstream", &range]).output()?;
            anyhow::ensure!(out.status.success(), "git format-patch: {}", String::from_utf8_lossy(&out.stderr).trim());
            Ok(out.stdout)
        }
        Format::Bundle => {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let file = std::env::temp_dir().join(format!("lmoxide-bundle-{}-{nanos}.bundle", std::process::id()));
            let made = git(dir, &["bundle", "create", "-q", &file.to_string_lossy(), &branch, "^HEAD"]);
            let bytes = made.and_then(|_| Ok(std::fs::read(&file)?));
            let _ = std::fs::remove_file(&file);
            bytes.map_err(|e| anyhow::anyhow!("{e:#} (nothing to download: every commit of {branch} is merged?)"))
        }
    }
}

/// How many commits `HEAD` has that `origin`'s main doesn't (merged, not pushed), when it
/// has a remote-tracking `origin/main`.
pub fn unpushed(dir: &Path) -> Option<u64> {
    git(dir, &["rev-list", "--count", "origin/main..HEAD"]).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon;
    use serde_json::json;

    fn tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lmoxide-contrib-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("npcs")).unwrap();
        std::fs::write(dir.join("content.json"), canon::to_string(&json!({ "format": 1, "reference": "../data" })).unwrap()).unwrap();
        for (slug, level) in [("a", 1), ("b", 1)] {
            std::fs::write(dir.join(format!("npcs/{slug}.json")), canon::to_string(&json!({ "id": format!("npc/{slug}"), "level": level })).unwrap()).unwrap();
        }
        crate::load::format_tree(&dir, false).unwrap();
        let g = |args: &[&str]| git(&dir, &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["add", "."]);
        g(&["commit", "-qm", "start"]);
        dir
    }

    fn level(work: &mut Work, slug: &str, n: u64, who: &str) {
        let k = Key::live(&format!("npc/{slug}"));
        let mut v = work.get(&k).unwrap().value.clone();
        v["level"] = json!(n);
        work.save(&k, v, None, who, 1).unwrap();
    }

    #[test]
    fn commit_merge_reject() {
        let dir = tree("flow");
        let mut work = Work::open(&dir).unwrap();
        level(&mut work, "a", 2, "ann");
        level(&mut work, "b", 5, "bob");
        let mine: Vec<Change> = crate::git::changes(&work).unwrap().into_iter().filter(|c| c.author == "ann").collect();
        assert_eq!(mine.len(), 1);
        let h = commit(&work, &mine, "ann", "ann", "Ann's level\n\nWhy: tests").unwrap();
        // the working copy is untouched, ann's change is on her branch
        assert_eq!(crate::git::changes(&work).unwrap().len(), 2);
        let on = committed(&dir, "ann");
        assert_eq!(on.get(&Key::live("npc/a")).cloned().flatten().unwrap()["level"], 2);
        assert!(!on.contains_key(&Key::live("npc/b")));
        let l = list(&dir).unwrap();
        assert_eq!(l.len(), 1);
        let c = &l[0];
        assert_eq!((c.hash.as_str(), c.author.as_str(), c.status.as_str(), c.subject.as_str()), (h.as_str(), "ann", "pending", "Ann's level"));
        assert_eq!(c.message, "Ann's level\n\nWhy: tests");
        assert_eq!(c.records.len(), 1);
        assert_eq!(c.records[0].change, "changed");
        assert_eq!(c.records[0].before.as_ref().unwrap()["level"], 1);
        // a second commit goes on top; download has both
        level(&mut work, "a", 3, "ann");
        let mine: Vec<Change> = crate::git::changes(&work).unwrap().into_iter().filter(|c| c.author == "ann").collect();
        let h2 = commit(&work, &mine, "ann", "root", "Level 3").unwrap();
        let patch = String::from_utf8(download(&dir, "ann", Format::Patch).unwrap()).unwrap();
        assert!(patch.contains("Ann's level") && patch.contains("Level 3"));
        assert!(!download(&dir, "ann", Format::Bundle).unwrap().is_empty());
        assert_eq!(get(&dir, &h2).unwrap().unwrap().by, "root");
        // reject the first, merge the second (only its own change: 2 -> 3)
        let r = reject(&dir, &h, "root", Some("no".into()), 5).unwrap();
        assert_eq!(r.status, "rejected");
        assert!(matches!(merge(&dir, &h, "root", 6), Err(MergeError::Decided(_))));
        // the second changes the line the rejected one did: main (1) and its base (2) differ there
        match merge(&dir, &h2, "root", 7) {
            Err(MergeError::Conflicts(files)) => assert_eq!(files, ["npcs/a.json"]),
            other => panic!("{other:?}"),
        }
        assert_eq!(list(&dir).unwrap().iter().map(|c| c.status.as_str()).collect::<Vec<_>>(), ["rejected", "pending"]);
        // once both are decided, the next commit starts from HEAD and merges; the old ones stay listed
        reject(&dir, &h2, "root", None, 8).unwrap();
        level(&mut work, "a", 4, "ann");
        let mine: Vec<Change> = crate::git::changes(&work).unwrap().into_iter().filter(|c| c.author == "ann").collect();
        let h3 = commit(&work, &mine, "ann", "ann", "Level 4").unwrap();
        assert_eq!(git(&dir, &["rev-parse", &format!("{h3}^")]).unwrap().trim(), rev(&dir, "HEAD").unwrap());
        merge(&dir, &h3, "root", 9).unwrap();
        let statuses: Vec<(String, String)> = list(&dir).unwrap().into_iter().map(|c| (c.subject, c.status)).collect();
        assert_eq!(statuses, [("Ann's level".into(), "rejected".into()), ("Level 3".into(), "rejected".into()), ("Level 4".into(), "merged".into())]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_applies_on_main() {
        let dir = tree("merge");
        let mut work = Work::open(&dir).unwrap();
        level(&mut work, "a", 2, "ann");
        let mine = crate::git::changes(&work).unwrap();
        let h = commit(&work, &mine, "ann", "ann", "A to 2").unwrap();
        let (_, new) = merge(&dir, &h, "root", 1).unwrap();
        assert_eq!(rev(&dir, "HEAD").unwrap(), new);
        let body = git(&dir, &["show", "-s", "--format=%an|%B", &new]).unwrap();
        assert!(body.starts_with("ann|A to 2") && body.contains("Approved-By: root") && body.contains(&format!("Lmoxide-Contribution: {h}")), "{body}");
        // main has it; the working copy's file was already so: nothing unpublished
        assert_eq!(file_at(&dir, &new, Path::new("npcs/a.json")).map(|t| records_of(&t)).unwrap()[&Key::live("npc/a")]["level"], 2);
        let work = Work::open(&dir).unwrap();
        assert!(crate::git::changes(&work).unwrap().is_empty());
        assert_eq!(list(&dir).unwrap()[0].status, "merged");
        assert!(committed(&dir, "ann").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
