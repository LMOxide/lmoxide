//! Build information for [`lmoxide_control::Build`]: the git commit (`LMOXIDE_GIT_COMMIT` and
//! `LMOXIDE_GIT_COMMIT_TIME` when set, as Docker builds have no `.git`; else asked of git), its
//! time, and when this ran (`LMOXIDE_BUILD_TIME` or now).

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn env(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn main() {
    let given = env("LMOXIDE_GIT_COMMIT");
    let given_time = env("LMOXIDE_GIT_COMMIT_TIME");
    let built = env("LMOXIDE_BUILD_TIME");
    let mut commit = given.clone();
    let mut commit_time = given_time;
    if given.is_none() {
        commit = git(&["rev-parse", "HEAD"]);
        commit_time = commit.as_ref().and_then(|_| git(&["log", "-1", "--format=%ct"]));
        // run again when HEAD moves (a commit, a checkout); only files that exist, as a missing
        // one would make it run on every build
        let mut watch = vec![git(&["rev-parse", "--git-path", "HEAD"]), git(&["rev-parse", "--git-path", "packed-refs"])];
        if let Some(r) = git(&["rev-parse", "--symbolic-full-name", "HEAD"]).filter(|r| r.starts_with("refs/")) {
            watch.push(git(&["rev-parse", "--git-path", &r]));
        }
        for path in watch.into_iter().flatten().filter(|p| std::path::Path::new(p).exists()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    let built = built.unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).to_string());
    println!("cargo:rustc-env=LMOXIDE_GIT_COMMIT={}", commit.unwrap_or_default());
    println!("cargo:rustc-env=LMOXIDE_GIT_COMMIT_TIME={}", commit_time.unwrap_or_default());
    println!("cargo:rustc-env=LMOXIDE_BUILD_TIME={built}");
}
