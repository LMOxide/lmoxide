//! How the world editor shows an object by where it came from (docs/SURVEY.md "Content
//! categories").

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Live,
    Demo,
    Client,
    Guessed,
    Surveyed,
}

impl Category {
    /// From `world_content.json`'s `source` (`proposed`, `live`, `demo`, `rdb_*`, `cine_point`).
    pub fn from_source(source: Option<&str>) -> Category {
        match source {
            Some("proposed") => Category::Guessed,
            Some("live") => Category::Live,
            Some("demo") => Category::Demo,
            _ => Category::Client,
        }
    }

    /// Only guesses can be confirmed, edited or deleted; everything else can only be hidden.
    pub fn editable(self) -> bool {
        matches!(self, Category::Guessed | Category::Surveyed)
    }
}
