//! Single files beside the records: `content.json` (the manifest), `tunables.json` (global
//! numbers) and `texts.json` (the client's dialogue texts).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::record::{Point, Ref};
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
    /// The Lost Creations runs (docs/LOST-CREATIONS.md; the levels are `lc` records).
    #[serde(default)]
    pub lost_creations: LostCreations,
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
    /// How long "this attack" lasts for procs that change an attack's hits (an area attack
    /// hits several enemies): Berserker's strike, Knockback.
    pub this_attack: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Loot {
    /// Teammates within this distance of a kill get their own bag (m).
    pub share_range: f32,
    pub bag_lifetime: Duration,
    /// A bag nobody has looted from opens to every player after this long.
    pub bag_public_after: Duration,
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
    /// Out of combat this long after the last hit taken, enemy attack or damage dealt to an
    /// enemy before health comes back at `regen_share`.
    pub regen_delay: Duration,
    pub regen_tick: Duration,
    /// Share of maximum health per tick out of combat (rounded down).
    pub regen_share: Ratio,
    /// Share of maximum health per tick in combat (rounded down).
    pub regen_share_in_combat: Ratio,
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

/// The Lost Creations: how a run goes (docs/LOST-CREATIONS.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LostCreations {
    /// The door that opens the "Lost Creations" window (door guid, in `entry_world`).
    pub entry_door: u32,
    #[schemars(extend("x-lmoxide-ref" = ["world"]))]
    pub entry_world: Ref,
    /// The quest (hash) the entry window requires, "Proved your strength at the Gauntlet" (the
    /// client checks the journal; every story quest is in it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requirement_quest: String,
    /// How long a level runs (the timer).
    pub level_time: Duration,
    /// The countdown after Start, before the start gate opens.
    pub warmup: Duration,
    /// How long teammates have to answer the invitation (no answer: as Cancel).
    pub invitation_time: Duration,
    /// How long a player at the exit door has to answer Continue / End Run (no answer: End Run).
    pub continue_time: Duration,
    /// After the whole team of a player is smashed, before they go to the Treasure Chamber (the
    /// "You were smashed!" splash).
    pub smashed_delay: Duration,
    /// Every this many levels the Gatekeeper's level comes (5, 10, ...), and only then.
    pub gatekeeper_every: u32,
    /// The run's element changes every this many levels.
    pub element_block: u32,
    /// Where players go when their run ends.
    #[schemars(extend("x-lmoxide-ref" = ["world"]))]
    pub treasure_chamber: Ref,
    pub treasure_arrival: Point,
    /// The chest a player's rewards come in: it drops where it stands when they walk into the
    /// Treasure Chamber's `BA_TriggerDropLoot`; opening it drops the parts.
    #[schemars(extend("x-lmoxide-ref" = ["item"]))]
    pub reward_chest: Ref,
    pub reward_chest_pos: [f32; 3],
    /// What enemies drop inside a run (hearts only; smashables drop as anywhere).
    #[schemars(extend("x-lmoxide-ref" = ["loot"]))]
    pub enemy_loot: Ref,
    /// The reward pools, lowest first (`minifig/...`): one more is unlocked every
    /// `levels_per_pool` levels completed.
    #[schemars(extend("x-lmoxide-ref" = ["minifig"]))]
    pub reward_pools: Vec<Vec<Ref>>,
    pub levels_per_pool: u32,
    /// Parts per unlocked pool: from, to (each a part of either minifigure of the pool).
    pub parts_per_pool: [u32; 2],
    /// The client's buffs: the level timer (the HUD's clock), no equipping, no rebuilds; the
    /// start button's arrow.
    pub timer_buff: u32,
    pub cant_equip_buff: u32,
    pub cant_resurrect_buff: u32,
    pub button_arrow_buff: u32,
    /// The effect package played on the button when the countdown starts.
    pub button_effect: u32,
    /// For testing: every level is this one (`lc/...`), the Gatekeeper's too (unset: any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-lmoxide-ref" = ["lc"]))]
    pub force_level: Option<Ref>,
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
