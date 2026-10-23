//! Saved characters: one JSON file per account in `--data-dir`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub name: String,
    pub playfield: u32,
    pub pos: [f32; 3],
    /// Minifigure team: (inventory 101 position, item template). The first entry is active.
    pub team: Vec<(u32, u32)>,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, account: u32) -> PathBuf {
        self.dir.join(format!("characters/{account}.json"))
    }

    pub fn load(&self, account: u32) -> Option<Character> {
        let path = self.path(account);
        let text = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&text)
            .inspect_err(|e| warn!(path = %path.display(), "unreadable character file: {e}"))
            .ok()
    }

    pub fn save(&self, account: u32, character: &Character) -> anyhow::Result<()> {
        let path = self.path(account);
        write_atomic(&path, serde_json::to_string_pretty(character)?.as_bytes())
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
