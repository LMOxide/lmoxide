//! Shared state and (re)building it: scanning the client dirs, patching the executables,
//! hashing, signing the manifests.
//!
//! A build runs in two phases. [`prepare`] is quick (directory walks, patching two
//! executables, the RDB index) and makes the stock client endpoints current; [`publish`] hashes
//! everything (slow the first time) and then makes the manifests available, so the stock
//! endpoints work while a fresh data dir is still indexing. [`watch`] rebuilds when the game
//! server's login key file changes, or on SIGHUP (after updating a client dir).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::Context;
use bytes::Bytes;
use ed25519_dalek::SigningKey;
use lmoxide_net::login::FUNCOM_SESSION_KEY;
use lmoxide_patch::{Platform, SignedManifest, hashcache::HashCache, manifest, release};
use tracing::{info, warn};

use crate::{
    Config,
    install::{self, ScanOptions, Source, Tree},
    patching,
    stock::{self, StockState, Upstream},
};

pub struct App {
    pub cfg: Arc<Config>,
    pub signing: SigningKey,
    pub upstream: Option<Upstream>,
    stock: RwLock<Arc<StockState>>,
    catalog: RwLock<Arc<Catalog>>,
}

/// What the `/lmoxide/v1/` routes serve.
#[derive(Default)]
pub struct Catalog {
    /// The login key patched into the executables of the current build.
    pub login_key: Option<String>,
    pub warnings: Vec<String>,
    /// Signed manifest per platform; missing while still being built.
    pub manifests: HashMap<Platform, Published>,
    /// sha256 -> file, for every file of every manifest and every patcher release.
    pub files: HashMap<String, PathBuf>,
    /// Signed patcher releases (JSON list) per platform, from `--patcher-dir`.
    pub releases: HashMap<Platform, Bytes>,
    /// Patcher name → platform → version, for `/lmoxide/v1/info`.
    pub patchers: BTreeMap<String, BTreeMap<Platform, String>>,
}

/// A platform's signed manifest as served.
#[derive(Clone)]
pub struct Published {
    /// The [`SignedManifest`] JSON.
    pub json: Bytes,
    /// [`lmoxide_patch::Manifest::revision`].
    pub revision: String,
    /// Size of the complete install.
    pub install_bytes: u64,
}

/// Output of [`prepare`], input of [`publish`].
pub struct Prepared {
    login_key: Option<String>,
    warnings: Vec<String>,
    trees: Vec<(Platform, Tree)>,
}

impl App {
    pub fn new(cfg: Arc<Config>, signing: SigningKey, upstream: Option<Upstream>) -> Self {
        let stock = Arc::new(stock::empty(&cfg));
        Self { cfg, signing, upstream, stock: RwLock::new(stock), catalog: RwLock::default() }
    }

    pub fn stock(&self) -> Arc<StockState> {
        self.stock.read().unwrap().clone()
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog.read().unwrap().clone()
    }

    /// Platforms with a client dir.
    pub fn platforms(&self) -> Vec<Platform> {
        Platform::ALL.into_iter().filter(|p| self.cfg.client_dir_for(*p).is_some()).collect()
    }
}

