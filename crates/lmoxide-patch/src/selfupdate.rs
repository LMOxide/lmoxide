//! Updating a patcher program from the [`release`](crate::release)s its server offers.
//!
//! Releases are signed with the server's manifest key, so a patcher trusts them as far as it
//! trusts that server's client files (which it runs too): [`check`] takes the key an install
//! already trusts. [`install`] downloads the new build next to the running program, checks it
//! against the signed sha256 and only then swaps it in, so a failed update leaves the old
//! program working. On Windows a running program can't be overwritten, only renamed: it moves
//! to `<exe>.old`, which [`cleanup`] deletes on the next start.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use ed25519_dalek::VerifyingKey;
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::{
    Error, Platform, Result,
    client::Client,
    release::{Release, compare_versions},
    sync::{Progress, ProgressFn},
};

/// The newest release of program `name` for `platform` that is newer than `current`, if the
/// server offers one. Every release must verify with `trusted`.
pub async fn check(client: &Client, name: &str, platform: Platform, current: &str, trusted: &VerifyingKey) -> Result<Option<Release>> {
    let mut best: Option<Release> = None;
    for signed in client.releases(platform).await? {
        let r = signed.verify(trusted)?;
        if r.name != name || r.platform != platform || compare_versions(&r.version, current).is_le() {
            continue;
        }
        if best.as_ref().is_none_or(|b| compare_versions(&r.version, &b.version).is_gt()) {
            best = Some(r);
        }
    }
    Ok(best)
}

/// `<exe>` with `suffix` appended to its file name.
fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = exe.file_name().unwrap_or_default().to_owned();
    name.push(suffix);
    exe.with_file_name(name)
}

/// Downloads `release` and puts it in place of `exe` (normally [`std::env::current_exe`]).
/// The running process keeps running the old build; start `exe` again to use the new one.
pub async fn install(client: &Client, release: &Release, exe: &Path, progress: &ProgressFn) -> Result<()> {
    let new = sibling(exe, ".new");
    let resp = client.file(&release.sha256, 0).await?;
    let label = format!("{} {}", release.name, release.version);
    progress(Progress::FileStarted { path: label.clone(), size: release.size, resumed_from: 0 });
    let mut file = tokio::fs::File::create(&new).await?;
    let mut stream = resp.bytes_stream();
    let mut got = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        got += chunk.len() as u64;
        if got > release.size {
            break;
        }
        file.write_all(&chunk).await?;
        progress(Progress::Bytes(chunk.len() as u64));
    }
    file.flush().await?;
    drop(file);
    let (new2, exe2, release2) = (new.clone(), exe.to_path_buf(), release.clone());
    let placed = tokio::task::spawn_blocking(move || -> Result<()> {
        let hash = crate::hashcache::sha256_file(&new2)?;
        if got != release2.size || hash != release2.sha256 {
            return Err(Error::HashMismatch { path: release2.name.clone(), expected: release2.sha256.clone(), got: hash });
        }
        copy_mode(&exe2, &new2)?;
        replace(&new2, &exe2)
    })
    .await
    .map_err(|e| Error::Other(e.to_string()))?;
    if placed.is_err() {
        let _ = fs::remove_file(&new);
    }
    placed?;
    progress(Progress::FileDone { path: label });
    progress(Progress::Done);
    Ok(())
}

#[cfg(unix)]
fn copy_mode(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(from).map(|m| m.permissions().mode()).unwrap_or(0o755);
    fs::set_permissions(to, fs::Permissions::from_mode(mode | 0o100))
}

#[cfg(not(unix))]
fn copy_mode(_: &Path, _: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Puts `new` in place of `exe`.
fn replace(new: &Path, exe: &Path) -> Result<()> {
    if cfg!(windows) {
        let old = sibling(exe, ".old");
        let _ = fs::remove_file(&old);
        fs::rename(exe, &old)?;
        if let Err(e) = fs::rename(new, exe) {
            let _ = fs::rename(&old, exe);
            return Err(e.into());
        }
    } else {
        fs::rename(new, exe)?;
    }
    Ok(())
}

/// Removes what an earlier [`install`] left next to `exe` (the replaced build on Windows, a
/// download that never finished). Call it at startup.
pub fn cleanup(exe: &Path) {
    for suffix in [".old", ".new"] {
        let _ = fs::remove_file(sibling(exe, suffix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("patcher");
        fs::write(&exe, b"old").unwrap();
        let new = sibling(&exe, ".new");
        fs::write(&new, b"new").unwrap();
        copy_mode(&exe, &new).unwrap();
        replace(&new, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert!(!new.exists());
        fs::write(sibling(&exe, ".old"), b"x").unwrap();
        cleanup(&exe);
        assert!(!sibling(&exe, ".old").exists());
        assert!(exe.exists());
    }
}
