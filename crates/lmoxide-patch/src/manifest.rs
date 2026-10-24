//! Protocol documents and manifest signing.
//!
//! A manifest travels inside a [`SignedManifest`] envelope that carries the manifest JSON as a
//! *string*: the signature covers exactly those bytes, so nobody has to re-serialize JSON
//! canonically to check it. The signed message is [`SIGNATURE_CONTEXT`] followed by the
//! manifest's UTF-8 bytes (the context keeps a manifest signature from being valid for anything
//! else signed with the same key). Keys and signatures are lower-case hex.

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{Error, Platform, Result, check_path, is_sha256_hex};

/// Current [`Manifest::format`] and [`SignedManifest::format`].
pub const MANIFEST_FORMAT: u32 = 1;

/// Prefix of every signed message (domain separation).
pub const SIGNATURE_CONTEXT: &[u8] = b"lmoxide-manifest-v1\0";

/// `GET /lmoxide/v1/info`. Unsigned: informational, the manifest repeats what matters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Info {
    /// [`crate::PROTOCOL_VERSION`].
    pub protocol: u32,
    /// Display name of the server.
    pub name: String,
    /// The game server's universe agent, `host:port` (the client's `UniverseAddr`).
    pub universe_addr: String,
    /// Public plain-HTTP address of this patch server, `host:port`.
    pub http_addr: String,
    /// Public HTTPS address, if built-in TLS is on.
    #[serde(default)]
    pub https_addr: Option<String>,
    /// Platforms with a manifest (or one being built).
    pub platforms: Vec<Platform>,
    /// Platforms whose manifest is ready (the rest are still being indexed).
    #[serde(default)]
    pub ready: Vec<Platform>,
    /// ed25519 public key manifests are signed with (hex, 64 chars).
    pub manifest_key: String,
    /// The game server's login public key (hex, 512 chars) patched into the client binaries, or
    /// `None` if the patch server doesn't have it (binaries are then served unpatched).
    pub login_public_key: Option<String>,
    /// Human-readable warnings for the host (e.g. missing login key).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// [`Manifest::revision`] of each ready platform's manifest: compare it with the installed
    /// manifest's to see whether there's an update without downloading the manifest (a hint;
    /// only the signed manifest counts).
    #[serde(default)]
    pub revisions: BTreeMap<Platform, String>,
    /// Size of each ready platform's complete install in bytes (a hint, as `revisions`).
    #[serde(default)]
    pub install_bytes: BTreeMap<Platform, u64>,
    /// Public base URL of the server's REST API (lmoxide-api), whose `GET /v1/status` says
    /// whether the game server is up and `GET /v1/config` whether registration is open.
    #[serde(default)]
    pub api_url: Option<String>,
    /// The server's website (news, account registration).
    #[serde(default)]
    pub website: Option<String>,
    /// Patcher builds the server hands out (`GET /lmoxide/v1/patchers/{platform}`): name (e.g.
    /// `lmoxide-patcher-gui`) → platform → newest version.
    #[serde(default)]
    pub patchers: BTreeMap<String, BTreeMap<Platform, String>>,
}

/// One platform's complete install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`MANIFEST_FORMAT`].
    pub format: u32,
    /// Server display name (as in [`Info::name`]).
    pub server: String,
    pub platform: Platform,
    /// Hash over the file list (sha256 of `path\0sha256\0size\0exec\n` lines), so patchers
    /// can tell quickly whether anything changed.
    pub version: String,
    /// Unix time the manifest was built.
    pub generated_at: u64,
    /// The login public key patched into the executable (`None`: unpatched).
    pub login_public_key: Option<String>,
    /// The game server's universe agent, `host:port`.
    pub universe_addr: String,
    /// Every file of the install, sorted by path.
    pub files: Vec<FileEntry>,
    /// Files to delete from an install if present (e.g. libraries the server's build replaces).
    #[serde(default)]
    pub remove: Vec<String>,
    pub launch: Launch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative, `/`-separated (see [`crate::check_path`]).
    pub path: String,
    pub size: u64,
    /// Lower-case hex; the body is at `/lmoxide/v1/files/{sha256}`.
    pub sha256: String,
    /// Set the executable bits (unix).
    #[serde(default)]
    pub executable: bool,
}