/// Loads the manifest signing key from the data dir, or creates it (mode 0600).
pub fn load_signing_key(data_dir: &Path) -> anyhow::Result<SigningKey> {
    let path = data_dir.join("manifest-signing.key");
    match fs::read_to_string(&path) {
        Ok(s) => return manifest::parse_signing_key(&s).with_context(|| format!("{}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    }
    fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
    let key = manifest::generate_signing_key();
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    use std::io::Write;
    let mut f = opts.open(&path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all((manifest::signing_key_hex(&key) + "\n").as_bytes())?;
    f.sync_all()?;
    info!(path = %path.display(), "generated a new manifest signing key");
    Ok(key)
}

/// Reads the game server's login public key: hex, whitespace-trimmed, zero-padded to 512
/// characters. `Ok(None)` if no file is configured or it doesn't exist (yet).
pub fn read_login_key(path: Option<&Path>) -> Result<Option<String>, String> {
    let Some(path) = path else { return Ok(None) };
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let key = text.trim().to_ascii_lowercase();
    let width = FUNCOM_SESSION_KEY.len();
    if key.is_empty() || key.len() > width || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{}: not a login public key ({width} hex characters)", path.display()));
    }
    Ok(Some(format!("{key:0>width$}")))
}

fn blobs_dir(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("blobs")
}

/// Stores generated content as `blobs/<sha256>` and returns its source.
fn write_blob(cfg: &Config, data: &[u8], executable: bool) -> io::Result<Source> {
    let sha = lmoxide_patch::sha256_hex(data);
    let dir = blobs_dir(cfg);
    fs::create_dir_all(&dir)?;
    let path = dir.join(&sha);
    if fs::metadata(&path).map(|m| m.len()).ok() != Some(data.len() as u64) {
        let tmp = dir.join(format!(".{sha}.tmp"));
        fs::write(&tmp, data)?;
        fs::rename(&tmp, &path)?;
    }
    Ok(Source { path, executable, sha256: Some(sha) })
}

fn executable_name(p: Platform) -> &'static str {
    match p {
        Platform::Windows => "LMO.exe",
        Platform::Linux => "LMO",
    }
}

/// Patches the platform's executable in `tree` (login key and chat filter URL), replacing its
/// source with the patched blob.
fn patch_executable(
    cfg: &Config,
    platform: Platform,
    tree: &mut Tree,
    login_key: Option<&str>,
    warnings: &mut Vec<String>,
) -> io::Result<()> {
    let name = executable_name(platform);
    let Some((rel, src)) = tree.get_ci(name).map(|(k, v)| (k.clone(), v.clone())) else {
        warnings.push(format!("{platform}: no {name} in the client dir; nothing patched"));
        return Ok(());
    };
    let original = fs::read(&src.path)?;
    let mut data = original.clone();
    match patching::patch_chat_filter(&mut data, &cfg.chat_filter_host()) {
        Ok(at) => info!(%platform, at = format_args!("{at:#x}"), host = cfg.chat_filter_host(), "patched chat filter URL"),
        Err(e) => warnings.push(format!("{platform}: chat filter not patched: {e} (see --chat-filter-host)")),
    }
    match patching::patch_tells(&mut data) {
        Ok(at) => info!(%platform, at = format_args!("{at:#x}"), "patched DoTell (private messages)"),
        Err(e) => warnings.push(format!("{platform}: private messages not enabled: {e}")),
    }
    if let Some(key) = login_key {
        match patching::patch_login_key(&mut data, key) {
            Ok(at) => info!(%platform, at = format_args!("{at:#x}"), "patched login public key"),
            Err(e) => warnings.push(format!("{platform}: login key not patched: {e}")),
        }
    }
    if data != original {
        let blob = write_blob(cfg, &data, src.executable)?;
        tree.files.insert(rel, blob);
    }
    Ok(())
}

/// The chat window for everyone, not only GMs (see `patching::patch_chat_gui`).
fn patch_modules_xml(cfg: &Config, platform: Platform, tree: &mut Tree, warnings: &mut Vec<String>) -> io::Result<()> {
    const MODULES: &str = "Data/Gui/Default/Modules.xml";
    let Some((rel, src)) = tree.get_ci(MODULES).map(|(k, v)| (k.clone(), v.clone())) else {
        warnings.push(format!("{platform}: no {MODULES}; chat stays GM-only"));
        return Ok(());
    };
    let original = fs::read_to_string(&src.path)?;
    match patching::patch_chat_gui(&original) {
        Ok(xml) if xml != original => {
            info!(%platform, "patched the chat window for everyone");
            let blob = write_blob(cfg, xml.as_bytes(), false)?;
            tree.files.insert(rel, blob);
        }
        Ok(_) => {}
        Err(e) => warnings.push(format!("{platform}: chat stays GM-only: {e}")),
    }
    Ok(())
}

/// Phase 1: scan, patch, rebuild the stock endpoints.
pub async fn prepare(app: &Arc<App>) -> anyhow::Result<Prepared> {
    let app2 = app.clone();
    let prepared = tokio::task::spawn_blocking(move || prepare_blocking(&app2)).await??;
    // Keep serving the previous manifests until the new ones are signed.
    let old = app.catalog();
    let catalog = Catalog {
        login_key: prepared.login_key.clone(),
        warnings: prepared.warnings.clone(),
        manifests: old.manifests.clone(),
        files: old.files.clone(),
        releases: old.releases.clone(),
        patchers: old.patchers.clone(),
    };
    *app.catalog.write().unwrap() = Arc::new(catalog);
    Ok(prepared)
}

fn prepare_blocking(app: &App) -> anyhow::Result<Prepared> {
    let cfg = &app.cfg;
    let mut warnings = Vec::new();
    let login_key = match read_login_key(cfg.login_public_key_file.as_deref()) {
        Ok(Some(k)) => Some(k),
        Ok(None) => {
            warnings.push(match &cfg.login_public_key_file {
                Some(p) => format!("login public key file {} not found: serving unpatched login key", p.display()),
                None => "no --login-public-key-file: serving unpatched login key (clients can't log in to an LMOxide game server)".into(),
            });
            None
        }
        Err(e) => {
            warnings.push(format!("{e}: serving unpatched login key"));
            None
        }
    };
    let exclude = install::build_globset(&cfg.exclude)?;
    let mut trees = Vec::new();
    for platform in app.platforms() {
        let dir = cfg.client_dir_for(platform).unwrap();
        let mut tree = install::scan(&ScanOptions {
            platform,
            client_dir: dir,
            overlay_dir: cfg.overlay_dir_for(platform),
            exclude: &exclude,
            remove: cfg.remove_for(platform),
            use_orig: !cfg.no_orig,
        })
        .with_context(|| format!("scanning {}", dir.display()))?;
        patch_executable(cfg, platform, &mut tree, login_key.as_deref(), &mut warnings)?;
        patch_modules_xml(cfg, platform, &mut tree, &mut warnings)?;
        info!(%platform, files = tree.files.len(), remove = tree.remove.len(), "install scanned");
        trees.push((platform, tree));
    }

    let windows = trees.iter().find(|(p, _)| *p == Platform::Windows).map(|(_, t)| t);
    let stock = Arc::new(stock::build(cfg, windows));
    let local_config = write_blob(cfg, stock.local_config.as_bytes(), false)?;
    for (platform, tree) in &mut trees {
        tree.files.insert("LocalConfig.xml".into(), local_config.clone());
        // The Linux kit's `lmo` launcher reads its server from lmo-kit.conf.
        if *platform == Platform::Linux && tree.files.contains_key("lmo") {
            let conf = format!(
                "# written by lmoxide-patch-server\nLMO_KIT_VERSION=lmoxide\nLMO_SERVER={}\n",
                cfg.public_host
            );
            tree.files.insert("lmo-kit.conf".into(), write_blob(cfg, conf.as_bytes(), false)?);
        }
    }
    *app.stock.write().unwrap() = stock;
    for w in &warnings {
        warn!("{w}");
    }
    Ok(Prepared { login_key, warnings, trees })
}

/// Phase 2: hash, sign and publish the manifests.
pub async fn publish(app: &Arc<App>, prepared: Prepared) -> anyhow::Result<()> {
    let app2 = app.clone();
    let catalog = tokio::task::spawn_blocking(move || publish_blocking(&app2, prepared)).await??;
    let platforms: Vec<_> = catalog.manifests.keys().copied().collect();
    *app.catalog.write().unwrap() = Arc::new(catalog);
    info!(?platforms, "manifests ready");
    cleanup_blobs(&app.cfg, &app.catalog());
    Ok(())
}

fn publish_blocking(app: &App, mut prepared: Prepared) -> anyhow::Result<Catalog> {
    let cfg = &app.cfg;
    let cache_path = cfg.data_dir.join("hash-cache.json");
    let mut cache = HashCache::load(&cache_path);
    let started = std::time::Instant::now();
    {
        let mut trees: Vec<&mut Tree> = prepared.trees.iter_mut().map(|(_, t)| t).collect();
        install::hash_trees(&mut trees, &mut cache, cfg.hash_jobs)?;
    }
    let live: HashSet<String> = prepared
        .trees
        .iter()
        .flat_map(|(_, t)| t.files.values().map(|s| install::cache_key(&s.path)))
        .collect();
    cache.retain(|k| live.contains(k));
    if let Err(e) = cache.save(&cache_path) {
        warn!("saving {}: {e}", cache_path.display());
    }
    info!(secs = started.elapsed().as_secs_f32(), cached = cache.len(), "client files hashed");

    let mut catalog = Catalog { login_key: prepared.login_key.clone(), warnings: prepared.warnings, ..Default::default() };
    for (platform, tree) in &prepared.trees {
        let m = install::manifest(*platform, tree, &cfg.server_name, &cfg.universe_addr(), prepared.login_key.as_deref())?;
        let bytes: u64 = m.files.iter().map(|f| f.size).sum();
        info!(%platform, files = m.files.len(), mb = bytes >> 20, version = %m.version, "manifest");
        let signed = SignedManifest::sign(&m, &app.signing)?;
        let json = Bytes::from(serde_json::to_vec(&signed)?);
        catalog.manifests.insert(*platform, Published { json, revision: m.revision(), install_bytes: bytes });
        for src in tree.files.values() {
            catalog.files.insert(src.sha256.clone().unwrap(), src.path.clone());
        }
    }
    if let Some(dir) = &cfg.patcher_dir {
        publish_releases(app, dir, &mut catalog).with_context(|| format!("--patcher-dir {}", dir.display()))?;
    }
    Ok(catalog)
}

/// Signs the newest build of each patcher per platform in `dir` (files named
/// `<name>-<version>-<platform>[.exe]`, see `lmoxide_patch::release`).
fn publish_releases(app: &App, dir: &Path, catalog: &mut Catalog) -> anyhow::Result<()> {
    let mut newest: BTreeMap<(String, Platform), (String, PathBuf)> = BTreeMap::new();
    for e in fs::read_dir(dir)?.filter_map(Result::ok) {
        let path = e.path();
        let Some((name, version, platform)) = e.file_name().to_str().and_then(release::parse_file_name) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        let slot = newest.entry((name, platform)).or_insert_with(|| (version.clone(), path.clone()));
        if release::compare_versions(&version, &slot.0).is_gt() {
            *slot = (version, path);
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let cache_path = app.cfg.data_dir.join("patcher-hash-cache.json");
    let mut cache = HashCache::load(&cache_path);
    cache.retain(|k| newest.values().any(|(_, p)| p.to_string_lossy() == k));
    let mut lists: BTreeMap<Platform, Vec<release::SignedRelease>> = BTreeMap::new();
    for ((name, platform), (version, path)) in newest {
        let sha256 = cache.hash(&path.to_string_lossy(), &path)?;
        let size = fs::metadata(&path)?.len();
        info!(%name, %version, %platform, "patcher release");
        let r = release::Release { format: release::RELEASE_FORMAT, name: name.clone(), version: version.clone(), platform, size, sha256: sha256.clone(), generated_at: now };
        lists.entry(platform).or_default().push(release::SignedRelease::sign(&r, &app.signing)?);
        catalog.patchers.entry(name).or_default().insert(platform, version);
        catalog.files.insert(sha256, path);
    }
    if let Err(e) = cache.save(&cache_path) {
        warn!("saving {}: {e}", cache_path.display());
    }
    for (platform, list) in lists {
        catalog.releases.insert(platform, Bytes::from(serde_json::to_vec(&list)?));
    }
    Ok(())
}

/// Deletes blobs no manifest refers to any more (old patched executables).
fn cleanup_blobs(cfg: &Config, catalog: &Catalog) {
    let used: HashSet<&Path> = catalog.files.values().map(PathBuf::as_path).collect();
    let Ok(entries) = fs::read_dir(blobs_dir(cfg)) else { return };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if !used.contains(p.as_path()) {
            let _ = fs::remove_file(&p);
        }
    }
}

/// Rebuilds when the login key file changes (polled) or on SIGHUP.
pub async fn watch(app: Arc<App>) {
    let mut tick = tokio::time::interval(Duration::from_secs(app.cfg.key_poll_secs.max(1)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    #[cfg(unix)]
    let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()).ok();
    loop {
        #[cfg(unix)]
        let hangup = async {
            match hup.as_mut() {
                Some(h) => h.recv().await,
                None => std::future::pending().await,
            }
        };
        #[cfg(not(unix))]
        let hangup = std::future::pending::<Option<()>>();
        let reason = tokio::select! {
            _ = tick.tick() => {
                let current = app.catalog().login_key.clone();
                match read_login_key(app.cfg.login_public_key_file.as_deref()) {
                    Ok(k) if k != current => "login key changed",
                    _ => continue,
                }
            }
            _ = hangup => "SIGHUP",
        };
        info!("{reason}: rebuilding");
        let result = async { publish(&app, prepare(&app).await?).await }.await;
        if let Err(e) = result {
            warn!("rebuild failed: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("login.pub");
        assert_eq!(read_login_key(None), Ok(None));
        assert_eq!(read_login_key(Some(&p)), Ok(None));
        fs::write(&p, format!("  {}\n", "AB".repeat(256))).unwrap();
        assert_eq!(read_login_key(Some(&p)), Ok(Some("ab".repeat(256))));
        fs::write(&p, "abc\n").unwrap();
        assert_eq!(read_login_key(Some(&p)).unwrap().unwrap(), format!("{:0>512}", "abc"));
        fs::write(&p, "xyz").unwrap();
        assert!(read_login_key(Some(&p)).is_err());
        fs::write(&p, "a".repeat(513)).unwrap();
        assert!(read_login_key(Some(&p)).is_err());
    }

    #[test]
    fn signing_key_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_signing_key(&dir.path().join("data")).unwrap();
        let b = load_signing_key(&dir.path().join("data")).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join("data/manifest-signing.key")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
