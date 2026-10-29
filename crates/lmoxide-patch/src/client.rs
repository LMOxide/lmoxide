//! HTTP side of the protocol, and deciding which manifest key to trust.

use std::path::{Path, PathBuf};

use ed25519_dalek::VerifyingKey;
use reqwest::{StatusCode, header};

use crate::{
    Error, Info, Manifest, Platform, Result, STATE_DIR, SignedManifest,
    manifest::{parse_public_key, public_key_hex},
};

/// A patch server, e.g. `http://192.168.5.69:8080`.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    /// `server` is the patch server's base URL; a bare `host[:port]` means `http://`.
    pub fn new(server: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("lmoxide-patch/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self::with_http(server, http))
    }

    /// Like [`Client::new`] with a preconfigured `reqwest::Client` (proxies, timeouts).
    pub fn with_http(server: &str, http: reqwest::Client) -> Self {
        let server = server.trim().trim_end_matches('/');
        let base = if server.contains("://") { server.to_string() } else { format!("http://{server}") };
        Self { http, base }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, rel: &str) -> String {
        format!("{}/lmoxide/v1/{rel}", self.base)
    }

    /// [`Error::Unreachable`] for connection failures and timeouts, else [`Error::Http`].
    fn send_error(&self, e: reqwest::Error) -> Error {
        if e.is_connect() || e.is_timeout() {
            Error::Unreachable { url: self.base.clone(), source: e }
        } else {
            Error::Http(e)
        }
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response> {
        self.check(url, self.http.get(url).send().await.map_err(|e| self.send_error(e))?)
    }

    /// Turns error statuses into errors.
    fn check(&self, url: &str, resp: reqwest::Response) -> Result<reqwest::Response> {
        match resp.status() {
            s if s.is_success() => Ok(resp),
            StatusCode::SERVICE_UNAVAILABLE => {
                let retry_after = resp
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5);
                Err(Error::NotReady { retry_after })
            }
            s => Err(Error::Status { status: s.as_u16(), url: url.to_string() }),
        }
    }

    pub async fn info(&self) -> Result<Info> {
        Ok(self.get(&self.url("info")).await?.json().await?)
    }

    /// The signed manifest, unverified (see [`Trust::verify`]). [`Error::NotReady`] while the
    /// server is still hashing its files.
    pub async fn signed_manifest(&self, platform: Platform) -> Result<SignedManifest> {
        Ok(self.get(&self.url(&format!("manifest/{platform}"))).await?.json().await?)
    }

    /// [`Client::signed_manifest`] unless its [`Manifest::revision`] is still `revision` (the
    /// installed one): then `None`, without downloading it (`If-None-Match`).
    pub async fn signed_manifest_if_changed(&self, platform: Platform, revision: &str) -> Result<Option<SignedManifest>> {
        let url = self.url(&format!("manifest/{platform}"));
        let resp = self
            .http
            .get(&url)
            .header(header::IF_NONE_MATCH, format!("\"{revision}\""))
            .send()
            .await
            .map_err(|e| self.send_error(e))?;
        if resp.status() == StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        Ok(Some(self.check(&url, resp)?.json().await?))
    }

    /// The patcher releases offered for `platform`, unverified (see
    /// [`crate::release::SignedRelease::verify`]); empty if the server offers none.
    pub async fn releases(&self, platform: Platform) -> Result<Vec<crate::release::SignedRelease>> {
        match self.get(&self.url(&format!("patchers/{platform}"))).await {
            Err(Error::Status { status: 404, .. }) => Ok(Vec::new()),
            r => Ok(r?.json().await?),
        }
    }

    /// Requests a file body, from byte `offset` on (HTTP Range). The response status tells
    /// whether the range was honoured (206) or the whole body follows (200).
    pub(crate) async fn file(&self, sha256: &str, offset: u64) -> Result<reqwest::Response> {
        let url = self.url(&format!("files/{sha256}"));
        let mut req = self.http.get(&url);
        if offset > 0 {
            req = req.header(header::RANGE, format!("bytes={offset}-"));
        }
        let resp = req.send().await.map_err(|e| self.send_error(e))?;
        if resp.status().is_success() {
            Ok(resp)
        } else {
            Err(Error::Status { status: resp.status().as_u16(), url })
        }
    }
}

