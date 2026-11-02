//! Contributors' branches (docs/WORLD-CONTENT.md): what a world editor commits of their changes
//! in the working copy goes on a branch of theirs, `contrib/<editor>/<name>` (as many as they
//! like), made in a temporary worktree so the shared working copy (and everyone's unsaved work
//! in it) stays as it is. An admin reviews a branch as a whole, like a pull request: merging puts
//! its records, as the branch has them, on the working copy's `main` in one commit and resets the
//! branch to `main`; rejecting asks for changes (the next commit reopens it). Its editor can
//! reset or delete it. Merges are record by record: a conflict is a record `main` changed since
//! the branch started that the branch changes too.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::git::{Change, file_at, git, records_of};
use crate::work::{Key, Work};

/// Where merged, reset and deleted branches' commits are kept (for the lists).
const HISTORY: &str = "refs/contrib-history/";

/// A contributor's branch: `contrib/<slug of their name>/<slug of the branch's name>`.
pub fn branch_of(author: &str, name: &str) -> String {
    format!("contrib/{}/{}", crate::slug(author), crate::slug(name))
}

/// The editor's part of a branch's name (`contrib/<editor>/<name>`).
pub fn owner_of(branch: &str) -> Option<&str> {
    branch.strip_prefix("contrib/")?.split('/').next()
}

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

fn now_nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// Runs `f` in a temporary worktree of `dir` at `at` (removed after): its result.
fn in_worktree<T>(dir: &Path, at: &str, f: impl FnOnce(&Path) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let tmp = std::env::temp_dir().join(format!("lmoxide-contrib-{}-{}", std::process::id(), now_nanos()));
    git(dir, &["worktree", "add", "-q", "--detach", &tmp.to_string_lossy(), at])?;
    let out = f(&tmp);
    let _ = git(dir, &["worktree", "remove", "--force", &tmp.to_string_lossy()]);
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = git(dir, &["worktree", "prune"]);
    out
}

// ------------------------------------------------------------------ review state

/// What admins decided about a branch (it stays until its next commit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    /// `rejected`.
    pub status: String,
    pub by: String,
    /// Unix time.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The branch's last commit it was about.
    pub tip: String,
}

/// A branch that was merged, reset or deleted (its commits under [`HISTORY`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Closed {
    pub branch: String,
    pub title: String,
    /// `merged`, `reset` or `deleted`.
    pub status: String,
    pub by: String,
    pub at: u64,
    /// The commit it became on `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_as: Option<String>,
    /// The rejection it had, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct BranchState {
    /// The name as its editor typed it.
    title: String,
    decision: Option<Decision>,
}

/// Kept beside the working copy ([`review_file`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Review {
    branches: BTreeMap<String, BranchState>,
    /// By the branch's last commit then.
    closed: BTreeMap<String, Closed>,
}

/// `var/content-work` -> `var/content-work-review.json`.
pub fn review_file(dir: &Path) -> PathBuf {
    let name = dir.file_name().map_or("content-work".into(), |n| n.to_string_lossy().into_owned());
    dir.with_file_name(format!("{name}-review.json"))
}

