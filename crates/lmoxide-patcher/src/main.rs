//! `lmoxide-patcher`: installs, updates and verifies a client from an LMOxide patch server
//! (the command-line face of `lmoxide_patch`; see `docs/PATCHING.md`).
//!
//! ```text
//! lmoxide-patcher install --server http://192.168.5.69:8080 --dir ~/Games/LMO
//! lmoxide-patcher verify  --server http://192.168.5.69:8080 --dir ~/Games/LMO
//! lmoxide-patcher info    --server http://192.168.5.69:8080
//! lmoxide-patcher self-update --server http://192.168.5.69:8080
//! ```
//!
//! The first install trusts the key that signed the manifest and remembers it in
//! `<dir>/.lmoxide/server-key`; later runs refuse a manifest signed by another key unless
//! `--trust <key>` names it (the server logs its key at startup and shows it in `info`).

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use lmoxide_patch::{
    Error, Manifest, Platform, SignedManifest,
    client::{Client, Trust},
    manifest::parse_public_key,
    selfupdate,
    sync::{self, Options, Plan, Progress, ProgressFn, Reason},
};

#[derive(Parser)]
#[command(version, about = "Install and update LEGO Minifigures Online from an LMOxide server")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install or update the client (download what differs, remove what the server removes).
    #[command(alias = "update", alias = "sync")]
    Install(SyncArgs),
    /// Check an install against the server's manifest without changing anything (exit code 1
    /// if it isn't up to date).
    Verify {
        #[command(flatten)]
        args: SyncArgs,
        /// Rehash every file instead of trusting unchanged size and modification time.
        #[arg(long)]
        full: bool,
    },
    /// Replace this program with the newest build the server offers, if it is newer. The build
    /// must be signed with the key the install in --dir trusts (or --trust).
    SelfUpdate {
        /// Patch server URL, e.g. http://192.168.5.69:8080.
        #[arg(long, env = "LMOXIDE_SERVER")]
        server: String,
        /// Install whose trusted key the build must be signed with (default ~/Games/LMO).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Signing key to trust instead (hex).
        #[arg(long)]
        trust: Option<String>,
    },
    /// Show what the server offers.
    Info {
        /// Patch server URL, e.g. http://192.168.5.69:8080.
        #[arg(long, env = "LMOXIDE_SERVER")]
        server: String,
    },
}

#[derive(Args)]
struct SyncArgs {
    /// Patch server URL, e.g. http://192.168.5.69:8080.
    #[arg(long, env = "LMOXIDE_SERVER")]
    server: String,
    /// Client build (default: this OS's).
    #[arg(long, default_value_t = Platform::host())]
    platform: Platform,
    /// Install directory (default ~/Games/LMO).
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Manifest signing key to trust (hex); default: the key remembered from the first install.
    #[arg(long)]
    trust: Option<String>,
    /// Parallel downloads.
    #[arg(long, short, default_value_t = 4)]
    jobs: usize,
}

impl SyncArgs {
    fn dir(&self) -> anyhow::Result<PathBuf> {
        match &self.dir {
            Some(d) => Ok(d.clone()),
            None => lmoxide_patch::default_install_dir().context("no home directory; pass --dir"),
        }
    }