/// Which manifest signing key to accept.
#[derive(Debug, Clone)]
pub enum Trust {
    /// Exactly this key (e.g. `--trust`, or a key shipped with a GUI build). Also stored in the
    /// install dir, replacing an earlier one.
    Key(VerifyingKey),
    /// Trust on first use: the key stored in the install dir, or, for a fresh install, whatever
    /// key signed the first manifest (then stored).
    FirstUse,
}

/// Where the trusted key of an install is kept.
pub fn trusted_key_path(install_dir: &Path) -> PathBuf {
    install_dir.join(STATE_DIR).join("server-key")
}

/// The key stored in an install, if any.
pub fn stored_key(install_dir: &Path) -> Result<Option<VerifyingKey>> {
    match std::fs::read_to_string(trusted_key_path(install_dir)) {
        Ok(s) => parse_public_key(&s).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl Trust {
    /// Verifies `signed` for `platform` and returns the manifest. On success the trusted key
    /// is stored in `install_dir`; [`Trust::FirstUse`] fails with [`Error::KeyMismatch`] if the
    /// server's key changed since (re-run with an explicit key to accept a new one).
    pub fn verify(&self, install_dir: &Path, signed: &SignedManifest, platform: Platform) -> Result<Manifest> {
        let stored = stored_key(install_dir)?;
        let key = match self {
            Trust::Key(k) => *k,
            Trust::FirstUse => match stored {
                Some(k) => k,
                None => signed.claimed_key()?,
            },
        };
        let manifest = signed.verify_for(&key, platform)?;
        if stored != Some(key) {
            let path = trusted_key_path(install_dir);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, public_key_hex(&key) + "\n")?;
        }
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Launch, MANIFEST_FORMAT, generate_signing_key};

    fn signed(key: &ed25519_dalek::SigningKey) -> SignedManifest {
        let m = Manifest {
            format: MANIFEST_FORMAT,
            server: "t".into(),
            platform: Platform::Linux,
            version: Manifest::version_of(&[]),
            generated_at: 0,
            login_public_key: None,
            universe_addr: "h:1".into(),
            files: vec![],
            remove: vec![],
            launch: Launch::default(),
            modes: Vec::new(),
        };
        SignedManifest::sign(&m, key).unwrap()
    }

    #[test]
    fn trust_on_first_use() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (generate_signing_key(), generate_signing_key());
        assert!(stored_key(dir.path()).unwrap().is_none());
        Trust::FirstUse.verify(dir.path(), &signed(&a), Platform::Linux).unwrap();
        assert_eq!(stored_key(dir.path()).unwrap(), Some(a.verifying_key()));
        // same key again: fine; a new key: refused
        Trust::FirstUse.verify(dir.path(), &signed(&a), Platform::Linux).unwrap();
        assert!(matches!(
            Trust::FirstUse.verify(dir.path(), &signed(&b), Platform::Linux),
            Err(Error::KeyMismatch { .. })
        ));
        // an explicit key replaces the stored one
        Trust::Key(b.verifying_key()).verify(dir.path(), &signed(&b), Platform::Linux).unwrap();
        assert_eq!(stored_key(dir.path()).unwrap(), Some(b.verifying_key()));
        assert!(Trust::Key(a.verifying_key()).verify(dir.path(), &signed(&b), Platform::Linux).is_err());
    }

    #[test]
    fn base_urls() {
        assert_eq!(Client::new("10.0.0.1:8080").unwrap().base_url(), "http://10.0.0.1:8080");
        assert_eq!(Client::new("https://lmo.example/").unwrap().base_url(), "https://lmo.example");
    }
}