/// How to start the client once installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Launch {
    /// Program to run, relative to the install dir; also the working directory's root.
    pub executable: String,
    /// Fixed arguments.
    pub args: Vec<String>,
    /// Arguments for automatic login; `{username}` and `{password}` are substituted.
    #[serde(default)]
    pub login_args: Vec<String>,
    /// Environment variables to set (values relative to the install dir where they are paths).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl Launch {
    /// The command that starts the client installed in `dir`: the executable (checked to be
    /// inside `dir` and present) with `args`, then `login_args` with `{username}` and
    /// `{password}` substituted when `login` is given, the `env` variables, and `dir` as the
    /// working directory (where relative paths in `env` are resolved). `wrapper` is a program
    /// and arguments to run the executable with (e.g. `["wine"]` for the Windows client
    /// elsewhere); empty runs it directly. Stdio is inherited.
    #[cfg(feature = "client")]
    pub fn command(&self, dir: &std::path::Path, login: Option<(&str, &str)>, wrapper: &[&str]) -> Result<std::process::Command> {
        if self.executable.is_empty() {
            return Err(Error::Other("the server's manifest doesn't say how to start the client".into()));
        }
        let exe = crate::sync::safe_join(dir, &self.executable)?;
        if !exe.is_file() {
            return Err(Error::Other(format!("{} is missing; update the install", self.executable)));
        }
        let mut cmd = match wrapper.split_first() {
            Some((prog, args)) => {
                let mut c = std::process::Command::new(prog);
                c.args(args).arg(&exe);
                c
            }
            None => std::process::Command::new(&exe),
        };
        cmd.args(&self.args);
        if let Some((username, password)) = login {
            cmd.args(self.login_args.iter().map(|a| a.replace("{username}", username).replace("{password}", password)));
        }
        cmd.envs(&self.env).current_dir(dir);
        Ok(cmd)
    }
}

impl Manifest {
    /// The [`Manifest::version`] of a file list.
    pub fn version_of(files: &[FileEntry]) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for f in files {
            h.update(format!("{}\0{}\0{}\0{}\n", f.path, f.sha256, f.size, f.executable as u8));
        }
        hex::encode(h.finalize())
    }

    /// Identifies the manifest's content apart from when it was built: sha256 of its JSON with
    /// `generated_at` zeroed. Servers send it in [`Info::revisions`] and as the manifest's
    /// `ETag`, so a patcher can compare it with the installed manifest's.
    pub fn revision(&self) -> String {
        let m = Manifest { generated_at: 0, ..self.clone() };
        crate::sha256_hex(&serde_json::to_vec(&m).expect("manifests serialize"))
    }

    /// Checks paths and hashes; run on every manifest from the network.
    pub fn validate(&self) -> Result<()> {
        if self.format != MANIFEST_FORMAT {
            return Err(Error::Format(self.format));
        }
        let mut seen = std::collections::HashSet::new();
        for f in &self.files {
            check_path(&f.path)?;
            if !is_sha256_hex(&f.sha256) {
                return Err(Error::Other(format!("{}: bad sha256", f.path)));
            }
            // Windows file systems are case-insensitive and can't hold both (the Linux client
            // has both `LMO` and the kit's `lmo` launcher).
            let key = match self.platform {
                Platform::Windows => f.path.to_ascii_lowercase(),
                Platform::Linux => f.path.clone(),
            };
            if !seen.insert(key) {
                return Err(Error::Other(format!("{}: listed twice", f.path)));
            }
        }
        for r in &self.remove {
            check_path(r)?;
        }
        if !self.launch.executable.is_empty() {
            check_path(&self.launch.executable)?;
        }
        Ok(())
    }
}

/// `GET /lmoxide/v1/manifest/{platform}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    /// [`MANIFEST_FORMAT`].
    pub format: u32,
    /// Signing public key, hex.
    pub key: String,
    /// ed25519 signature over [`SIGNATURE_CONTEXT`] + `manifest`, hex.
    pub signature: String,
    /// The [`Manifest`] as JSON.
    pub manifest: String,
}

impl SignedManifest {
    pub fn sign(manifest: &Manifest, key: &SigningKey) -> Result<Self> {
        let json = serde_json::to_string(manifest)?;
        Ok(Self { format: MANIFEST_FORMAT, key: public_key_hex(&key.verifying_key()), signature: sign(SIGNATURE_CONTEXT, &json, key), manifest: json })
    }

