//! The game's content (`content/`, crate lmoxide-content, docs/CONTENT.md): loaded once at start.
//! Systems moved to it so far: loot.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use lmoxide_content::{Content, Options, Severity};
use tracing::{info, warn};

static CONTENT: OnceLock<Content> = OnceLock::new();

/// The tree in this source checkout (for builds run from it and for tests).
fn built_in_dir() -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content")).to_path_buf()
}

/// The content folder: `dir`, or the checkout's `content/`.
pub fn dir(dir: Option<PathBuf>) -> PathBuf {
    dir.unwrap_or_else(built_in_dir)
}

/// Loads the content from `dir` (default: the checkout's `content/`). Errors stop the server:
/// a template with a broken reference or a misspelled field would fail later, in play.
pub fn init(dir: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = dir.unwrap_or_else(built_in_dir);
    let content = load(&dir)?;
    info!(dir = %dir.display(), worlds = content.worlds.len(), loot_tables = content.loot.len(), items = content.items.len(), "content loaded");
    let _ = CONTENT.set(content);
    Ok(())
}

fn load(dir: &Path) -> anyhow::Result<Content> {
    // client ids are the editors' and CI's check (`lmoxide-content check`)
    let options = Options {
        check_client_ids: false,
        ..Default::default()
    };
    let (content, problems) = lmoxide_content::load(dir, &options);
    let errors: Vec<String> = problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .map(|p| p.to_string())
        .collect();
    for p in problems.iter().filter(|p| p.severity == Severity::Warning) {
        warn!("content: {p}");
    }
    anyhow::ensure!(
        errors.is_empty(),
        "content in {} has {} errors (lmoxide-content check lists them):\n{}",
        dir.display(),
        errors.len(),
        errors
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(content)
}

/// The content. Loaded by [`init`] at start; tests (and anything before `init`) get the
/// checkout's tree.
pub fn get() -> &'static Content {
    CONTENT.get_or_init(|| load(&built_in_dir()).expect("content"))
}
