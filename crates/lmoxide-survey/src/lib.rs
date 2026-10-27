//! Survey sets (docs/SURVEY.md): what surveyors placed, confirmed, edited or deleted in game
//! (exact placements the server can spawn as they are), and the areas, scripts and waves they
//! marked for someone to review.
//!
//! - [`set`]: a set's folder of per-playfield files, loading and saving.
//! - [`ids`]: ids of new records and the stable ids of the base content (guesses included).
//! - [`merge`]: importing someone else's set into yours, skipping duplicates and near-duplicates.
//! - [`apply`]: base content + placements → what a playfield spawns, with each object's category.
//! - [`features`]: what each record is for (mimic live, new content, ...), nested and copyable.
//! - [`review`]: a working set against an approved one: the changes to approve, and summaries.

pub mod apply;
pub mod features;
pub mod ids;
pub mod merge;
pub mod review;
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
    /// Enemies and NPCs: the looping route it walks (as `world_content.json` `patrol`; two or
    /// more points, the last leading back to the first). Empty: it stays where it is placed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patrol: Vec<Vec3>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// The feature it belongs to ([`features`]; `live`: mimicking live).
    #[serde(default = "features::live", skip_serializing_if = "features::is_live")]
    pub feature: String,
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
    /// The feature it belongs to ([`features`]; `live`: mimicking live).
    #[serde(default = "features::live", skip_serializing_if = "features::is_live")]
    pub feature: String,
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
    /// The feature it belongs to ([`features`]; `live`: mimicking live).
    #[serde(default = "features::live", skip_serializing_if = "features::is_live")]
    pub feature: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub updated: u64,
}

/// What crossing a trigger wall does (what isn't known yet goes in the wall's comment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallAction {
    /// Starts a script's wave (none: its first).
    Script {
        #[serde(default)]
        script: Option<String>,
        #[serde(default)]
        wave: Option<String>,
    },
    /// Discovers a location (an exploration area's name, if found), counting toward an
    /// achievement (its id, if known; some need several locations).
    Discover {
        #[serde(default)]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        achievement: Option<String>,
    },
    /// Opens a dialogue (its first text's id, which names it, if found; or a description).
    Dialog {
        #[serde(default)]
        dialog: Option<String>,
    },
    /// Opens a portal: a teleport window (its id; `to` is where it leads, for reading), or
    /// with only `to` a trip to that playfield.
    Portal {
        #[serde(default)]
        window: Option<u32>,
        #[serde(default)]
        to: Option<u32>,
    },
    /// Shows a message.
    Message {
        #[serde(default)]
        text: String,
    },
    Other,
}

/// A trigger wall: an upright box (`size`: width across, height, depth along the facing) standing
/// on `pos` (its bottom centre), turned by `heading`; a player walking through it sets off
/// `action`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerWall {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub pos: Vec3,
    #[serde(default)]
    pub heading: f32,
    pub size: Vec3,
    /// A trigger radius instead of a wall: a cylinder of this radius (m) and `size`'s height
    /// around `pos`; walking into it sets it off (the width and depth are unused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    /// How the survey server draws it ("green", "shield", ...; the server's list).
    #[serde(default = "default_color")]
    pub color: String,
    pub action: WallAction,
    /// Only the first crossing (per player) counts.
    #[serde(default, skip_serializing_if = "is_false")]
    pub once: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// Deleted (kept so merging doesn't bring it back).
    #[serde(default, skip_serializing_if = "is_false")]
    pub removed: bool,
    /// The feature it belongs to ([`features`]; `live`: mimicking live).
    #[serde(default = "features::live", skip_serializing_if = "features::is_live")]
    pub feature: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub updated: u64,
}

fn default_color() -> String {
    "green".into()
}

impl TriggerWall {
    pub fn same_content(&self, other: &TriggerWall) -> bool {
        let strip = |w: &TriggerWall| TriggerWall { author: String::new(), updated: 0, ..w.clone() };
        strip(self) == strip(other)
    }

    /// Where `p` is in the wall's own frame: across (x), up (y, from its bottom), along its
    /// facing (z). Heading h faces (sin h, cos h) in x/z.
    pub fn local(&self, p: Vec3) -> Vec3 {
        let (s, c) = self.heading.sin_cos();
        let d = [p[0] - self.pos[0], p[1] - self.pos[1], p[2] - self.pos[2]];
        [d[0] * c - d[2] * s, d[1], d[0] * s + d[2] * c]
    }