fn review(dir: &Path) -> Review {
    std::fs::read_to_string(review_file(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_review(dir: &Path, r: &Review) -> anyhow::Result<()> {
    let path = review_file(dir);
    std::fs::write(path.with_extension("json.tmp"), crate::canon::to_string(r)?)?;
    std::fs::rename(path.with_extension("json.tmp"), path)?;
    Ok(())
}

// ------------------------------------------------------------------ what a branch changes

/// A record a branch changes: `before` as `main` has it now (merged: as the branch started),
/// `after` as the branch has it.
#[derive(Debug, Clone, Serialize)]
pub struct BranchRecord {
    #[serde(flatten)]
    pub key: Key,
    /// `added`, `changed` or `removed` (against `before`).
    pub change: &'static str,
    pub file: PathBuf,
    /// Who saved it last on the branch.
    pub author: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    /// `main` changed it since the branch started: merging puts the branch's version over that.
    pub conflict: bool,
    /// The record as the branch started (with a conflict).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Value>,
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

/// Records in `files` at `rev`, with the file each is in.
fn records_at(dir: &Path, rev: &str, files: &[PathBuf]) -> BTreeMap<Key, (Value, PathBuf)> {
    let mut out = BTreeMap::new();
    for f in files {
        for (k, v) in file_at(dir, rev, f).map(|t| records_of(&t)).unwrap_or_default() {
            out.insert(k, (v, f.clone()));
        }
    }
    out
}

/// The records the commits `base..tip` change, against `against` (`main` now; `base` for a
/// branch already merged). Records `against` has as the branch does are left out.
fn net(dir: &Path, base: &str, tip: &str, against: &str) -> Vec<BranchRecord> {
    let files = changed_json(dir, base, tip);
    let at_base = records_at(dir, base, &files);
    let at_tip = records_at(dir, tip, &files);
    let at_main = if against == base { at_base.clone() } else { records_at(dir, against, &files) };
    let keys: BTreeSet<&Key> = at_base.keys().chain(at_tip.keys()).collect();
    let val = |m: &BTreeMap<Key, (Value, PathBuf)>, k: &Key| m.get(k).map(|(v, _)| v.clone());
    keys.into_iter()
        .filter(|k| val(&at_base, k) != val(&at_tip, k))
        .filter_map(|k| {
            let (b, t, m) = (val(&at_base, k), val(&at_tip, k), val(&at_main, k));
            if m == t {
                return None;
            }
            let conflict = m != b;
            Some(BranchRecord {
                key: k.clone(),
                change: match (&m, &t) {
                    (None, _) => "added",
                    (_, None) => "removed",
                    _ => "changed",
                },
                file: at_tip.get(k).or(at_base.get(k)).map(|(_, f)| f.clone()).unwrap_or_default(),
                author: t.as_ref().or(b.as_ref()).and_then(|v| v.get("author")).and_then(Value::as_str).unwrap_or("").to_string(),
                before: m,
                after: t,
                conflict,
                base: if conflict { b } else { None },
            })
        })
        .collect()
}

/// One commit of a branch.
#[derive(Debug, Clone, Serialize)]
pub struct BranchCommit {
    pub hash: String,
    /// The editor whose changes these are (`Content-Author:`).
    pub author: String,
    /// Who committed them (the editor, or someone for them).
    pub by: String,
    /// Unix time.
    pub created: u64,
    /// The message's first line.
    pub subject: String,
    /// The whole message, without its trailers.
    pub message: String,
}

fn commits_of(dir: &Path, base: &str, tip: &str) -> Vec<BranchCommit> {
    let format = "--format=%H%x1f%at%x1f%an%x1f%(trailers:key=Content-Author,valueonly,separator=%x2C)%x1f%(trailers:key=Committed-By,valueonly,separator=%x2C)%x1f%s%x1f%B%x1e";
    let log = git(dir, &["log", "--reverse", format, &format!("{base}..{tip}")]).unwrap_or_default();
    log.split('\x1e')
        .map(|e| e.trim_start_matches('\n'))
        .filter(|e| !e.trim().is_empty())
        .filter_map(|e| {
            let f: Vec<&str> = e.splitn(7, '\x1f').collect();
            let [hash, at, git_author, author, by, subject, body] = f[..] else { return None };
            let author = author.trim().split(" <").next().unwrap_or("").trim();
            let author = if author.is_empty() { git_author.trim() } else { author };
            let message: Vec<&str> = body.trim_end().lines().filter(|l| !l.starts_with("Content-Author:") && !l.starts_with("Committed-By:")).collect();
            Some(BranchCommit {
                hash: hash.to_string(),
                author: author.to_string(),
                by: Some(by.trim()).filter(|b| !b.is_empty()).unwrap_or(author).to_string(),
                created: at.trim().parse().unwrap_or(0),
                subject: subject.to_string(),
                message: message.join("\n").trim().to_string(),
            })
        })
        .collect()
}

/// A contributor's branch as editors and admins see it.
#[derive(Debug, Clone, Serialize)]
pub struct Branch {
    /// `contrib/<editor>/<name>`.
    pub branch: String,
    /// The name as its editor typed it.
    pub title: String,
    /// Whose branch (the first commit's author).
    pub author: String,
    /// `open` (nothing to merge), `pending`, `rejected` (until its next commit); closed ones:
    /// `merged`, `reset`, `deleted`.
    pub status: String,
    /// Its last commit.
    pub tip: String,
    /// The commit of `main` it started from.
    pub base: String,
    /// Unix time of its last commit.
    pub updated: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    /// A closed branch: how and by whom.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<Closed>,
    /// Oldest first.
    pub commits: Vec<BranchCommit>,
    pub records: Vec<BranchRecord>,
    /// How many records `main` changed since the branch started.
    pub conflicts: usize,
}

fn describe(dir: &Path, branch: &str, tip: &str, rv: &Review, closed: Option<Closed>) -> anyhow::Result<Branch> {
    let head = rev(dir, "HEAD").ok_or_else(|| anyhow::anyhow!("the working copy has no commit"))?;
    let base = git(dir, &["merge-base", &head, tip])?.trim().to_string();
    let commits = commits_of(dir, &base, tip);
    let records = net(dir, &base, tip, if closed.is_some() { &base } else { &head });
    let state = rv.branches.get(branch).cloned().unwrap_or_default();
    let decision = state.decision.filter(|d| d.tip == tip);
    let status = match &closed {
        Some(c) => c.status.clone(),
        None if records.is_empty() => "open".into(),
        None if decision.is_some() => "rejected".into(),
        None => "pending".into(),
    };
    let title = closed.as_ref().map(|c| c.title.clone()).filter(|t| !t.is_empty()).unwrap_or(state.title);
    Ok(Branch {
        branch: branch.to_string(),
        title: if title.is_empty() { branch.rsplit('/').next().unwrap_or(branch).to_string() } else { title },
        author: commits.first().map(|c| c.author.clone()).unwrap_or_else(|| owner_of(branch).unwrap_or("").to_string()),
        status,
        tip: tip.to_string(),
        base,
        updated: commits.last().map_or(0, |c| c.created),
        decision: if closed.is_some() { None } else { decision },
        conflicts: records.iter().filter(|r| r.conflict).count(),
        closed,
        commits,
        records,
    })
}

/// Every contributor branch: the open ones, then the closed ones (merged, reset, deleted),
/// each oldest first.
pub fn list(dir: &Path) -> anyhow::Result<Vec<Branch>> {
    let rv = review(dir);
    let mut open = Vec::new();
    for line in git(dir, &["for-each-ref", "--format=%(refname:short) %(objectname)", "refs/heads/contrib/"])?.lines() {
        let Some((branch, tip)) = line.split_once(' ') else { continue };
        open.push(describe(dir, branch, tip, &rv, None)?);
    }
    open.sort_by_key(|b| b.updated);
    let mut closed = Vec::new();
    for line in git(dir, &["for-each-ref", "--format=%(objectname)", HISTORY])?.lines() {
        let Some(c) = rv.closed.get(line.trim()) else { continue };
        closed.push(describe(dir, &c.branch.clone(), line.trim(), &rv, Some(c.clone()))?);
    }
    closed.sort_by_key(|b| b.closed.as_ref().map_or(0, |c| c.at));
    open.extend(closed);
    Ok(open)
}

/// One open branch.
pub fn get(dir: &Path, branch: &str) -> anyhow::Result<Option<Branch>> {
    let Some(tip) = rev(dir, &format!("refs/heads/{branch}")) else { return Ok(None) };
    Ok(Some(describe(dir, branch, &tip, &review(dir), None)?))
}

/// The records `author`'s open branches change (and `main` doesn't have so): each one's
/// values there (none: removed). A change of the working copy that has one of them is
/// committed, not uncommitted.
pub fn committed(dir: &Path, author: &str) -> BTreeMap<Key, Vec<Option<Value>>> {
    let mut out: BTreeMap<Key, Vec<Option<Value>>> = BTreeMap::new();
    let Some(head) = rev(dir, "HEAD") else { return out };
    let prefix = format!("refs/heads/contrib/{}/", crate::slug(author));
    for tip in git(dir, &["for-each-ref", "--format=%(objectname)", &prefix]).unwrap_or_default().lines() {
        let Ok(base) = git(dir, &["merge-base", &head, tip]) else { continue };
        for r in net(dir, base.trim(), tip, &head) {
            out.entry(r.key).or_default().push(r.after);
        }
    }
    out
}

// ------------------------------------------------------------------ committing

/// Commits `changes` (records of the working copy `work`, as they are now) to `author`'s
/// branch `name` (made from `main` when new; the name as typed is its title). `by`: who asked
/// (someone may commit an editor's changes for them; `Committed-By:` then). The new commit's
/// hash and the branch.
pub fn commit(work: &Work, changes: &[Change], author: &str, by: &str, name: &str, message: &str) -> anyhow::Result<(String, String)> {
    anyhow::ensure!(!changes.is_empty(), "nothing to commit");
    anyhow::ensure!(!message.trim().is_empty(), "a commit needs a message");
    anyhow::ensure!(!name.trim().is_empty(), "a commit goes on a branch: name it");
    let dir = work.dir();
    let branch = branch_of(author, name);
    let refname = format!("refs/heads/{branch}");
    let old = rev(dir, &refname);
    let base = match &old {
        Some(h) => h.clone(),
        None => rev(dir, "HEAD").ok_or_else(|| anyhow::anyhow!("the working copy has no commit"))?,
    };
    let hash = in_worktree(dir, &base, |tmp| {
        let mut t = Work::open(tmp)?;
        for c in changes {
            // a new record goes where it is in the working copy's file
            let before = if c.after.is_some() && t.get(&c.key).is_none() { work.next_in_file(&c.key).filter(|k| t.get(k).is_some()) } else { None };
            t.reset_before(&c.key, c.after.clone(), before.as_ref()).map_err(|e| anyhow::anyhow!("{} ({}): {e}", c.key.id, c.key.feature))?;
        }
        git(tmp, &["add", "-A"])?;
        anyhow::ensure!(git(tmp, &["diff", "--cached", "--quiet"]).is_err(), "nothing to commit: {branch} has these records as they are");
        let mut msg = format!("{}\n\nContent-Author: {author} <{}>", message.trim(), email_of(author));
        if by != author {
            msg.push_str(&format!("\nCommitted-By: {by}"));
        }
        let email = email_of(author);
        let env = [COMMITTER[0], COMMITTER[1], ("GIT_AUTHOR_NAME", author), ("GIT_AUTHOR_EMAIL", email.as_str())];
        git_env(tmp, &env, &["commit", "-q", "-m", &msg])?;
        rev(tmp, "HEAD").ok_or_else(|| anyhow::anyhow!("no commit made"))
    })?;
    git(dir, &["update-ref", &refname, &hash, old.as_deref().unwrap_or("")])?;
    let mut rv = review(dir);
    let state = rv.branches.entry(branch.clone()).or_default();
    if state.title.is_empty() || old.is_none() {
        state.title = name.trim().to_string();
    }
    save_review(dir, &rv)?;
    Ok((hash, branch))
}

// ------------------------------------------------------------------ admins and editors

/// Why a merge, rejection or reset didn't happen.
#[derive(Debug)]
pub enum ReviewError {
    NotFound,
    /// Nothing to merge (`open`).
    Empty,
    /// Records `main` changed since the branch started (merge again with `force`).
    Conflicts(Vec<String>),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for ReviewError {
    fn from(e: anyhow::Error) -> Self {
        ReviewError::Other(e)
    }
}

/// Moves a branch's commits to the history (its ref under [`HISTORY`], `closed` kept by tip).
fn close(dir: &Path, rv: &mut Review, b: &Branch, closed: Closed) -> anyhow::Result<()> {
    git(dir, &["update-ref", &format!("{HISTORY}{}/{}", b.branch.trim_start_matches("contrib/"), &b.tip[..12]), &b.tip])?;
    rv.closed.insert(b.tip.clone(), closed);
    if let Some(s) = rv.branches.get_mut(&b.branch) {
        s.decision = None;
    }
    Ok(())
}

/// Merges a branch into the working copy's branch (`HEAD`, `main`): one commit with its
/// records as the branch has them, authored as its editor, `message` (default: its commits'),
/// `Approved-By:`, `Lmoxide-Branch:` and `Lmoxide-Commits:`. With conflicts only when `force`.
/// The working tree's files stay as they are (the caller brings the records in line); the
/// branch is reset to the new `main`. `next_in_file`: where a new record goes (the working
/// copy's order). The branch as merged and the new commit.
pub fn merge(dir: &Path, branch: &str, approver: &str, message: Option<&str>, force: bool, now: u64, next_in_file: impl Fn(&Key) -> Option<Key>) -> Result<(Branch, String), ReviewError> {
    let b = get(dir, branch)?.ok_or(ReviewError::NotFound)?;
    if b.records.is_empty() {
        return Err(ReviewError::Empty);
    }
    if b.conflicts > 0 && !force {
        return Err(ReviewError::Conflicts(b.records.iter().filter(|r| r.conflict).map(|r| r.key.id.to_string()).collect()));
    }
    let head = rev(dir, "HEAD").ok_or_else(|| anyhow::anyhow!("the working copy has no commit"))?;
    let text = match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.to_string(),
        None if b.commits.len() == 1 => b.commits[0].message.clone(),
        None => format!("{}\n\n{}", b.title, b.commits.iter().map(|c| format!("- {}", c.subject)).collect::<Vec<_>>().join("\n")),
    };
    let mut msg = format!("{text}\n\nContent-Author: {} <{}>", b.author, email_of(&b.author));
    for by in b.commits.iter().map(|c| c.by.as_str()).filter(|by| *by != b.author).collect::<BTreeSet<_>>() {
        msg.push_str(&format!("\nCommitted-By: {by}"));
    }
    msg.push_str(&format!(
        "\nApproved-By: {approver}\nLmoxide-Branch: {}\nLmoxide-Commits: {}",
        b.branch,
        b.commits.iter().map(|c| &c.hash[..12]).collect::<Vec<_>>().join(", ")
    ));
    let author = b.author.clone();
    let records = b.records.clone();
    let new = in_worktree(dir, &head, |tmp| {
        let mut t = Work::open(tmp)?;
        for r in &records {
            let before = if r.after.is_some() && t.get(&r.key).is_none() { next_in_file(&r.key).filter(|k| t.get(k).is_some()) } else { None };
            t.reset_before(&r.key, r.after.clone(), before.as_ref()).map_err(|e| anyhow::anyhow!("{} ({}): {e}", r.key.id, r.key.feature))?;
        }
        git(tmp, &["add", "-A"])?;
        anyhow::ensure!(git(tmp, &["diff", "--cached", "--quiet"]).is_err(), "nothing to merge: main has these records so");
        let email = email_of(&author);
        let env = [COMMITTER[0], COMMITTER[1], ("GIT_AUTHOR_NAME", author.as_str()), ("GIT_AUTHOR_EMAIL", email.as_str())];
        git_env(tmp, &env, &["commit", "-q", "-m", &msg])?;
        rev(tmp, "HEAD").ok_or_else(|| anyhow::anyhow!("no commit made"))
    })?;
    git(dir, &["update-ref", "-m", &format!("merge {}", b.branch), "HEAD", &new, &head])?;
    // the index as HEAD has it; the files stay
    git(dir, &["reset", "-q"])?;
    git(dir, &["update-ref", &format!("refs/heads/{}", b.branch), &new, &b.tip])?;
    let mut rv = review(dir);
    let closed = Closed { branch: b.branch.clone(), title: b.title.clone(), status: "merged".into(), by: approver.into(), at: now, merged_as: Some(new.clone()), decision: rv.branches.get(&b.branch).and_then(|s| s.decision.clone()) };
    close(dir, &mut rv, &b, closed)?;
    save_review(dir, &rv)?;
    Ok((b, new))
}

/// Rejects a branch with the reason: it stays, rejected, until its next commit.
pub fn reject(dir: &Path, branch: &str, by: &str, reason: Option<String>, now: u64) -> Result<Branch, ReviewError> {
    let b = get(dir, branch)?.ok_or(ReviewError::NotFound)?;
    if b.records.is_empty() {
        return Err(ReviewError::Empty);
    }
    let mut rv = review(dir);
    rv.branches.entry(b.branch.clone()).or_default().decision = Some(Decision { status: "rejected".into(), by: by.into(), at: now, reason, tip: b.tip.clone() });
    save_review(dir, &rv)?;
    get(dir, branch)?.ok_or(ReviewError::NotFound)
}

/// Resets a branch to `main` (`delete`: removes it): its commits go to the history. The branch
/// as it was (its records: the caller puts the working copy's back as `main` has them).
pub fn reset(dir: &Path, branch: &str, by: &str, delete: bool, now: u64) -> Result<Branch, ReviewError> {
    let b = get(dir, branch)?.ok_or(ReviewError::NotFound)?;
    let head = rev(dir, "HEAD").ok_or_else(|| anyhow::anyhow!("the working copy has no commit"))?;
    let mut rv = review(dir);
    if !b.commits.is_empty() {
        let closed = Closed { branch: b.branch.clone(), title: b.title.clone(), status: if delete { "deleted" } else { "reset" }.into(), by: by.into(), at: now, merged_as: None, decision: rv.branches.get(&b.branch).and_then(|s| s.decision.clone()) };
        close(dir, &mut rv, &b, closed)?;
    }
    let refname = format!("refs/heads/{}", b.branch);
    if delete {
        git(dir, &["update-ref", "-d", &refname, &b.tip])?;
        rv.branches.remove(&b.branch);
    } else {
        git(dir, &["update-ref", &refname, &head, &b.tip])?;
    }
    save_review(dir, &rv)?;
    Ok(b)
}

/// How a branch downloads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    /// `git format-patch` of its commits not in `HEAD` (`git am` applies them).
    Patch,
    /// A git bundle of the branch (`git fetch <file> <branch>`), needing `HEAD`'s history.
    Bundle,
}

/// An open branch's commits, to download.
pub fn download(dir: &Path, branch: &str, format: Format) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(rev(dir, &format!("refs/heads/{branch}")).is_some(), "no branch {branch}");
    let range = format!("HEAD..{branch}");
    match format {
        Format::Patch => {
            let out = Command::new("git").arg("-C").arg(dir).args(["format-patch", "--stdout", &range]).output()?;
            anyhow::ensure!(out.status.success(), "git format-patch: {}", String::from_utf8_lossy(&out.stderr).trim());
            anyhow::ensure!(!out.stdout.is_empty(), "{branch} has no commits that main doesn't");
            Ok(out.stdout)
        }
        Format::Bundle => {
            let file = std::env::temp_dir().join(format!("lmoxide-bundle-{}-{}.bundle", std::process::id(), now_nanos()));
            let made = git(dir, &["bundle", "create", "-q", &file.to_string_lossy(), branch, "^HEAD"]);
            let bytes = made.and_then(|_| Ok(std::fs::read(&file)?));
            let _ = std::fs::remove_file(&file);
            bytes.map_err(|e| anyhow::anyhow!("{e:#} ({branch} has no commits that main doesn't?)"))
        }
    }
}

// ------------------------------------------------------------------ pushing

/// Where merged commits go (`Push` in the web UI).
#[derive(Debug, Clone, PartialEq)]
pub enum PushTarget {
    /// A checkout of the content (the play servers' `content/`): fast-forwarded to the working
    /// copy's `main`. Play servers take it in at their next start.
    Checkout(PathBuf),
    /// A git remote (a URL or path), its `branch`; `key`: an SSH key for it (a deploy key with
    /// write access, for GitHub).
    Remote { url: String, branch: String, key: Option<PathBuf> },
}

impl PushTarget {
    /// For people: the folder, or the remote and branch.
    pub fn describe(&self) -> String {
        match self {
            PushTarget::Checkout(dir) => dir.display().to_string(),
            PushTarget::Remote { url, branch, .. } => format!("{url} ({branch})"),
        }
    }
}

/// The last commit pushed to a remote target (pushing to a checkout reads the checkout).
const PUSHED: &str = "refs/lmoxide/pushed";

/// The commit `target` has now, when this checkout has it (else unknown).
fn target_head(dir: &Path, target: &PushTarget) -> Option<String> {
    let head = match target {
        PushTarget::Checkout(to) => git(to, &["rev-parse", "HEAD"]).ok()?.trim().to_string(),
        PushTarget::Remote { .. } => rev(dir, PUSHED)?,
    };
    git(dir, &["cat-file", "-e", &format!("{head}^{{commit}}")]).ok()?;
    Some(head)
}

/// How many commits of `HEAD` (merged) `target` doesn't have yet. Unknown when the target has
/// commits this checkout doesn't know, or a remote nothing was pushed to from here yet.
pub fn unpushed(dir: &Path, target: &PushTarget) -> Option<u64> {
    let theirs = target_head(dir, target)?;
    git(dir, &["rev-list", "--count", &format!("{theirs}..HEAD")]).ok()?.trim().parse().ok()
}

/// What a push did.
#[derive(Debug, Clone, Serialize)]
pub struct Pushed {
    pub target: String,
    /// Commits the target got.
    pub commits: u64,
    /// The target's commit before, and now.
    pub from: Option<String>,
    pub to: String,
}

/// Pushes the working copy's `HEAD` (`main`, with the merged branches) to `target`. A checkout
/// is only fast-forwarded: when it has commits `main` doesn't (or edits in files that changed),
/// nothing happens and the error says why.
pub fn push(dir: &Path, target: &PushTarget) -> anyhow::Result<Pushed> {
    let head = rev(dir, "HEAD").ok_or_else(|| anyhow::anyhow!("the working copy has no commit"))?;
    match target {
        PushTarget::Checkout(to) => {
            anyhow::ensure!(
                crate::git::is_repo(to) && git(to, &["rev-parse", "--show-prefix"])?.trim().is_empty(),
                "{} isn't a git checkout of the content (an image's or release's content can't take pushes: push to the content repository instead)",
                to.display()
            );
            let theirs = git(to, &["rev-parse", "HEAD"])?.trim().to_string();
            // (their commit here, to compare)
            git(dir, &["fetch", "-q", &std::fs::canonicalize(to)?.to_string_lossy(), "HEAD"])?;
            if theirs == head || git(dir, &["merge-base", "--is-ancestor", &head, &theirs]).is_ok() {
                return Ok(Pushed { target: target.describe(), commits: 0, from: Some(theirs.clone()), to: theirs });
            }
            anyhow::ensure!(
                git(dir, &["merge-base", "--is-ancestor", &theirs, &head]).is_ok(),
                "{} has commits the world editor's content doesn't (restart the server: it brings the editor's copy up to them first)",
                to.display()
            );
            let commits: u64 = git(dir, &["rev-list", "--count", &format!("{theirs}..{head}")])?.trim().parse()?;
            git(to, &["fetch", "-q", &std::fs::canonicalize(dir)?.to_string_lossy(), &head])?;
            git(to, &["merge", "--ff-only", "-q", &head]).map_err(|e| anyhow::anyhow!("{} couldn't take them (edits there in files they change?): {e:#}", to.display()))?;
            Ok(Pushed { target: target.describe(), commits, from: Some(theirs), to: head })
        }
        PushTarget::Remote { url, branch, key } => {
            let before = rev(dir, PUSHED);
            let commits = before.as_ref().and_then(|b| git(dir, &["rev-list", "--count", &format!("{b}..{head}")]).ok()).and_then(|n| n.trim().parse().ok()).unwrap_or(0);
            let ssh = key.as_ref().map(|k| format!("ssh -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new", k.display()));
            let env: Vec<(&str, &str)> = ssh.as_deref().map(|s| vec![("GIT_SSH_COMMAND", s)]).unwrap_or_default();
            git_env(dir, &env, &["push", "-q", url, &format!("{head}:refs/heads/{branch}")])?;
            git(dir, &["update-ref", PUSHED, &head])?;
            Ok(Pushed { target: target.describe(), commits, from: before, to: head })
        }
    }
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
        for slug in ["a", "b", "c"] {
            std::fs::write(dir.join(format!("npcs/{slug}.json")), canon::to_string(&json!({ "id": format!("npc/{slug}"), "level": 1 })).unwrap()).unwrap();
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

    fn of(work: &Work, who: &str, slug: &str) -> Vec<Change> {
        crate::git::changes(work).unwrap().into_iter().filter(|c| c.author == who && c.key.id.to_string() == format!("npc/{slug}")).collect()
    }

    fn lvl(v: &Option<Value>) -> u64 {
        v.as_ref().unwrap()["level"].as_u64().unwrap()
    }

    #[test]
    fn branches_reject_fix_merge() {
        let dir = tree("flow");
        let mut work = Work::open(&dir).unwrap();
        level(&mut work, "a", 2, "ann");
        level(&mut work, "b", 5, "ann");
        // two branches of ann's
        let (h1, b1) = commit(&work, &of(&work, "ann", "a"), "ann", "ann", "Isle of Yarr", "A to 2\n\nWhy: tests").unwrap();
        let (_, b2) = commit(&work, &of(&work, "ann", "b"), "ann", "root", "Other work", "B to 5").unwrap();
        assert_eq!((b1.as_str(), b2.as_str()), ("contrib/ann/isle-of-yarr", "contrib/ann/other-work"));
        // the working copy is untouched; both changes count as committed
        assert_eq!(crate::git::changes(&work).unwrap().len(), 2);
        assert_eq!(committed(&dir, "ann").len(), 2);
        let l = list(&dir).unwrap();
        let first = l.iter().find(|b| b.branch == b1).unwrap();
        assert_eq!((first.title.as_str(), first.author.as_str(), first.status.as_str()), ("Isle of Yarr", "ann", "pending"));
        assert_eq!(first.commits.len(), 1);
        assert_eq!((first.commits[0].hash.as_str(), first.commits[0].message.as_str()), (h1.as_str(), "A to 2\n\nWhy: tests"));
        assert_eq!(first.records.len(), 1);
        assert_eq!((lvl(&first.records[0].before), lvl(&first.records[0].after)), (1, 2));
        assert_eq!(l.iter().find(|b| b.branch == b2).unwrap().commits[0].by, "root");
        // rejected; the fix on the same branch reopens it, and merging takes both as one commit
        assert_eq!(reject(&dir, &b1, "root", Some("too low".into()), 5).unwrap().status, "rejected");
        level(&mut work, "a", 3, "ann");
        commit(&work, &of(&work, "ann", "a"), "ann", "ann", "Isle of Yarr", "A to 3").unwrap();
        let b = get(&dir, &b1).unwrap().unwrap();
        assert_eq!((b.status.as_str(), b.commits.len(), b.records.len()), ("pending", 2, 1));
        assert_eq!(lvl(&b.records[0].after), 3);
        let patch = String::from_utf8(download(&dir, &b1, Format::Patch).unwrap()).unwrap();
        assert!(patch.contains("A to 2") && patch.contains("A to 3"));
        assert!(!download(&dir, &b1, Format::Bundle).unwrap().is_empty());
        let (_, new) = merge(&dir, &b1, "root", None, false, 6, |_| None).unwrap();
        assert_eq!(rev(&dir, "HEAD").unwrap(), new);
        let body = git(&dir, &["show", "-s", "--format=%an|%B", &new]).unwrap();
        assert!(body.starts_with("ann|Isle of Yarr\n\n- A to 2\n- A to 3") && body.contains("Approved-By: root") && body.contains("Lmoxide-Branch: contrib/ann/isle-of-yarr"), "{body}");
        let at_head = records_at(&dir, "HEAD", &[PathBuf::from("npcs/a.json")]).remove(&Key::live("npc/a")).map(|x| x.0);
        assert_eq!(lvl(&at_head), 3);
        // the branch is back at main, open; the merge shows in the history
        let b = get(&dir, &b1).unwrap().unwrap();
        assert_eq!((b.status.as_str(), b.tip.as_str()), ("open", new.as_str()));
        let l = list(&dir).unwrap();
        let merged = l.iter().find(|b| b.status == "merged").unwrap();
        assert_eq!((merged.commits.len(), merged.records.len(), merged.closed.as_ref().unwrap().merged_as.as_deref()), (2, 1, Some(new.as_str())));
        assert_eq!(merged.closed.as_ref().unwrap().decision.as_ref().unwrap().reason.as_deref(), Some("too low"));
        // the working copy has npc/a as main does: only b is still a change
        let work = Work::open(&dir).unwrap();
        assert_eq!(crate::git::changes(&work).unwrap().iter().map(|c| c.key.id.to_string()).collect::<Vec<_>>(), ["npc/b"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_to_checkout_and_remote() {
        let dir = tree("push");
        let play = dir.with_file_name(format!("{}-play", dir.file_name().unwrap().to_string_lossy()));
        let bare = dir.with_file_name(format!("{}-bare", dir.file_name().unwrap().to_string_lossy()));
        let _ = std::fs::remove_dir_all(&play);
        let _ = std::fs::remove_dir_all(&bare);
        git(dir.parent().unwrap(), &["clone", "-q", &dir.to_string_lossy(), &play.to_string_lossy()]).unwrap();
        let target = PushTarget::Checkout(play.clone());
        assert_eq!(unpushed(&dir, &target), Some(0));
        let mut work = Work::open(&dir).unwrap();
        level(&mut work, "a", 9, "ann");
        let (_, b) = commit(&work, &of(&work, "ann", "a"), "ann", "ann", "x", "A to 9").unwrap();
        merge(&dir, &b, "root", None, false, 1, |_| None).unwrap();
        assert_eq!(unpushed(&dir, &target), Some(1));
        let p = push(&dir, &target).unwrap();
        assert_eq!(p.commits, 1);
        assert_eq!(unpushed(&dir, &target), Some(0));
        let played = records_of(&std::fs::read_to_string(play.join("npcs/a.json")).unwrap());
        assert_eq!(played[&Key::live("npc/a")]["level"], 9);
        assert_eq!(push(&dir, &target).unwrap().commits, 0);
        // a checkout with commits of its own: nothing to push while main has none it lacks,
        // refused once both have their own
        git(&play, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "theirs"]).unwrap();
        assert_eq!(push(&dir, &target).unwrap().commits, 0);
        level(&mut work, "b", 4, "ann");
        let (_, b) = commit(&work, &of(&work, "ann", "b"), "ann", "ann", "x", "B to 4").unwrap();
        merge(&dir, &b, "root", None, false, 2, |_| None).unwrap();
        assert!(push(&dir, &target).is_err());
        // (still counted: the push says why it can't go)
        assert_eq!(unpushed(&dir, &target), Some(1));
        // a remote: unknown until pushed from here once
        git(dir.parent().unwrap(), &["init", "-q", "--bare", &bare.to_string_lossy()]).unwrap();
        let remote = PushTarget::Remote { url: bare.to_string_lossy().into_owned(), branch: "main".into(), key: None };
        assert_eq!(unpushed(&dir, &remote), None);
        push(&dir, &remote).unwrap();
        assert_eq!(unpushed(&dir, &remote), Some(0));
        assert_eq!(git(&bare, &["rev-parse", "main"]).unwrap().trim(), rev(&dir, "HEAD").unwrap());
        for d in [&dir, &play, &bare] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn conflicts_and_reset() {
        let dir = tree("conflict");
        let mut work = Work::open(&dir).unwrap();
        level(&mut work, "c", 4, "ann");
        let (_, ann) = commit(&work, &of(&work, "ann", "c"), "ann", "ann", "work", "C to 4").unwrap();
        level(&mut work, "c", 7, "bob");
        let (_, bob) = commit(&work, &of(&work, "bob", "c"), "bob", "bob", "work", "C to 7").unwrap();
        merge(&dir, &bob, "root", None, false, 1, |_| None).unwrap();
        // main changed npc/c since ann's branch started: a conflict, merged only when forced
        let b = get(&dir, &ann).unwrap().unwrap();
        assert_eq!((b.conflicts, lvl(&b.records[0].before), lvl(&b.records[0].base)), (1, 7, 1));
        match merge(&dir, &ann, "root", None, false, 2, |_| None) {
            Err(ReviewError::Conflicts(ids)) => assert_eq!(ids, ["npc/c"]),
            other => panic!("{other:?}"),
        }
        // ann resets hers: open again, from main; a reset shows in the history
        let was = reset(&dir, &ann, "ann", false, 3).unwrap();
        assert_eq!(was.records.len(), 1);
        let b = get(&dir, &ann).unwrap().unwrap();
        assert_eq!((b.status.as_str(), b.commits.len()), ("open", 0));
        assert!(list(&dir).unwrap().iter().any(|b| b.status == "reset" && b.commits.len() == 1));
        reset(&dir, &ann, "ann", true, 4).unwrap();
        assert!(get(&dir, &ann).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
