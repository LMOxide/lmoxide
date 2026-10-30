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
    /// The Lost Creations runs (docs/LOST-CREATIONS.md; the levels are `lc` records).
    #[serde(default)]
    pub lost_creations: LostCreations,
    /// Potions, health orbs and level-ups.
    #[serde(default)]
    pub health: Health,
    /// Smashed minifigures: rebuild time, the swap's knockaway and immunity.
    #[serde(default)]
    pub smashed: Smashed,
    /// Zone elements: Wrong Element and Elemental Affinity.
    #[serde(default)]
    pub elements: Elements,
    /// Champions' element auras.
    #[serde(default)]
    pub champions: Champions,
    /// The story: narrated cinematics, quest pickups.
    #[serde(default)]
    pub story: Story,
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
    /// Players' stuns on enemies (Petrifying Gaze, Touch of Midas, the Wacky Witch's...) last
    /// this long after they land. Unset: the stun's own TimeExist (Petrifying Gaze: 3 s after
    /// the gaze's last tick, as on live).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stun_linger: Option<Duration>,
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
    /// Loot power level per player level (the server Lua: `playerLevel * 100`).
    #[serde(default)]
    pub power_per_level: u32,
    /// Super-region -> the highest loot power level its chests and quest rewards roll at
    /// (DoSuperChestPowerLevel: Pirate 1000, Medieval 1500, Space 2000, Mythology 2500, Dino 3000).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub power_caps: BTreeMap<String, u32>,
    /// An enemy's or smashable's loot power level per its level, when its template has no
    /// `stats.power_level` (the client's NPC PowerLevel stat 499).
    #[serde(default)]
    pub npc_power: u32,
    /// Quest rewards (the "Quest Completion Loot Spawner" scripts): playfield thousands
    /// (`4000` = 4000-4999) -> the highest power there; other playfields (the hub,
    /// micro-dungeons) aren't capped.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quest_power_caps: BTreeMap<String, u32>,
    /// Quest rewards: playfield thousands -> the lowest power there (the scripts' Dino branch
    /// uses math.max where the others use math.min: at least 3000 in 6000-6999).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quest_power_floors: BTreeMap<String, u32>,
    /// A quest's reward item (client template, the quest's `rewards`) -> the loot bag it drops at
    /// the player when the quest (or story step) is done.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quest_rewards: BTreeMap<String, QuestReward>,
    /// How a roll's loot power level changes what drops.
    #[serde(default)]
    pub power: LootPower,
}

/// What a quest's reward item drops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct QuestReward {
    #[schemars(extend("x-lmoxide-ref" = ["loot"]))]
    pub table: Ref,
    /// A fixed power level (its script passes a number, e.g. 0); none: the quest rule (the
    /// player's level x `power_per_level`, `quest_power_caps`, `quest_power_floors`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<u32>,
}

/// How the loot power level (player or NPC level x 100, capped by super-region; the client's
/// `gethashfromloot(table, power, ...)`) changes a roll. The live tables' scaling was server
/// data: every curve is (power, factor) points, linear between them and flat beyond the ends;
/// none: factor 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LootPower {
    /// Single stars (`stars` counts) times this, rounded (never below 1 when the entry drops any).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stars: Vec<[f64; 2]>,
    /// Chances of entries that drop minifigure parts times this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<[f64; 2]>,
    /// Chances of entries that drop minifigure bags times this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bags: Vec<[f64; 2]>,
    /// Entries with a `power_tier` above power + this are skipped; none: tiers don't limit drops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_window: Option<u32>,
}

impl LootPower {
    /// The factor of `curve` at `power`.
    pub fn factor(curve: &[[f64; 2]], power: u32) -> f64 {
        let p = f64::from(power);
        match curve {
            [] => 1.0,
            [only] => only[1],
            _ => {
                if p <= curve[0][0] {
                    return curve[0][1];
                }
                for w in curve.windows(2) {
                    let ([x0, y0], [x1, y1]) = (w[0], w[1]);
                    if p <= x1 {
                        return if x1 > x0 { y0 + (y1 - y0) * (p - x0) / (x1 - x0) } else { y1 };
                    }
                }
                curve[curve.len() - 1][1]
            }
        }
    }

