//! The game's content: every template the server runs on, in one editable tree of JSON files
//! (docs/CONTENT.md).
//!
//! ```text
//! content/
//!   content.json  tunables.json  texts.json
//!   worlds/<playfield>-<slug>/world.json, spawns.json, portals.json, bouncers.json,
//!                             areas.json, triggers.json, puzzles.json, scripts/<slug>.json
//!   regions/ npcs/ factions/ objects/ skills/ buffs/ loot/ items/ minifigures/
//!   dialogues/ achievements/     (one record per file)
//! ```
//!
//! A file holds one record (it has an `id`) or sections of records (`{ "spawns": [...] }`).
//! The kind of a record is the first part of its id (`npc/pirate-specter`). [`load`] reads
//! the tree, applies the active features and `extends`, checks every reference and returns the
//! typed [`Content`] with every problem found.

pub mod action;
pub mod canon;
pub mod catalog;
pub mod compat;
pub mod git;
pub mod import;
pub mod load;
pub mod merge;
pub mod model;
pub mod record;
pub mod schema;
pub mod tunables;
pub mod units;
pub mod work;

pub use action::{Action, Destination, SpawnAt, Target};
pub use load::{Content, Options, Problem, Severity, load};
pub use model::*;
pub use record::{GameAction, Point, Record, Ref, Rot, Source, Vec3};
pub use tunables::{Manifest, Texts, Tunables};
pub use units::{Duration, Range, Ratio};

/// The format version `content.json` names.
pub const FORMAT: u32 = 1;

/// Every kind of record: its id prefix, its folder (definitions) or the world file its records
/// go in.
pub const KINDS: &[(&str, &str)] = &[
    ("world", "worlds/<world>/world.json"),
    ("spawn", "worlds/<world>/spawns.json"),
    ("portal", "worlds/<world>/portals.json"),
    ("bouncer", "worlds/<world>/bouncers.json"),
    ("area", "worlds/<world>/areas.json"),
    ("trigger", "worlds/<world>/triggers.json"),
    ("puzzle", "worlds/<world>/puzzles.json"),
    ("script", "worlds/<world>/scripts/"),
    ("region", "regions/"),
    ("npc", "npcs/"),
    ("faction", "factions/"),
    ("object", "objects/"),
    ("skill", "skills/"),
    ("buff", "buffs/"),
    ("loot", "loot/"),
    ("item", "items/"),
    ("minifig", "minifigures/"),
    ("dialogue", "dialogues/"),
    ("achievement", "achievements/"),
];

/// The kind of an id (`npc/pirate-specter` -> `npc`), if it's one.
pub fn kind_of(id: &str) -> Option<&'static str> {
    let prefix = id.split_once('/')?.0;
    KINDS.iter().map(|(k, _)| *k).find(|k| *k == prefix)
}

/// A readable id part from a name: lower case, `-` between words ("XX_Snake Shot" ->
/// "xx-snake-shot").
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "unnamed".into()
    } else {
        out
    }
}

/// One JSON Schema per kind and file, for the world editor's forms and editors' validation.
pub fn schemas() -> Vec<(&'static str, serde_json::Value)> {
    macro_rules! schema {
        ($t:ty) => {
            serde_json::to_value(schemars::schema_for!($t)).expect("schema")
        };
    }
    vec![
        ("world", schema!(World)),
        ("spawn", schema!(Spawn)),
        ("portal", schema!(Portal)),
        ("bouncer", schema!(Bouncer)),
        ("area", schema!(Area)),
        ("trigger", schema!(Trigger)),
        ("puzzle", schema!(Puzzle)),
        ("script", schema!(Script)),
        ("region", schema!(Region)),
        ("npc", schema!(Npc)),
        ("faction", schema!(Faction)),
        ("object", schema!(Object)),
        ("skill", schema!(Skill)),
        ("buff", schema!(Buff)),
        ("loot", schema!(LootTable)),
        ("item", schema!(Item)),
        ("minifig", schema!(Minifigure)),
        ("dialogue", schema!(Dialogue)),
        ("achievement", schema!(Achievement)),
        ("content", schema!(Manifest)),
        ("tunables", schema!(Tunables)),
        ("texts", schema!(Texts)),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn slugs() {
        assert_eq!(super::slug("XX_Snake Shot"), "xx-snake-shot");
        assert_eq!(super::slug("  Ship's Mate! "), "ship-s-mate");
        assert_eq!(super::slug("???"), "unnamed");
    }

    #[test]
    fn kinds() {
        assert_eq!(super::kind_of("npc/pirate-specter"), Some("npc"));
        assert_eq!(super::kind_of("spawn/4200/x"), Some("spawn"));
        assert_eq!(super::kind_of("Pirate World"), None);
        assert_eq!(super::kind_of("npcs/x"), None);
    }
}
