//! Patcher releases: builds of a patcher (`lmoxide-patcher`, `lmoxide-patcher-gui`) a server
//! hands out so patchers can update themselves (`GET /lmoxide/v1/patchers/{platform}`).
//!
//! A [`Release`] travels in a [`SignedRelease`] envelope like a manifest, signed with the same
//! server key but over [`RELEASE_CONTEXT`], so a manifest signature can never pass for a release
//! or the other way round. The body is served by `GET /lmoxide/v1/files/{sha256}` like any file.
//!
//! The server finds releases by file name: `<name>-<version>-<platform>[.exe]`, e.g.
//! `lmoxide-patcher-gui-0.2.0-linux` or `lmoxide-patcher-gui-0.2.0-windows.exe` ([`parse_file_name`]).

use std::cmp::Ordering;

use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{
    Error, Platform, Result, is_sha256_hex,
    manifest::{self, public_key_hex},
};

/// Current [`Release::format`] and [`SignedRelease::format`].
pub const RELEASE_FORMAT: u32 = 1;

/// Prefix of every signed release (domain separation from manifests).
pub const RELEASE_CONTEXT: &[u8] = b"lmoxide-release-v1\0";

/// One build of a patcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// [`RELEASE_FORMAT`].
    pub format: u32,
    /// Program name, e.g. `lmoxide-patcher-gui`.
    pub name: String,
    /// Dotted version, e.g. `0.2.0` (compared with [`compare_versions`]).
    pub version: String,
    pub platform: Platform,
    pub size: u64,
    /// Lower-case hex; the body is at `/lmoxide/v1/files/{sha256}`.
    pub sha256: String,
    /// Unix time the release was signed.
    pub generated_at: u64,
}

/// `GET /lmoxide/v1/patchers/{platform}` answers a list of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRelease {
    /// [`RELEASE_FORMAT`].
    pub format: u32,
    /// Signing public key, hex.
    pub key: String,
    /// ed25519 signature over [`RELEASE_CONTEXT`] + `release`, hex.
    pub signature: String,
    /// The [`Release`] as JSON.
    pub release: String,
}

impl SignedRelease {
    pub fn sign(release: &Release, key: &SigningKey) -> Result<Self> {
        let json = serde_json::to_string(release)?;
        Ok(Self {
            format: RELEASE_FORMAT,
            key: public_key_hex(&key.verifying_key()),
            signature: manifest::sign(RELEASE_CONTEXT, &json, key),
            release: json,
        })
    }

    /// Verifies the signature with `trusted` and returns the release.
    pub fn verify(&self, trusted: &VerifyingKey) -> Result<Release> {
        if self.format != RELEASE_FORMAT {
            return Err(Error::Format(self.format));
        }
        manifest::verify(RELEASE_CONTEXT, &self.release, &self.key, &self.signature, trusted)?;
        let r: Release = serde_json::from_str(&self.release)?;
        if r.format != RELEASE_FORMAT {
            return Err(Error::Format(r.format));
        }
        if !is_sha256_hex(&r.sha256) {
            return Err(Error::Other(format!("{}: bad sha256", r.name)));
        }
        Ok(r)
    }
}

/// Splits a release file name `<name>-<version>-<platform>[.exe]` (the version is dotted
/// numbers; `.exe` only for Windows).
pub fn parse_file_name(file: &str) -> Option<(String, String, Platform)> {
    let (stem, exe) = match file.strip_suffix(".exe") {
        Some(s) => (s, true),
        None => (file, false),
    };
    let (rest, platform) = stem.rsplit_once('-')?;
    let platform: Platform = platform.parse().ok().filter(|p: &Platform| p.as_str() == platform)?;
    if exe != (platform == Platform::Windows) {
        return None;
    }
    let (name, version) = rest.rsplit_once('-')?;
    let valid = !name.is_empty() && !version.is_empty() && version.split('.').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    valid.then(|| (name.to_string(), version.to_string(), platform))
}

/// Orders dotted versions numerically (`0.10.0` > `0.9.1`; missing parts count as 0).
/// Parts that aren't numbers compare as 0.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parse = |v: &str| -> Vec<u64> { v.split('.').map(|n| n.parse().unwrap_or(0)).collect() };
    let (a, b) = (parse(a), parse(b));
    for i in 0..a.len().max(b.len()) {
        match a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{SignedManifest, generate_signing_key};

    fn release() -> Release {
        Release {
            format: RELEASE_FORMAT,
            name: "lmoxide-patcher-gui".into(),
            version: "0.2.0".into(),
            platform: Platform::Linux,
            size: 3,
            sha256: crate::sha256_hex(b"abc"),
            generated_at: 1,
        }
    }

    #[test]
    fn sign_and_verify() {
        let key = generate_signing_key();
        let signed = SignedRelease::sign(&release(), &key).unwrap();
        assert_eq!(signed.verify(&key.verifying_key()).unwrap(), release());
        let other = generate_signing_key();
        assert!(matches!(signed.verify(&other.verifying_key()), Err(Error::KeyMismatch { .. })));
        let mut t = signed.clone();
        t.release = t.release.replace("0.2.0", "9.9.9");
        assert!(matches!(t.verify(&key.verifying_key()), Err(Error::BadSignature)));
    }

    /// A manifest signature doesn't verify as a release (different context).
    #[test]
    fn contexts_differ() {
        let key = generate_signing_key();
        let json = serde_json::to_string(&release()).unwrap();
        let as_manifest = SignedManifest { format: 1, key: public_key_hex(&key.verifying_key()), signature: manifest::sign(manifest::SIGNATURE_CONTEXT, &json, &key), manifest: json.clone() };
        let forged = SignedRelease { format: 1, key: as_manifest.key, signature: as_manifest.signature, release: json };
        assert!(matches!(forged.verify(&key.verifying_key()), Err(Error::BadSignature)));
    }

    #[test]
    fn file_names() {
        assert_eq!(
            parse_file_name("lmoxide-patcher-gui-0.2.0-linux"),
            Some(("lmoxide-patcher-gui".into(), "0.2.0".into(), Platform::Linux))
        );
        assert_eq!(
            parse_file_name("lmoxide-patcher-1.10-windows.exe"),
            Some(("lmoxide-patcher".into(), "1.10".into(), Platform::Windows))
        );
        for bad in ["lmoxide-patcher-gui-linux", "x-0.1-windows", "x-0.1-linux.exe", "x-0.a-linux", "-0.1-linux", "x-0.1-Linux", "README"] {
            assert_eq!(parse_file_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn versions() {
        assert_eq!(compare_versions("0.10.0", "0.9.1"), Ordering::Greater);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.1.0", "0.1.1"), Ordering::Less);
    }
}