    /// Whether an entry of `tier` drops at `power`.
    pub fn tier_allowed(&self, tier: Option<u32>, power: u32) -> bool {
        match (tier, self.tier_window) {
            (Some(t), Some(w)) => t <= power.saturating_add(w),
            _ => true,
        }
    }
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
    /// Super-region (world `super_region`) -> the level cap there; none: no cap.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub level_caps: BTreeMap<String, u32>,
}

/// A push away from a point (the client's AreaKnockaway: Radius, Distance, Speed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Knockaway {
    pub radius: f32,
    pub distance: f32,
    pub speed: f32,
    /// The client spell that does it (shown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spell: Option<u32>,
}

/// Potions, health orbs and level-ups (the server Lua shipped with the client).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Health {
    /// A Potion of Health heals this share of the maximum (script 464: `life * 0.8`).
    pub potion_heal: Ratio,
    /// Drinking one at or below this share of health adds `barely_alive_tag` ("Close Call!").
    pub barely_alive_below: Ratio,
    #[schemars(extend("x-lmoxide-client" = "lore"))]
    pub barely_alive_tag: u32,
    /// A level-up heals fully and knocks the enemies around away (script 990, spell 423601).
    pub level_up_knockaway: Knockaway,
}

/// Smashed minifigures (scripts 973 and 3804, "minifigure swapper smashed").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Smashed {
    /// Rebuild time by player level: the entry of the highest listed level at or below the
    /// player's.
    pub rebuild_by_level: BTreeMap<String, Duration>,
    /// The rebuild time of members (a subscribed account or UseMemberSmashedTimer) and in
    /// `free_super_regions`.
    pub free_rebuild: Duration,
    pub free_super_regions: Vec<u32>,
    /// Whether players count as members (LMOxide: everyone is).
    pub everyone_member: bool,
    /// The swap pushes the enemies around away (465935) ...
    pub knockaway: Knockaway,
    /// ... and the next minifigure can't be hurt for this long (465943, MitigationOverrideFlat 100).
    pub immunity: Duration,
    #[schemars(extend("x-lmoxide-client" = "nano"))]
    pub immunity_nano: u32,
}

/// Champions' element auras (script 3722 "champion aura random selector"): entering combat, a
/// champion casts its zone's element's aura (a random one where the zone has none).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Champions {
    /// Only NPCs with at least this PowerLevel (stat 499) ...
    pub aura_min_power_level: u32,
    /// ... taken as level x this.
    pub power_per_level: u32,
    /// The NPC is out of combat (the aura's pulses stop) this long after its last attack
    /// (tweak NPCBreakOOCTime).
    pub out_of_combat: Duration,
    /// Element (`light` ... `nature`) -> the aura (`skill/...`).
    #[schemars(extend("x-lmoxide-ref" = ["skill"]))]
    pub auras: BTreeMap<String, Ref>,
}

/// The story (story.rs; research/lmoxide-notes/client-survey/story.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Story {
    /// The narrated cinematics by name (a trigger's `cinematic` action names one): client effect
    /// packages with a CinematicState timeline, the narrator's subtitles TDB 50000. Live started
    /// them with an EffectPackageIIR on the player.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cinematics: BTreeMap<String, Cinematic>,
    /// A quest pickup quest's `dropped_by` defeat drops one of its pickups this often (not in the
    /// client).
    #[serde(default = "full")]
    pub pickup_drop_chance: Ratio,
    /// Every teleport window is open, whatever the story: the login journal lists
    /// `window_quests` as completed (the windows check CoreJournalHolder::HasEverCompletedQuest
    /// for their required quest). False: as live, a window opens once its quest is really done.
    #[serde(default = "yes")]
    pub windows_unlocked: bool,
    /// Main quest -> the quests the teleport windows (RDB 1000644 `required_quest_task_hash`) and
    /// the Lost Creations entry window require.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub window_quests: BTreeMap<String, Vec<u32>>,
    /// A dialogue's `spawn <hash>` that is no quest: the minifigure it gives, straight into the
    /// collection with the client's "You got a new minifigure" window
    /// (`[Give Swashbuckler]{spawn 7LLT 1 m}`; the other hashes are guesses by the texts' words,
    /// docs/TUNABLES.md "Story").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(extend("x-lmoxide-ref" = ["minifig"]))]
    pub spawn_minifigures: BTreeMap<String, String>,
    /// A pocket adventure whose minifigure a conversation gives (the Monkey Cave's Island
    /// Warrior rescue) gives it again on later clears, as a bag, once that conversation has
    /// played (it plays once). False (the user's choice, 2026-10-29): only the conversation does.
    #[serde(default)]
    pub repeat_conversation_minifigures: bool,
}