    /// The signing key this envelope claims (only meaningful once verified against a trusted key).
    pub fn claimed_key(&self) -> Result<VerifyingKey> {
        parse_public_key(&self.key)
    }

    /// Verifies the signature with `trusted` and returns the validated manifest.
    pub fn verify(&self, trusted: &VerifyingKey) -> Result<Manifest> {
        if self.format != MANIFEST_FORMAT {
            return Err(Error::Format(self.format));
        }
        verify(SIGNATURE_CONTEXT, &self.manifest, &self.key, &self.signature, trusted)?;
        let manifest: Manifest = serde_json::from_str(&self.manifest)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// [`SignedManifest::verify`], also checking the manifest is for `platform`.
    pub fn verify_for(&self, trusted: &VerifyingKey, platform: Platform) -> Result<Manifest> {
        let m = self.verify(trusted)?;
        if m.platform != platform {
            return Err(Error::WrongPlatform { expected: platform, got: m.platform });
        }
        Ok(m)
    }
}

/// Hex ed25519 signature over `context` + `json`.
pub(crate) fn sign(context: &[u8], json: &str, key: &SigningKey) -> String {
    hex::encode(key.sign(&[context, json.as_bytes()].concat()).to_bytes())
}

/// Checks that `key` (hex, as an envelope names it) is `trusted` and that `signature` (hex) is
/// its signature over `context` + `json`.
pub(crate) fn verify(context: &[u8], json: &str, key: &str, signature: &str, trusted: &VerifyingKey) -> Result<()> {
    let expected = public_key_hex(trusted);
    if !key.eq_ignore_ascii_case(&expected) {
        return Err(Error::KeyMismatch { expected, got: key.to_string() });
    }
    let sig: [u8; 64] = hex::decode(signature).ok().and_then(|b| b.try_into().ok()).ok_or(Error::BadSignature)?;
    trusted.verify_strict(&[context, json.as_bytes()].concat(), &Signature::from_bytes(&sig)).map_err(|_| Error::BadSignature)
}

/// A new random signing key.
pub fn generate_signing_key() -> SigningKey {
    SigningKey::from_bytes(&rand::random())
}

/// The secret key as stored by the server (hex of the 32-byte seed).
pub fn signing_key_hex(key: &SigningKey) -> String {
    hex::encode(key.to_bytes())
}

pub fn parse_signing_key(s: &str) -> Result<SigningKey> {
    let bytes: [u8; 32] = hex::decode(s.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| Error::BadKey("signing key must be 64 hex characters".into()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub fn public_key_hex(key: &VerifyingKey) -> String {
    hex::encode(key.to_bytes())
}

/// Parses a public key: 64 hex characters, optionally prefixed with `ed25519:`.
pub fn parse_public_key(s: &str) -> Result<VerifyingKey> {
    let s = s.trim();
    let s = s.strip_prefix("ed25519:").unwrap_or(s);
    let bytes: [u8; 32] = hex::decode(s)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| Error::BadKey("public key must be 64 hex characters".into()))?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| Error::BadKey(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        let files = vec![FileEntry {
            path: "LMO.exe".into(),
            size: 3,
            sha256: crate::sha256_hex(b"abc"),
            executable: true,
        }];
        Manifest {
            format: MANIFEST_FORMAT,
            server: "test".into(),
            platform: Platform::Windows,
            version: Manifest::version_of(&files),
            generated_at: 1,
            login_public_key: None,
            universe_addr: "127.0.0.1:7100".into(),
            files,
            remove: vec!["old.dll".into()],
            launch: Launch { executable: "LMO.exe".into(), args: vec!["-verified".into()], ..Default::default() },
        }
    }

    #[test]
    fn sign_and_verify() {
        let key = generate_signing_key();
        let signed = SignedManifest::sign(&sample(), &key).unwrap();
        // survives a JSON round trip of the envelope
        let signed: SignedManifest = serde_json::from_str(&serde_json::to_string(&signed).unwrap()).unwrap();
        let pk = parse_public_key(&public_key_hex(&key.verifying_key())).unwrap();
        assert_eq!(signed.verify(&pk).unwrap(), sample());
        assert_eq!(signed.claimed_key().unwrap(), pk);
        assert!(matches!(
            signed.verify_for(&pk, Platform::Linux),
            Err(Error::WrongPlatform { .. })
        ));
    }

    #[test]
    fn tampering_detected() {
        let key = generate_signing_key();
        let pk = key.verifying_key();
        let signed = SignedManifest::sign(&sample(), &key).unwrap();

        let mut t = signed.clone();
        t.manifest = t.manifest.replace("old.dll", "new.dll");
        assert!(matches!(t.verify(&pk), Err(Error::BadSignature)));

        // a different key, even if the envelope names it
        let other = generate_signing_key();
        let forged = SignedManifest::sign(&sample(), &other).unwrap();
        assert!(matches!(forged.verify(&pk), Err(Error::KeyMismatch { .. })));

        let mut t = signed.clone();
        t.key = public_key_hex(&other.verifying_key());
        assert!(t.verify(&other.verifying_key()).is_err());
    }

    #[test]
    fn unsafe_manifest_rejected_even_if_signed() {
        let key = generate_signing_key();
        let mut m = sample();
        m.files[0].path = "../evil".into();
        let signed = SignedManifest::sign(&m, &key).unwrap();
        assert!(matches!(signed.verify(&key.verifying_key()), Err(Error::UnsafePath(_))));
    }

    #[test]
    fn case_collisions_only_matter_on_windows() {
        let mut m = sample();
        let mut lower = m.files[0].clone();
        lower.path = "lmo.exe".into();
        m.files.push(lower);
        assert!(m.validate().is_err());
        m.platform = Platform::Linux; // the Linux client has both `LMO` and `lmo`
        m.validate().unwrap();
        m.files.push(m.files[0].clone());
        assert!(m.validate().is_err());
    }

    #[test]
    fn revision_ignores_build_time() {
        let mut m = sample();
        let r = m.revision();
        m.generated_at = 99;
        assert_eq!(m.revision(), r);
        m.launch.args.clear();
        assert_ne!(m.revision(), r);
    }

    #[test]
    fn info_without_new_fields() {
        let info: Info = serde_json::from_str(
            r#"{"protocol":1,"name":"t","universe_addr":"h:1","http_addr":"h:2","platforms":["linux"],
                "manifest_key":"00","login_public_key":null}"#,
        )
        .unwrap();
        assert!(info.revisions.is_empty() && info.api_url.is_none());
        let mut info = info;
        info.revisions.insert(Platform::Linux, "abc".into());
        assert!(serde_json::to_string(&info).unwrap().contains(r#""revisions":{"linux":"abc"}"#));
    }

    #[cfg(all(unix, feature = "client"))]
    #[test]
    fn launch_command() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().canonicalize().unwrap();
        std::fs::write(d.join("LMO"), "#!/bin/sh\necho \"$PWD|$GREETING|$*\"\n").unwrap();
        std::fs::set_permissions(d.join("LMO"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut l = Launch {
            executable: "LMO".into(),
            args: vec!["-verified".into()],
            login_args: ["-username", "{username}", "-password", "{password}"].map(String::from).to_vec(),
            env: BTreeMap::from([("GREETING".into(), "hi".into())]),
        };
        let run = |l: &Launch, login, wrapper: &[&str]| {
            let out = l.command(&d, login, wrapper).unwrap().output().unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let pwd = d.display();
        assert_eq!(run(&l, Some(("blake", "p w")), &[]), format!("{pwd}|hi|-verified -username blake -password p w\n"));
        assert_eq!(run(&l, None, &[]), format!("{pwd}|hi|-verified\n"));
        assert_eq!(run(&l, None, &["sh"]), format!("{pwd}|hi|-verified\n"));
        l.executable = "../LMO".into();
        assert!(l.command(&d, None, &[]).is_err());
        l.executable = "Missing".into();
        assert!(l.command(&d, None, &[]).is_err());
    }

    #[test]
    fn key_encodings() {
        let key = generate_signing_key();
        assert_eq!(parse_signing_key(&signing_key_hex(&key)).unwrap().to_bytes(), key.to_bytes());
        let pk = public_key_hex(&key.verifying_key());
        assert_eq!(parse_public_key(&format!("ed25519:{pk}")).unwrap(), key.verifying_key());
        assert!(parse_public_key("abcd").is_err());
    }
}
