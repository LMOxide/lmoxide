//! The LMOxide patcher protocol, shared by `lmoxide-patch-server` and the patchers (the
//! `lmoxide-patcher` CLI, `lmoxide-patcher-gui`).
//!
//! The stock client's own patcher only knows Funcom's RDB layout and can't install a client or
//! change its executable, so LMOxide servers also offer a simple file-sync protocol under
//! `/lmoxide/v1/` (see `docs/PATCHING.md`):
//!
//! - `GET /lmoxide/v1/info`: [`Info`] (server name and addresses, platforms, keys)
//! - `GET /lmoxide/v1/manifest/{platform}`: a [`SignedManifest`] wrapping the [`Manifest`] of
//!   one platform's complete install, already patched for this server
//! - `GET /lmoxide/v1/files/{sha256}`: file bodies by content hash (HTTP Range supported)
//! - `GET /lmoxide/v1/patchers/{platform}`: signed [`release`]s of the patchers themselves
//!
//! Manifests are signed with the server's ed25519 key, so a patcher that pinned that key can
//! fetch everything over plain HTTP (which the stock client needs anyway) and still detect
//! tampering: file bodies are checked against the signed sha256.
//!
//! Without the default `client` feature this crate has only the types, signing and the
//! [`hashcache`]; with it, [`client`] (HTTP), [`sync`] (plan, download, verify) and
//! [`selfupdate`] (replacing a patcher with a newer release).

pub mod hashcache;
pub mod manifest;
pub mod release;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod selfupdate;
#[cfg(feature = "client")]
pub mod sync;

pub use manifest::{FileEntry, Info, Launch, Manifest, SignedManifest};

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Version of the protocol under `/lmoxide/v1/`; bumped on incompatible changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Where the patcher keeps its state inside an install directory (trusted key, hash cache,
/// partial downloads, the last manifest). Never listed in a manifest.
pub const STATE_DIR: &str = ".lmoxide";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsafe path in manifest: {0:?}")]
    UnsafePath(String),
    #[error("invalid key: {0}")]
    BadKey(String),
    #[error("signed by {got}, expected {expected}")]
    KeyMismatch { expected: String, got: String },
    #[error("bad manifest signature")]
    BadSignature,
    #[error("unsupported manifest format {0}")]
    Format(u32),
    #[error("manifest is for {got}, asked for {expected}")]
    WrongPlatform { expected: Platform, got: Platform },
    #[error("malformed manifest: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[cfg(feature = "client")]
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    /// Connecting failed or timed out (`url` is the server's base URL).
    #[cfg(feature = "client")]
    #[error("can't reach {url}: the server is down or the address is wrong")]
    Unreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// Stopped through [`sync::Options::cancel`].
    #[error("cancelled")]
    Cancelled,
    #[error("server answered {status} for {url}")]
    Status { status: u16, url: String },
    #[error("the server is still indexing its client files; try again in {retry_after} s")]
    NotReady { retry_after: u64 },
    #[error("{path}: downloaded data has sha256 {got}, expected {expected}")]
    HashMismatch { path: String, expected: String, got: String },
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A client build the server can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
}

impl Platform {
    pub const ALL: [Platform; 2] = [Platform::Windows, Platform::Linux];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Windows => "windows",
            Platform::Linux => "linux",
        }
    }

    /// The build that runs natively here (Windows elsewhere, e.g. under Wine on macOS).
    pub fn host() -> Self {
        if cfg!(target_os = "linux") { Platform::Linux } else { Platform::Windows }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "windows" | "win" | "win32" => Ok(Platform::Windows),
            "linux" => Ok(Platform::Linux),
            _ => Err(format!("unknown platform {s:?} (windows, linux)")),
        }
    }
}

/// Checks that a manifest path is a plain relative path that stays inside the install dir:
/// `/`-separated, no empty, `.` or `..` components, no leading `/`, no `\`, `:` (Windows drive
/// letters, alternate streams) or control characters, and not inside [`STATE_DIR`].
pub fn check_path(path: &str) -> Result<()> {
    let bad = || Error::UnsafePath(path.to_string());
    if path.is_empty() || path.len() > 1024 || path.chars().any(|c| c == '\\' || c == ':' || c.is_control()) {
        return Err(bad());
    }
    for (i, part) in path.split('/').enumerate() {
        if part.is_empty() || part == "." || part == ".." || (i == 0 && part.eq_ignore_ascii_case(STATE_DIR)) {
            return Err(bad());
        }
    }
    Ok(())
}

/// Where patchers install by default: `~/Games/LMO` (under `%USERPROFILE%` on Windows).
/// `None` without a home directory.
pub fn default_install_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(std::path::PathBuf::from(home).join("Games").join("LMO"))
}

/// Lower-case hex sha256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(data))
}

/// Whether `s` looks like a lower-case hex sha256.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        for ok in ["LMO.exe", "RDB/1010001/ABC.rdbres", "Data/startup.lua", "a b/c"] {
            check_path(ok).unwrap();
        }
        for bad in [
            "", "/etc/passwd", "../x", "a/../b", "a//b", "./a", "a/.", "C:/x", "a\\b", "a\0b", ".lmoxide/key",
            ".LMOXIDE/x", "x/",
        ] {
            assert!(check_path(bad).is_err(), "{bad:?}");
        }
        // only the top-level state dir is reserved
        check_path("Data/.lmoxide").unwrap();
    }

    #[test]
    fn platform_parse() {
        assert_eq!("Linux".parse::<Platform>().unwrap(), Platform::Linux);
        assert_eq!(serde_json::to_string(&Platform::Windows).unwrap(), "\"windows\"");
        assert!("mac".parse::<Platform>().is_err());
    }
}
