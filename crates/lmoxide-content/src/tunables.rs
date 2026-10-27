//! Single files beside the records: `content.json` (the manifest), `tunables.json` (global
//! numbers) and `texts.json` (the client's dialogue texts).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::units::{Duration, Ratio};

/// `content.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The format version (crate::FORMAT).
    pub format: u32,
    /// Where the generated, read-only client catalogs are (monster data, effects, stats ...),
    /// relative to this folder.
    pub reference: String,
    /// Features with a title and comment (the world editor's `features.json`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub features: BTreeMap<String, serde_json::Value>,
}

/// `tunables.json`: every global number the server uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Tunables {
    pub combat: Combat,
    pub loot: Loot,
    pub npc: NpcTunables,
    pub progression: Progression,
    pub npc_life: NpcLife,
    pub rarity: Rarity,
    /// The client's minifigure stat model (SimpleChar_t::CalculateLife, AttackUtils): tweaks,
    /// player defaults, formulas.
    pub minifigure_model: serde_json::Value,
    /// Main quest giver -> its quests, marked completed for everyone (the client's teleport
    /// windows and map markers need them).
    pub story_quests: BTreeMap<String, Vec<u32>>,
    /// Anything else, by name (`section.name`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub more: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Combat {
    /// Players' level (all minifigures fight at this level without --progression).
    pub level: u32,
    pub health_per_level: f32,
    pub star_factor: f32,
    pub power_rating_all: f32,
    /// Damage varies by this share either way.
    pub variance: Ratio,
    /// Damage of normals while empowered.
    pub empowered_normals: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Loot {
    /// Teammates within this distance of a kill get their own bag (m).
    pub share_range: f32,
    pub bag_lifetime: Duration,
    pub bag_template: u32,
    pub bag_dynel_type: u32,
    pub bag_interaction_radius: u32,
    /// Which loot table each source rolls (`smashable`, `enemy`, `champion`, `boss`, `chest`,
    /// `member_chest`).
    pub tables: BTreeMap<String, String>,
    /// Items the loot rules name (`star`, `health_orb`, `potion`, `stars_25`, ...).
    pub items: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct NpcTunables {
    /// Out of combat for this long before health comes back.
    pub regen_delay: Duration,
    pub regen_tick: Duration,
    /// Share of maximum health per tick.
    pub regen_share: Ratio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Progression {
    /// Minifigure rank -> stars needed.
    pub minifigure_xp: BTreeMap<String, u32>,
    /// Player level needed per minifigure rank.
    pub rank_player_level: Vec<u32>,
}

/// NPC life on the live server: life = base[level] x the template's life factor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct NpcLife {
    pub interpolation: String,
    pub base: BTreeMap<String, u32>,
}

/// How rare each minifigure's bag and parts are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Rarity {
    /// Tier -> weight.
    pub tiers: BTreeMap<String, u32>,
    pub default: String,
    /// Part location -> weight.
    pub part_locations: BTreeMap<String, u32>,
}

/// `texts.json`: the client's dialogue texts and answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Texts {
    /// Text id -> text.
    pub texts: BTreeMap<String, String>,
    /// Text id -> the answer text id.
    pub answers: BTreeMap<String, u32>,
    /// Client text keys ("category/id" -> key).
    pub keys: BTreeMap<String, String>,
    /// Name text id -> name.
    pub names: BTreeMap<String, String>,
}
