//! Survey sets (docs/SURVEY.md): what surveyors placed, confirmed, edited or deleted in game
//! (exact placements the server can spawn as they are), and the areas, scripts and waves they
//! marked for someone to review.
//!
//! - [`set`]: a set's folder of per-playfield files, loading and saving.
//! - [`ids`]: ids of new records and the stable ids of the base content (guesses included).
//! - [`merge`]: importing someone else's set into yours, skipping duplicates and near-duplicates.
//! - [`apply`]: base content + placements → what a playfield spawns, with each object's category.

pub mod apply;
pub mod ids;
pub mod merge;
pub mod set;

use serde::{Deserialize, Serialize};

/// A position (x, y, z) in a playfield, y up.
pub type Vec3 = [f32; 3];

/// The file format version written in every file.
pub const FORMAT: u32 = 1;

/// What a placement is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Enemy,
    Npc,
    Smashable,
    Buildie,
    Interactable,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Enemy => "enemy",
            Kind::Npc => "npc",
            Kind::Smashable => "smashable",
            Kind::Buildie => "buildie",
            Kind::Interactable => "interactable",
        }
    }
}

/// The base object a placement replaces (a confirmed, edited or deleted guess), as it was when
/// it was replaced: its id, and what finds it again if the guesses are regenerated and the id
/// changes ([`apply::apply`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub id: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monster_data: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<u32>,
    pub pos: Vec3,
}

/// The script wave that spawns a placement (rather than the playfield starting).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WaveRef {
    pub script: String,
    pub wave: String,
}

/// An exact placement: spawned as it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub id: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The look (MonsterData) of enemies, NPCs and smashables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monster_data: Option<u32>,
    /// The item template of smashables, buildies and interactables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u32>,
    pub pos: Vec3,
    /// Yaw in radians (as `world_content.json` `rot`).
    #[serde(default)]
    pub heading: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<Origin>,
    /// Deleted: with `replaces`, the guess isn't spawned; without, it marks a placement deleted
    /// so merging doesn't bring it back.
    #[serde(default, skip_serializing_if = "is_false")]
    pub removed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave: Option<WaveRef>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(default)]
    pub author: String,
    /// Unix seconds of the last change.
    #[serde(default)]
    pub updated: u64,
}

impl Placement {
    /// Whether two placements say the same thing (who saved them and when aside).
    pub fn same_content(&self, other: &Placement) -> bool {
        let strip = |p: &Placement| Placement { author: String::new(), updated: 0, ..p.clone() };
        strip(self) == strip(other)
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// An area a note or script is about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    Point(Vec3),
    Circle { center: Vec3, radius: f32 },
    /// Walked out by the surveyor, in order.
    Polygon { points: Vec<Vec3> },
}

impl Area {
    pub fn center(&self) -> Vec3 {
        match self {
            Area::Point(p) => *p,
            Area::Circle { center, .. } => *center,
            Area::Polygon { points } if points.is_empty() => [0.0; 3],
            Area::Polygon { points } => {
                let n = points.len() as f32;
                let sum = points.iter().fold([0.0; 3], |s, p| [s[0] + p[0], s[1] + p[1], s[2] + p[2]]);
                [sum[0] / n, sum[1] / n, sum[2] / n]
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    MissionArea,
    Achievement,
    Dialog,
    Script,
    Comment,
}

/// Where a reviewer stands on a note or script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    #[default]
    Unreviewed,
    Accepted,
    Rejected,
}

/// Something about an area that needs a person to check and implement it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub kind: NoteKind,
    pub area: Area,
    /// The achievement, dialogue, script ... by name or id, if the surveyor found it.
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub status: Review,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub updated: u64,
}

/// What starts a wave, as far as the surveyor could tell (the wave's comment says the rest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Seconds after the script starts (first wave) or the previous wave.
    After { seconds: f32 },
    /// A placement (usually a boss) drops below this much of its health.
    HealthBelow {
        #[serde(default)]
        placement: Option<String>,
        percent: f32,
    },
    /// A player enters a note's area (none: the script's own area).
    EnterArea {
        #[serde(default)]
        note: Option<String>,
    },
    /// Every placement of that wave is defeated.
    WaveCleared { wave: String },
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wave {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub trigger: Option<Trigger>,
    #[serde(default)]
    pub comment: String,
}

/// A scripted event: its waves spawn the placements that name them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Script {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub area: Option<Area>,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub waves: Vec<Wave>,
    #[serde(default)]
    pub status: Review,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub updated: u64,
}

/// `<playfield>.placements.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementsFile {
    pub format: u32,
    pub playfield: u32,
    #[serde(default)]
    pub placements: Vec<Placement>,
}

impl PlacementsFile {
    pub fn new(playfield: u32) -> Self {
        PlacementsFile { format: FORMAT, playfield, placements: Vec::new() }
    }
}

/// `<playfield>.notes.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotesFile {
    pub format: u32,
    pub playfield: u32,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub scripts: Vec<Script>,
}

impl NotesFile {
    pub fn new(playfield: u32) -> Self {
        NotesFile { format: FORMAT, playfield, notes: Vec::new(), scripts: Vec::new() }
    }
}

pub(crate) fn distance(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The difference between two headings (radians), in degrees, 0..=180.
pub(crate) fn heading_difference(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_examples_parse() {
        let p: PlacementsFile = serde_json::from_str(
            r#"{"format":1,"playfield":4600,"placements":[{"id":"p-a-1","kind":"enemy","name":"Skeleton Warrior",
            "monster_data":82410,"level":2,"pos":[334.0,48.75,124.0],"heading":1.57,
            "replaces":{"id":"g-1","kind":"enemy","monster_data":82410,"pos":[331.2,48.75,120.0]},
            "wave":{"script":"s-a-1","wave":"w-a-1"},"comment":"patrols","author":"a","updated":1}]}"#,
        )
        .unwrap();
        assert_eq!(p.placements[0].wave.as_ref().unwrap().wave, "w-a-1");
        let n: NotesFile = serde_json::from_str(
            r#"{"format":1,"playfield":4600,
            "notes":[{"id":"n-1","kind":"achievement","area":{"circle":{"center":[300.0,48.0,90.0],"radius":12.0}},
              "reference":"Treasure Hunter","comment":"","status":"unreviewed","author":"a","updated":0},
              {"id":"n-2","kind":"comment","area":{"point":[1.0,2.0,3.0]}}],
            "scripts":[{"id":"s-1","name":"Dock ambush","area":{"polygon":{"points":[[0,0,0],[2,0,0],[2,0,2]]}},
              "waves":[{"id":"w-1","name":"first","trigger":{"enter_area":{"note":"n-1"}}},
                       {"id":"w-2","trigger":{"health_below":{"placement":"p-1","percent":50}}},
                       {"id":"w-3","trigger":"other"}]}]}"#,
        )
        .unwrap();
        assert_eq!(n.notes[1].area.center(), [1.0, 2.0, 3.0]);
        assert_eq!(n.scripts[0].waves.len(), 3);
        // round trip
        let again: NotesFile = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(again, n);
    }

    #[test]
    fn heading_wraps() {
        assert!((heading_difference(0.1, std::f32::consts::TAU - 0.1) - 0.2f32.to_degrees()).abs() < 1e-3);
        assert!((heading_difference(0.0, std::f32::consts::PI) - 180.0).abs() < 1e-3);
    }
}
