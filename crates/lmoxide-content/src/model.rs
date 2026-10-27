//! Every kind of record. Definitions (NPCs, skills, buffs, loot tables, items, objects,
//! minifigures, dialogues, achievements, regions) are one file each; a world's records are in
//! its folder, one file per section.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::action::{Action, Destination};
use crate::record::{GameAction, Point, Ref, Rot, Vec3, record};
use crate::units::{Duration, Range, Ratio};

// ---------------------------------------------------------------------------------------------
// Worlds

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorldKind {
    /// A world players share and come back to (enemies respawn).
    Open,
    /// An adventure or side dungeon: its encounters run once per visit, nothing respawns while
    /// someone is there.
    Pocket,
    /// A hub (LEGO Center).
    Hub,
    /// Made per group on entry (Lost Creations levels).
    Instance,
    /// Development, PvP and asset test playfields.
    #[default]
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct RespawnPoint {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guid: Option<u32>,
    pub pos: Vec3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rot: Option<Rot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discoverable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restoration: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shady: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LevelRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    /// Players fight at most at this level here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<u32>,
}

/// NPC-against-NPC fights set up in a world (npc.rs `Kind::Fighter`): the spawns marked `fight`
/// take part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct NpcFights {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// Proposed enemies anchored at these areas are replaced by the fight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replace_anchors: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rivals_about: String,
    /// Sides by look (monster data): each group fights the other groups.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rivals: Vec<Vec<u32>>,
    /// NPC names that fight the world's enemies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub armed_npcs: Vec<String>,
}

record! {
    /// A playfield: `worlds/<playfield>-<slug>/world.json`.
    #[derive(Default)]
    pub struct World: "world" {
        /// The client's playfield id.
        pub playfield: u32,
        /// The name the client shows (TDB 52000), when it differs from `name`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub display_name: Option<String>,
        #[serde(default)]
        pub kind: WorldKind,
        /// The region it belongs to (`region/...`): which minifigures drop here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub region: Option<Ref>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub level: Option<LevelRange>,
        /// Where teleports arrive (a resurrection point of the client's data).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub arrival: Option<Point>,
        /// The client's resurrection point guid of `arrival`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub arrival_guid: Option<u32>,
        /// How arrival is placed when there is no point (`navmesh`, ...).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub arrival_mode: Option<String>,
        /// Arrival when coming back from another world (`world/...` -> point).
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub arrival_from: BTreeMap<Ref, Point>,
        /// The client teleport window that leads here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub window: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub respawn_points: Vec<RespawnPoint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub npc_fights: Option<NpcFights>,
        /// Music tracks (client ids).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub music: Vec<u32>,
    }
}