fn full() -> Ratio {
    Ratio(1.0)
}

fn yes() -> bool {
    true
}

impl Default for Story {
    fn default() -> Self {
        Story { cinematics: BTreeMap::new(), pickup_drop_chance: full(), windows_unlocked: true, window_quests: BTreeMap::new(), spawn_minifigures: BTreeMap::new(), repeat_conversation_minifigures: false }
    }
}

/// One narrated cinematic, played once per character.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cinematic {
    /// The client effect package (RDB 1000622).
    #[schemars(extend("x-lmoxide-client" = "effect_package"))]
    pub effect_package: u32,
    /// Its narration (TDB 50000 instance).
    pub text: u32,
    /// How long it runs.
    pub length: Duration,
    /// The lore flag ("Cinematic Flags") it sets, which also marks it seen; none: the server
    /// keeps that itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<u32>,
    /// Plays only for a player with all these tags ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<u32>,
    /// ... and none of these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unless: Vec<u32>,
    /// The dialogue the server starts when it ends (`dialogue/...`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-lmoxide-ref" = ["dialogue"]))]
    pub then_dialogue: Option<Ref>,
    /// Where it plays and how sure that is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

/// Zone elements (the playfield controllers 464871 / 517564 / 517102 and DoElementalAffinity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Elements {
    /// A minifigure whose element isn't the zone's gets this buff (the controller's Wrong
    /// Element; which controller each zone used is unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-lmoxide-ref" = ["buff"]))]
    pub wrong_element: Option<Ref>,
    /// Elemental Affinity (an upgrade with the stat `elemental_affinity`) in a zone of the
    /// minifigure's element: its role (`striker`, `defender`, `builder`) -> buff.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(extend("x-lmoxide-ref" = ["buff"]))]
    pub affinity: BTreeMap<String, Ref>,
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
    /// The quest (hash) the entry window requires, "Test your mettle inside the Gauntlet" (the
    /// client checks the journal: story `window_quests`).
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
    /// How far (m) the second and later enemies of a spawn's `count` land from its point, when
    /// the point gives no `spread` (live Monster Laboratory: 1.0-1.6 m).
    pub spawn_spread: f32,
    /// Before a level's wave spawns, each point it uses shows `wave_particle` this long (live
    /// Monster Laboratory: 2.0 s, then the enemies; the only pause between waves).
    pub wave_telegraph: Duration,
    /// The pads' spawn particle (live: 542337, blue).
    pub wave_particle: u32,
    /// The Gatekeeper defeated, the enemies its skills summoned go too (unknown on live; the
    /// user's guess: they stay and fight on).
    pub gatekeeper_adds_vanish: bool,
    /// Which of a player's minifigures a completed level counts for (the levels 5, 10 and 15
    /// achievements).
    #[serde(default)]
    pub achievement_minifigs: AchievementMinifigs,
    /// For testing: every level is this one (`lc/...`), the Gatekeeper's too (unset: any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-lmoxide-ref" = ["lc"]))]
    pub force_level: Option<Ref>,
}

/// Who a completed Lost Creations level counts for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum AchievementMinifigs {
    /// The equipped minifigures not smashed in the run (user's rule).
    #[default]
    Unsmashed,
    /// Every equipped minifigure.
    Team,
    /// Only the active one.
    Active,
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