    /// Whether moving from `a` to `b` goes through the wall (into its box, or across its middle
    /// plane within its width and height).
    pub fn crossed(&self, a: Vec3, b: Vec3) -> bool {
        if let Some(r) = self.radius {
            let h = self.size[1];
            let inside = |p: Vec3| (p[0] - self.pos[0]).hypot(p[2] - self.pos[2]) <= r && (-1.0..=h).contains(&(p[1] - self.pos[1]));
            return inside(b) && !inside(a);
        }
        let (la, lb) = (self.local(a), self.local(b));
        let [w, h, d] = self.size;
        let inside = |l: Vec3| l[0].abs() <= w / 2.0 && (-1.0..=h).contains(&l[1]) && l[2].abs() <= d.max(0.1) / 2.0;
        if inside(lb) && !inside(la) {
            return true;
        }
        if la[2] * lb[2] < 0.0 {
            let t = la[2] / (la[2] - lb[2]);
            let (x, y) = (la[0] + (lb[0] - la[0]) * t, la[1] + (lb[1] - la[1]) * t);
            return x.abs() <= w / 2.0 && (-1.0..=h).contains(&y);
        }
        false
    }
}

/// `<playfield>.placements.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementsFile {
    pub format: u32,
    pub playfield: u32,
    #[serde(default)]
    pub placements: Vec<Placement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<TriggerWall>,
}

impl PlacementsFile {
    pub fn new(playfield: u32) -> Self {
        PlacementsFile { format: FORMAT, playfield, placements: Vec::new(), triggers: Vec::new() }
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
            "wave":{"script":"s-a-1","wave":"w-a-1"},"patrol":[[330.0,48.75,120.0],[340.0,48.75,120.0]],
            "comment":"patrols","author":"a","updated":1}]}"#,
        )
        .unwrap();
        assert_eq!(p.placements[0].wave.as_ref().unwrap().wave, "w-a-1");
        assert_eq!(p.placements[0].patrol.len(), 2);
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
    fn crossing_a_wall() {
        // 4 m wide, facing +z at the origin
        let w = TriggerWall {
            id: "t".into(),
            name: String::new(),
            pos: [0.0; 3],
            heading: 0.0,
            size: [4.0, 3.0, 1.0],
            radius: None,
            color: "red".into(),
            action: WallAction::Other,
            once: false,
            comment: String::new(),
            removed: false,
            feature: crate::features::live(),
            author: String::new(),
            updated: 0,
        };
        assert!(w.crossed([0.0, 0.0, -3.0], [0.0, 0.0, 3.0]));
        assert!(w.crossed([1.5, 0.0, -1.0], [1.5, 0.0, 0.2]));
        assert!(!w.crossed([3.0, 0.0, -3.0], [3.0, 0.0, 3.0])); // beside it
        assert!(!w.crossed([0.0, 5.0, -3.0], [0.0, 5.0, 3.0])); // over it
        assert!(!w.crossed([0.0, 0.0, 0.2], [0.0, 0.0, 0.3])); // already inside
        // turned 90°: its width runs along z
        // a trigger radius goes off on walking into it, from any side
        let c = TriggerWall { radius: Some(3.0), ..w.clone() };
        assert!(c.crossed([5.0, 0.0, 0.0], [2.5, 0.0, 0.0]) && c.crossed([0.0, 0.0, -4.0], [0.0, 0.0, -2.9]));
        assert!(!c.crossed([1.0, 0.0, 0.0], [2.0, 0.0, 0.0]) && !c.crossed([2.0, 0.0, 0.0], [5.0, 0.0, 0.0]));
        let t = TriggerWall { heading: std::f32::consts::FRAC_PI_2, ..w };
        assert!(t.crossed([-3.0, 0.0, 1.5], [3.0, 0.0, 1.5]));
        assert!(!t.crossed([0.0, 0.0, -3.0], [0.0, 0.0, -2.5]));
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""action":"other""#));
        let back: TriggerWall = serde_json::from_str(r#"{"id":"x","pos":[0,0,0],"size":[1,2,3],"action":{"script":{"script":"s"}}}"#).unwrap();
        assert_eq!((back.color.as_str(), back.action), ("green", WallAction::Script { script: Some("s".into()), wave: None }));
    }

    #[test]
    fn heading_wraps() {
        assert!((heading_difference(0.1, std::f32::consts::TAU - 0.1) - 0.2f32.to_degrees()).abs() < 1e-3);
        assert!((heading_difference(0.0, std::f32::consts::PI) - 180.0).abs() < 1e-3);
    }
}