record! {
    /// Several worlds that share their minifigure drops ("Pirate World").
    #[derive(Default)]
    pub struct Region: "region" {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub worlds: Vec<Ref>,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpawnKind {
    #[default]
    Enemy,
    Npc,
    Smashable,
    Buildie,
    /// Levers, chests, plates, mirrors: things players use.
    Interactable,
    Door,
    /// Scenery the server sends (simple dynels: markers, signs, props).
    Scenery,
    /// An invisible NPC that starts a cinematic or dialogue when players come near.
    Trigger,
    /// A cinematic actor the client places itself (its AI point).
    Actor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FightRole {
    /// Fights the world's NPCs (`npc_fights`).
    Enemy,
    /// An NPC that fights back.
    Fighter,
}

/// A spawn's script wave: it spawns when that wave starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaveRef {
    pub script: Ref,
    pub wave: String,
}

/// Client placement details the server sends as they are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct SpawnClient {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_group: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cars_group: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynel_type: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<[f32; 6]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_radius: Option<u32>,
    /// Cinematic actors: their item template, AI point and alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_template: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_point: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cinematic: Option<bool>,
}

record! {
    /// One thing placed in a world: `worlds/<world>/spawns.json`.
    #[derive(Default)]
    pub struct Spawn: "spawn" {
        pub kind: SpawnKind,
        /// What it is: `npc/...` for enemies, NPCs and triggers, `object/...` for the rest.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub template: Option<Ref>,
        pub pos: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub rot: Option<Rot>,
        #[serde(default = "crate::record::one", skip_serializing_if = "crate::record::is_one")]
        pub scale: f32,
        /// Changes to the template for this spawn only (any of its fields: `level`, `ai`, `loot` ...).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub overrides: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub respawn: Option<Duration>,
        /// Patrol route (positions in order; looped).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub patrol: Vec<Vec3>,
        /// Spawned by a script wave instead of with the world.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub wave: Option<WaveRef>,
        /// The named area or point it was proposed for (navmesh anchor, cinematic point).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub anchor: Option<String>,
        /// Seconds into the live capture it appeared (spawned during play, not at start).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub appeared_at: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub fight: Option<FightRole>,
        /// A world-editor placement that replaced a proposed spawn: that spawn's id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub replaces: Option<Ref>,
        /// Deleted in this feature (the spawn it replaces is gone too).
        #[serde(default, skip_serializing_if = "crate::record::is_false")]
        pub removed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub client: Option<SpawnClient>,
    }
}

record! {
    /// A portal: `worlds/<world>/portals.json`.
    #[derive(Default)]
    pub struct Portal: "portal" {
        pub pos: Vec3,
        /// Walk-in radius (m).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub radius: Option<f32>,
        /// The client teleport window it opens (none: straight to `to`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub window: Option<u32>,
        pub to: Destination,
    }
}

record! {
    /// A jump pad placed in a world: `worlds/<world>/bouncers.json`.
    #[derive(Default)]
    pub struct Bouncer: "bouncer" {
        /// Its kind (`object/...` with `bouncer`): look and launch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub template: Option<Ref>,
        pub pos: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub rot: Option<Rot>,
        #[serde(default = "crate::record::one", skip_serializing_if = "crate::record::is_one")]
        pub scale: f32,
        /// Where it throws you; none: it doesn't launch (shown only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub target: Option<Vec3>,
        /// How the landing point was found (`name`, `adjacent`, `chain`, `demo`, `onward`, ...).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub target_by: Option<String>,
        /// Arc height (m) and flight time as recorded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub apex: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub flight: Option<Duration>,
        /// The next pad of a route (thrown on without stopping).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub then: Option<Ref>,
        /// Works only after this buildie spawn is built.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub requires_built: Option<Ref>,
        /// The world-editor bouncer this one replaced.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub replaces: Option<Ref>,
        #[serde(default, skip_serializing_if = "crate::record::is_false")]
        pub removed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub client: Option<SpawnClient>,
    }
}

/// The shape of an area or trigger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    /// An upright box standing on `pos` (bottom centre), `size` = width, height, depth,
    /// turned by `heading`.
    Wall {
        pos: Vec3,
        #[serde(default)]
        heading: f32,
        size: Vec3,
    },
    /// A cylinder.
    Circle {
        pos: Vec3,
        radius: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<f32>,
    },
    /// A ground polygon (x, z) between two heights.
    Polygon {
        points: Vec<[f32; 2]>,
        heights: [f32; 2],
    },
    /// A ground position (x, z), at whatever height the ground is there.
    Ground { at: [f32; 2] },
    /// A point (cinematic and AI points).
    Point {
        pos: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rot: Option<Rot>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum AreaRole {
    /// Just a named place.
    #[default]
    Place,
    /// A boss or miniboss arena (doors close while the fight runs).
    Arena,
    Ambush,
    /// An exploration area (discovering it counts for achievements).
    Explore,
    /// Where a side dungeon's champion stands.
    Champion,
    /// A named point the client's cinematics and AI use.
    Marker,
    /// A world-editor note's area.
    Note,
}

/// Doors of an arena by client guid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ArenaDoors {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exit: Vec<u32>,
}

record! {
    /// A named area or point: `worlds/<world>/areas.json`.
    #[derive(Default)]
    pub struct Area: "area" {
        #[serde(default)]
        pub role: AreaRole,
        pub shape: Option<Shape>,
        /// The client's name for it (bounded area or point name), when `name` differs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub client_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub suffix: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub doors: Option<ArenaDoors>,
        /// World-editor notes: what it is about and how far it is checked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub note: Option<serde_json::Value>,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum TriggerOn {
    #[default]
    Enter,
    Exit,
    Interact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Once {
    /// Every time.
    #[default]
    Never,
    PerPlayer,
    /// Once per running world (resets when it empties).
    PerInstance,
    Global,
}

record! {
    /// A trigger: `worlds/<world>/triggers.json`.
    #[derive(Default)]
    pub struct Trigger: "trigger" {
        pub shape: Option<Shape>,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub on: TriggerOn,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub once: Once,
        /// How it looks in game (`invisible`, `shield`, `forcefield`, ...); the world editor's colours.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub look: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub actions: Vec<Action>,
        #[serde(default, skip_serializing_if = "crate::record::is_false")]
        pub removed: bool,
    }
}

/// What starts a wave.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum WaveTrigger {
    /// After the script starts (first wave) or the previous wave starts.
    After(Duration),
    /// A spawn (none: the script's strongest) drops below this share of its health.
    HealthBelow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn: Option<Ref>,
        share: Ratio,
    },
    /// A player enters this area (none: the script's own).
    EnterArea(Option<Ref>),
    /// Every spawn of that wave is defeated.
    WaveCleared(String),
    /// Started only by a trigger or command naming the wave.
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Wave {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<WaveTrigger>,
    /// Buffs every spawn of the wave gets (elemental damage, tougher enemies).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buffs: Vec<BuffUse>,
    /// Run when the wave starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScriptStart {
    /// From a trigger, a wave's `enter_area`, or a command.
    #[default]
    Trigger,
    /// When the world starts (and again after it resets).
    OnWorldLoad,
}

/// One version of a script, picked by weight among those whose `when` fits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub id: String,
    #[serde(default = "one_u32")]
    pub weight: u32,
    /// Script parameters it needs: `{ "lc_level": [1, 5] }`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub when: BTreeMap<String, Range>,
    /// Changes to the script's waves by wave id (any wave fields).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub waves: BTreeMap<String, serde_json::Value>,
}

fn one_u32() -> u32 {
    1
}

record! {
    /// A scripted event: `worlds/<world>/scripts/<slug>.json`. Its waves spawn the spawns that
    /// name them.
    #[derive(Default)]
    pub struct Script: "script" {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub area: Option<Ref>,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub start: ScriptStart,
        /// Parameters with their defaults (`lc_level`), set by whatever starts it.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub params: BTreeMap<String, serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub variants: Vec<Variant>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub waves: Vec<Wave>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub on_complete: Vec<Action>,
        /// Footage or a description of what live did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub reference: Option<String>,
        /// The world editor's review state.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub status: Option<String>,
    }
}

/// Mirrors reflecting a beam onto a target (mirrors.rs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Mirror {
    pub guid: u32,
    pub name: String,
    pub pos: Vec3,
    /// The mirror each turn sends the beam to (`plus`/`minus`; none: nowhere).
    pub out: BTreeMap<String, Option<String>>,
    pub beam: BTreeMap<String, Beam>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Beam {
    pub pos: Vec3,
    pub rot: Rot,
    pub length: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct NamedPos {
    pub name: String,
    pub pos: Vec3,
}

/// A gate opened by light reaching it through mirrors (gates.rs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LightGateMirror {
    pub guid: u32,
    pub pos: Vec3,
    pub light: Vec3,
    pub rot: Rot,
    pub length: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PuzzleKind {
    Mirrors {
        mirrors: Vec<Mirror>,
        roots: Vec<String>,
        targets: Vec<NamedPos>,
        opens: BTreeMap<String, serde_json::Value>,
    },
    LightGate {
        gate: u32,
        pos: Vec3,
        mirrors: Vec<LightGateMirror>,
    },
}

impl Default for PuzzleKind {
    fn default() -> Self {
        PuzzleKind::LightGate {
            gate: 0,
            pos: [0.0; 3],
            mirrors: vec![],
        }
    }
}

record! {
    /// A puzzle the server runs: `worlds/<world>/puzzles.json`.
    #[derive(Default)]
    pub struct Puzzle: "puzzle" {
        pub puzzle: PuzzleKind,
    }
}

// ---------------------------------------------------------------------------------------------
// NPCs and enemies

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum NpcKind {
    #[default]
    Enemy,
    Npc,
    Boss,
    Champion,
    /// Summoned by a skill.
    Summon,
    /// Invisible: starts a cinematic or dialogue.
    Trigger,
}

/// How a template looks in the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Render {
    /// The look: client monster data (RDB 1010514).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monster_data: Option<u32>,
    /// Client NPC template (TDB 51000 name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npc_template: Option<u32>,
    /// Client item template (smashables, buildies, chests, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<u32>,
    /// The look's name, for reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cars_group: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_package: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct SkillUse {
    pub skill: Ref,
    /// How often it's picked against the others.
    #[serde(default = "one_u32")]
    pub weight: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Ai {
    /// With several attacks, use the ranged ones while it can.
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub prefer_ranged: bool,
    /// Notices players this close (m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggro: Option<f32>,
    /// Gives up this far from home (m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leash: Option<f32>,
    /// `stand`, `wander`, `route` (the spawn's patrol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patrol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flee_below: Option<Ratio>,
    /// How far it reaches with melee (m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<f32>,
    /// Doesn't move or attack (scenery NPCs).
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub passive: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct NpcStats {
    /// Health: a fixed number; none: from `tunables.npc_life` by level x `life_factor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<u32>,
    /// This look's life against the live table (npc_life).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub life_factor: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defense: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walk: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<f32>,
}

record! {
    /// An NPC or enemy: `npcs/<slug>.json`.
    #[derive(Default)]
    pub struct Npc: "npc" {
        #[serde(default)]
        pub kind: NpcKind,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub render: Render,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub level: Option<u32>,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub stats: NpcStats,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub faction: Option<Ref>,
        /// Its basic attack.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub attack: Option<Ref>,
        /// Other skills it casts.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub skills: Vec<SkillUse>,
        /// How its skills were chosen (`recorded`, `named`, `generic`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub skills_from: Option<String>,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub ai: Ai,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub loot: Option<Ref>,
        /// Smash and collect achievement categories it counts for.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub counts_as: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub dialogue: Option<Ref>,
        /// Summons: how long they stay.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub lifetime: Option<Duration>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub on_defeat: Vec<Action>,
    }
}

record! {
    /// Who fights whom: `factions/<slug>.json`.
    #[derive(Default)]
    pub struct Faction: "faction" {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub enemies: Vec<Ref>,
    }
}

// ---------------------------------------------------------------------------------------------
// Objects: smashables, buildies, bouncers, doors, chests, scenery

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    #[default]
    Smashable,
    Buildie,
    Bouncer,
    Door,
    Interactable,
    Scenery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Build {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<Duration>,
    /// Build points needed (the client's max health of the buildie).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_visual: Option<u32>,
    /// Gone once built (bouncers show their pad instead).
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub hide_after: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    /// The effect players get when thrown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apex: Option<f32>,
}

/// Client object details sent as they are (buildies' recorded spawn data).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ObjectClient {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynel_type: Option<u32>,
    /// The client's own loot table id (its loot isn't in the client).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loot_table: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub particles: Vec<[u32; 5]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<[f32; 6]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructible: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<serde_json::Value>,
}

record! {
    /// A kind of object: `objects/<slug>.json`.
    #[derive(Default)]
    pub struct Object: "object" {
        pub kind: ObjectKind,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub render: Render,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub level: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub health: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub loot: Option<Ref>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub respawn: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub build: Option<Build>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub launch: Option<Launch>,
        /// States it can be in (`open`, `closed`, `built`, ...), the first is where it starts.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub states: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub on_use: Vec<Action>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub on_built: Vec<Action>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub on_smashed: Vec<Action>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub client: Option<ObjectClient>,
    }
}

// ---------------------------------------------------------------------------------------------
// Skills and buffs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    #[serde(rename = "self")]
    Owner,
    Ally,
    Enemy,
    Smashable,
    Ground,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct Projectile {
    /// The client item it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// 1 = straight, 2 = lobbed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attractor: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_z: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ricochets: Option<u32>,
    /// Hit radius (m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    /// Charged shots: tick, the item per charge level, the counter buff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_interval: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub charge_items: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter_buff: Option<u32>,
    /// Anything else the extractor recorded.
    #[serde(default, flatten, skip_serializing_if = "BTreeMap::is_empty")]
    pub more: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Dash {
    pub distance: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub backwards: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_mod: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival_mod: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival_radius: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_visuals: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arrival_visuals: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Summon {
    pub npc: Ref,
    #[serde(default = "one_u32")]
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<crate::action::SpawnAt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct AreaOfEffect {
    /// `circle`, `cone`, `column`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    pub radius: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_targets: Option<u32>,
    /// `self` or `target`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

/// The client's own definition of a spell or nano: its stats by name and its game action lists
/// (list number -> actions). The server runs these where a skill has no `effects` of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ClientSpell {
    /// The client spell / nano id.
    pub spell: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stats: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, Vec<GameAction>>,
}

record! {
    /// A skill (ability, attack, nano): `skills/<slug>.json`.
    #[derive(Default)]
    pub struct Skill: "skill" {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub client: Option<ClientSpell>,
        /// Which minifigure ability it belongs to, as the extractor found it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub minifigure: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub target: Vec<TargetKind>,
        /// Range (m).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub range: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cooldown: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cast_time: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub projectile: Option<Projectile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub dash: Option<Dash>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub summon: Option<Summon>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub area: Option<AreaOfEffect>,
        /// What a hit does (server-native skills; client ones use `client.actions`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub effects: Vec<Action>,
        /// The AI's use: `melee`, `ranged`, `heal`, ...
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub ai_category: Option<String>,
    }
}

/// How a stat changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum StatOp {
    /// Adds the value.
    #[default]
    Add,
    /// Multiplies by (1 + value).
    Mul,
    /// Sets the value.
    Set,
}

/// When a proc goes off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcOn {
    /// Any attack of its holder hits.
    Hit,
    NormalHit,
    SpecialHit,
    /// The holder attacks (hit or not).
    Attack,
    NormalAttack,
    SpecialAttack,
    /// The holder is hit.
    Damaged,
    /// The holder smashes an enemy.
    Kill,
    SpecialKill,
    /// The holder's health drops below `below`.
    HealthBelow,
    /// The holder hasn't been hit for `after`.
    NotHitFor,
    /// Every `every_time`.
    Interval,
    /// The holder smashes a smashable.
    Smash,
    /// The holder builds a buildie.
    Build,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Proc {
    pub on: ProcOn,
    /// Every nth time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chance: Option<Ratio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<Ratio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_time: Option<Duration>,
    /// Can't go off again for this long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockout: Option<Duration>,
    /// Only while the holder channels a special.
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub channelling: bool,
    pub then: Vec<Action>,
}

/// What a buff does while it's on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum BuffEffect {
    /// Changes a stat. `stat`: a name the server knows (`damage`, `defense`, `max_health`,
    /// `move_speed`, `build_speed`, `special_recharge`, `normal_recharge`, `health_regen`,
    /// `mitigation`, `orb_chance`, ...) or a client stat name or number. `value`: a number,
    /// `"20%"`, or `"amount"` / `"amount * 0.5"` (the buff's amount, or the caller's).
    Stat {
        stat: serde_json::Value,
        #[serde(default)]
        op: StatOp,
        value: serde_json::Value,
    },
    /// Deals its holder's damage as this type (elemental damage).
    DamageType(serde_json::Value),
    Proc(Proc),
    /// Changes a skill of its holder (`normal`, `special` or `skill/...`): more projectiles,
    /// faster recharge, ...
    SkillMod {
        skill: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projectiles: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cooldown: Option<Ratio>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ability_mod: Option<Ratio>,
    },
    /// Replaces a shortcut's skill while on (stances, combos: OverrideShortcut).
    Override {
        slot: u32,
        skill: Ref,
    },
    /// A client game action as the client's data has it.
    Client(GameAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Stacking {
    /// Again: the time starts over.
    #[default]
    Refresh,
    Stack,
    Replace,
    /// Again: nothing happens.
    Ignore,
}

record! {
    /// A buff, debuff, upgrade or passive: `buffs/<slug>.json`.
    #[derive(Default)]
    pub struct Buff: "buff" {
        /// The client nano / ability that shows it (icon, visuals), if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub nano: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub effect_package: Option<u32>,
        /// Default size, used by effects' `"amount"`; callers may give their own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub amount: Option<serde_json::Value>,
        /// None: until removed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub duration: Option<Duration>,
        /// Also ends (used up) when this happens: `hit` (the next attack), `damaged` (the next hit taken).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub ends_on: Option<ProcOn>,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub stacking: Stacking,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_stacks: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub effects: Vec<BuffEffect>,
    }
}

/// A buff as a wave, skill or upgrade gives it, optionally with its own size and time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct BuffUse {
    pub buff: Ref,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stacks: Option<u32>,
}

// ---------------------------------------------------------------------------------------------
// Loot and items

/// Minifigure parts or bags: from these minifigures, or from what the world drops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MinifigureSource {
    /// `"world"`: the minifigures of the world it drops in, by rarity.
    World(String),
    Some(Vec<Ref>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MinifigureDrop {
    pub from: MinifigureSource,
    #[serde(default = "one_range")]
    pub count: Range,
}

fn one_range() -> Range {
    Range::one(1)
}

/// What one loot entry gives; every part optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LootBundle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stars: Option<Range>,
    /// Health orbs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hearts: Option<Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub potions: Option<Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minifig_parts: Option<MinifigureDrop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minifig_bags: Option<MinifigureDrop>,
    /// One of these chests (`item/...`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chest: Vec<Ref>,
    /// These items (mission items, star bundles, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Ref>,
    /// Another table rolled too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<Ref>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LootWhen {
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub once_per_player: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worlds: Vec<Ref>,
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub members_only: bool,
    /// Raised by this buff's `orb_chance` (Resourceful).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bonus_stat: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LootEntry {
    /// Rolled on its own with this chance (`always` when neither this nor `weight`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chance: Option<Ratio>,
    /// Or: one of the weighted entries is picked per roll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    pub drop: LootBundle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<LootWhen>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

record! {
    /// A loot table (several enemies or objects can share one): `loot/<slug>.json`.
    #[derive(Default)]
    pub struct LootTable: "loot" {
        /// How many times the weighted entries are rolled.
        #[serde(default = "one_u32")]
        pub rolls: u32,
        pub entries: Vec<LootEntry>,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A minifigure part (head, torso, legs).
    Part,
    /// A bag that unlocks a minifigure.
    MinifigBag,
    Chest,
    /// Stars, potions and other counters.
    #[default]
    Token,
    HealthOrb,
    /// What a projectile is (visual and on-hit actions).
    Projectile,
    MissionItem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ChestInfo {
    /// 1 free, 2 members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loot_type: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_loot_table: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_effect_package: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lua_script: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<serde_json::Value>,
    /// What opening it gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loot: Option<Ref>,
    /// Not dropped (the client's Endless chests).
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub unused: bool,
}

/// A token an item adds to, and how much (negative: takes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct TokenAmount {
    pub token: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,
}

record! {
    /// An item: `items/<slug>.json`.
    #[derive(Default)]
    pub struct Item: "item" {
        pub kind: ItemKind,
        /// The client item template.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub template: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub dynel_type: Option<u32>,
        /// The tokens it adds to (stars, potions, part tokens ...).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub tokens: Vec<TokenAmount>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub minifigure: Option<Ref>,
        /// Parts: `Head`, `Torso`, `Legs`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub location: Option<String>,
        /// Bags: the client's ControlledLootType (3 regular, 4/5 tutorial).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub controlled_loot_type: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub monster_data: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub effect_package: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub chest: Option<ChestInfo>,
        /// Projectiles: hit radius (m).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub radius: Option<f32>,
        /// Client game actions it runs (bags: what they spawn; projectiles: on hit).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub actions: Vec<GameAction>,
    }
}

// ---------------------------------------------------------------------------------------------
// Minifigures

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct MinifigureStats {
    pub life: u32,
    pub power: u32,
    pub defense: u32,
    pub creativity: u32,
    pub role: u32,
    pub element: u32,
}

/// One hit of an attack, as the extractor followed it through the client's spells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Hit {
    pub skill: Ref,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ability_mod: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage_type: Option<u32>,
    /// The client spells it goes through.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Attack {
    /// The name the game shows for it ("Throw Record").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub skill: Ref,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hits: Vec<Hit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Upgrade {
    /// Its bit in the minifigure's upgrade mask (shop order).
    pub bit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u32>,
    pub buff: Ref,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct MinifigureObtain {
    /// `world`, `store`, `membership`, `achievement`, `event`, `club`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<Ref>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub achievement: Option<u32>,
    /// The client's "You can find me in ..." text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// `common`, `rare`, `epic` (tunables.rarity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rarity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_text: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_picture: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct MinifigurePart {
    pub location: String,
    pub item: Ref,
}

record! {
    /// A playable minifigure: `minifigures/<slug>.json`. New ones need client patches (the
    /// collection comes from the client's data); their numbers are free to change.
    #[derive(Default)]
    pub struct Minifigure: "minifig" {
        /// The client minifigure item template.
        pub template: u32,
        pub series: u32,
        #[serde(default)]
        pub series_index: u32,
        #[serde(default, skip_serializing_if = "crate::record::is_default")]
        pub render: Render,
        pub stats: MinifigureStats,
        pub normal: Attack,
        pub special: Attack,
        /// The client's left/right click spells.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub click_spells: Vec<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub upgrades: Vec<Upgrade>,
        /// Achievement auras: the four lore tags.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub aura_tags: Vec<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub parts: Vec<MinifigurePart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub bag: Option<Ref>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub obtain: Option<MinifigureObtain>,
    }
}

// ---------------------------------------------------------------------------------------------
// Dialogues and achievements

record! {
    /// An NPC dialogue: `dialogues/<slug>.json`. Its lines are client text ids (`texts.json`).
    #[derive(Default)]
    pub struct Dialogue: "dialogue" {
        pub texts: Vec<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub world: Option<Ref>,
        /// Who speaks: the client's name text id and the name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub npc_name: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub npc: Option<String>,
        /// Where it starts by itself (proximity).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub start: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub pos: Option<Vec3>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub follows: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "crate::record::is_false")]
        pub seen_live: bool,
    }
}

/// A smash or collect rule a world editor added to an achievement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct AchievementRule {
    /// Enemy names (`@category` for a category's).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enemies: Vec<String>,
    #[serde(default, skip_serializing_if = "crate::record::is_false")]
    pub once: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<u32>,
}

record! {
    /// An achievement (the client's lore tree), or a rule-only key (`category:`, `drop:`,
    /// `find:`): `achievements/<slug>.json`.
    #[derive(Default)]
    pub struct Achievement: "achievement" {
        /// The client's lore id (none: a rule-only record).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub lore: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub parent: Option<Ref>,
        /// The client's internal name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub client_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub points: Option<u32>,
        /// `target`, `counter`, `defeat`, `find`, `collect`, `all`, `any`, ...
        #[serde(default, skip_serializing_if = "String::is_empty")]
        pub kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub world: Option<Ref>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub find: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub place: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub stars: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub target: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub children: Vec<Ref>,
        /// Client stat and the count needed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub counter: Option<[u32; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub minifigure: Option<Ref>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub n: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub counts: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub series: Option<u32>,
        #[serde(default, skip_serializing_if = "crate::record::is_false")]
        pub invisible: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub rule: Option<AchievementRule>,
    }
}