    fn trust(&self) -> anyhow::Result<Trust> {
        Ok(match &self.trust {
            Some(k) => Trust::Key(parse_public_key(k)?),
            None => Trust::FirstUse,
        })
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    if let Ok(exe) = std::env::current_exe() {
        selfupdate::cleanup(&exe);
    }
    match cli.cmd {
        Cmd::Info { server } => {
            let info = Client::new(&server)?.info().await?;
            println!("server:       {}", info.name);
            println!("game:         {}", info.universe_addr);
            println!("http:         {}", info.http_addr);
            if let Some(kind) = &info.kind {
                println!("kind:         {kind}");
            }
            for other in &info.other_servers {
                println!("also here:    {other}");
            }
            if let Some(https) = &info.https_addr {
                println!("https:        {https}");
            }
            let platforms: Vec<String> = info
                .platforms
                .iter()
                .map(|p| if info.ready.contains(p) { p.to_string() } else { format!("{p} (indexing)") })
                .collect();
            println!("platforms:    {}", platforms.join(", "));
            println!("manifest key: {}", info.manifest_key);
            match &info.login_public_key {
                Some(k) => println!("login key:    {}…{}", &k[..16], &k[k.len() - 16..]),
                None => println!("login key:    none (binaries unpatched)"),
            }
            for (p, r) in &info.revisions {
                let size = info.install_bytes.get(p).map(|b| format!(" ({})", indicatif::HumanBytes(*b))).unwrap_or_default();
                println!("{:<13} {}{size}", format!("{p}:"), &r[..r.len().min(16)]);
            }
            if let Some(api) = &info.api_url {
                println!("api:          {api}");
            }
            if let Some(site) = &info.website {
                println!("website:      {site}");
            }
            for (name, builds) in &info.patchers {
                let builds: Vec<String> = builds.iter().map(|(p, v)| format!("{v} ({p})")).collect();
                println!("{:<13} {}", format!("{name}:"), builds.join(", "));
            }
            for w in &info.warnings {
                println!("warning:      {w}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::SelfUpdate { server, dir, trust } => {
            let key = match trust {
                Some(k) => parse_public_key(&k)?,
                None => {
                    let dir = match dir {
                        Some(d) => d,
                        None => lmoxide_patch::default_install_dir().context("no home directory; pass --dir")?,
                    };
                    lmoxide_patch::client::stored_key(&dir)?.with_context(|| {
                        format!("{} has no trusted server key: install from this server first, or pass --trust", dir.display())
                    })?
                }
            };
            let client = Client::new(&server)?;
            let current = env!("CARGO_PKG_VERSION");
            let Some(release) = selfupdate::check(&client, "lmoxide-patcher", Platform::host(), current, &key).await? else {
                println!("lmoxide-patcher {current} is the newest the server offers");
                return Ok(ExitCode::SUCCESS);
            };
            let exe = std::env::current_exe()?;
            println!("updating lmoxide-patcher {current} → {} ({})", release.version, indicatif::HumanBytes(release.size));
            selfupdate::install(&client, &release, &exe, &sync::no_progress()).await?;
            println!("done: {} is now {}", exe.display(), release.version);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Install(args) => {
            let dir = args.dir()?;
            let fresh = !dir.join(lmoxide_patch::STATE_DIR).exists();
            let (client, signed, manifest) = fetch(&args, &dir).await?;
            println!(
                "{} {} client from {} into {}",
                if fresh { "installing" } else { "updating" },
                manifest.platform,
                manifest.server,
                dir.display()
            );
            let opts = Options { jobs: args.jobs.max(1), ..Default::default() };
            let bars = Bars::default();
            let (plan, report) = sync::sync(&client, &dir, &manifest, &opts, &bars.progress()).await?;
            bars.finish();
            sync::save_manifest(&dir, &signed)?;
            if plan.is_up_to_date() {
                println!("up to date ({} files)", report.unchanged);
            } else {
                println!(
                    "done: {} files downloaded ({}), {} removed, {} unchanged",
                    report.downloaded_files,
                    indicatif::HumanBytes(report.downloaded_bytes),
                    report.removed_files,
                    report.unchanged
                );
            }
            print_launch(&dir, &manifest);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Verify { args, full } => {
            let dir = args.dir()?;
            let (_, _, manifest) = fetch(&args, &dir).await?;
            let opts = Options { jobs: args.jobs.max(1), full_check: full, ..Default::default() };
            let bars = Bars::default();
            let plan = sync::plan(&dir, &manifest, &opts, &bars.progress()).await?;
            bars.finish();
            print_plan(&plan);
            Ok(if plan.is_up_to_date() { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
    }
}

/// Fetches and verifies the manifest, waiting while the server is still indexing.
async fn fetch(args: &SyncArgs, dir: &Path) -> anyhow::Result<(Client, SignedManifest, Manifest)> {
    let client = Client::new(&args.server)?;
    let trust = args.trust()?;
    let mut waited = false;
    let signed = loop {
        match client.signed_manifest(args.platform).await {
            Err(Error::NotReady { retry_after }) => {
                if !waited {
                    eprintln!("the server is still indexing its client files; waiting…");
                    waited = true;
                }
                tokio::time::sleep(Duration::from_secs(retry_after.clamp(1, 60))).await;
            }
            Err(Error::Status { status: 404, .. }) => {
                anyhow::bail!("{} doesn't offer a {} client", client.base_url(), args.platform)
            }
            r => break r?,
        }
    };
    let first_use = matches!(trust, Trust::FirstUse) && lmoxide_patch::client::stored_key(dir)?.is_none();
    let manifest = match trust.verify(dir, &signed, args.platform) {
        Err(Error::KeyMismatch { expected, got }) => anyhow::bail!(
            "the manifest is not signed by the trusted key!\n  trusted: {expected}\n  server:  {got}\n\
             If the server operator really changed the key, re-run with --trust {got}"
        ),
        r => r?,
    };
    if first_use {
        eprintln!("trusting server key {} (stored in {})", signed.key, lmoxide_patch::client::trusted_key_path(dir).display());
    }
    if manifest.login_public_key.is_none() {
        eprintln!("warning: the server has no login key; the client is not patched for its login");
    }
    Ok((client, signed, manifest))
}

fn print_plan(plan: &Plan) {
    for c in &plan.download {
        let what = match c.reason {
            Reason::Missing => "missing",
            Reason::Changed => "changed",
        };
        println!("{what:>8}  {}", c.entry.path);
    }
    for e in &plan.fix_mode {
        println!("{:>8}  {}", "mode", e.path);
    }
    for r in &plan.remove {
        println!("{:>8}  {r}", "remove");
    }
    if plan.is_up_to_date() {
        println!("up to date ({} files)", plan.unchanged);
    } else {
        println!(
            "not up to date: {} files to download ({}), {} to fix, {} to remove; {} unchanged",
            plan.download.len(),
            indicatif::HumanBytes(plan.download_bytes()),
            plan.fix_mode.len(),
            plan.remove.len(),
            plan.unchanged
        );
    }
}

fn print_launch(dir: &Path, m: &Manifest) {
    if let Ok(cmd) = m.launch.command(dir, None, &[]) {
        println!("start: {cmd:?}");
    }
}

/// indicatif bars driven by the sync engine's progress events.
#[derive(Default, Clone)]
struct Bars(Arc<Mutex<Option<ProgressBar>>>);

impl Bars {
    fn progress(&self) -> ProgressFn {
        let bars = self.clone();
        Arc::new(move |p| bars.on(p))
    }

    fn on(&self, p: Progress) {
        let mut cur = self.0.lock().unwrap();
        match p {
            Progress::Checking { done, total } => {
                let bar = cur.get_or_insert_with(|| {
                    let b = ProgressBar::new(total as u64);
                    b.set_style(
                        ProgressStyle::with_template("checking    [{bar:30}] {pos}/{len} files {eta}")
                            .unwrap()
                            .progress_chars("=> "),
                    );
                    b
                });
                bar.set_position(done as u64);
            }
            Progress::Planned { download_files, download_bytes, .. } => {
                if let Some(b) = cur.take() {
                    b.finish_and_clear();
                }
                if download_files > 0 {
                    let b = ProgressBar::new(download_bytes);
                    b.set_style(
                        ProgressStyle::with_template(
                            "downloading [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec} {eta} {msg}",
                        )
                        .unwrap()
                        .progress_chars("=> "),
                    );
                    *cur = Some(b);
                }
            }
            Progress::FileStarted { path, .. } => {
                if let Some(b) = cur.as_ref() {
                    b.set_message(path);
                }
            }
            Progress::Bytes(n) => {
                if let Some(b) = cur.as_ref() {
                    b.inc(n);
                }
            }
            Progress::Removed { path } => match cur.as_ref() {
                Some(b) => b.println(format!("removed {path}")),
                None => println!("removed {path}"),
            },
            Progress::FileDone { .. } => {}
            Progress::Done => {
                if let Some(b) = cur.take() {
                    b.finish_and_clear();
                }
            }
        }
    }

    fn finish(&self) {
        if let Some(b) = self.0.lock().unwrap().take() {
            b.finish_and_clear();
        }
    }
}
