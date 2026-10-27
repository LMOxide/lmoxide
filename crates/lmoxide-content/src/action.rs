//! The one action vocabulary: triggers, script waves, skill hits, buff procs, defeats, builds,
//! levers and portals all run lists of these.
//!
//! Written as `{ "<action>": { ...arguments } }`, e.g. `{ "apply_buff": { "buff": "buff/burning",
//! "amount": "20%", "duration": "3s" } }`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::record::{Ref, Vec3};
use crate::units::{Duration, Ratio};

/// Who or what an action acts on. Default: whoever the action's owner aims at (the skill's
/// target, the player who set the trigger off, the defeated enemy ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// The action's own target (default).
    #[default]
    Target,
    /// Whoever owns the action (the caster, the buff's holder, the trigger, the object).
    #[serde(rename = "self")]
    Owner,
    /// The player who set it off (triggers, scripts).
    TriggerPlayer,
    /// The player's team on the same playfield.
    Team,
    /// Everyone in the area of the trigger or script.
    AllInArea,
    /// One placement or object by id (`spawn/4200/gate-north`).
    Placement(#[schemars(extend("x-lmoxide-ref" = ["spawn", "bouncer", "trigger"]))] Ref),
    /// Every placement with this tag.
    Tag(String),
}

fn is_default_target(t: &Target) -> bool {
    *t == Target::Target
}

/// Where spawned things appear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpawnAt {
    #[default]
    OnSelf,
    OnTarget,
    AroundSelf,
    AroundTarget,
    /// A random point in an area (`area/...`).
    InArea(#[schemars(extend("x-lmoxide-ref" = ["area"]))] Ref),
    At(#[schemars(extend("x-lmoxide-position" = true))] Vec3),
}

/// Where a portal, trigger or teleport takes a player.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    /// The world (`world/...`); none: the same world.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-lmoxide-ref" = ["world"]))]
    pub world: Option<Ref>,
    /// A named arrival point of that world (`arrival_from` key or respawn point name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<String>,
    /// An exact position (instead of a point).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-lmoxide-position" = true))]
    pub pos: Option<Vec3>,
    /// A server command instead of a place, e.g. `lost_creations.advance` (the server lists its
    /// commands and their arguments).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, serde_json::Value>,
    /// Ask first ("Do you want to travel to X?").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Spawn {
        /// `npc/...` or `object/...`.
        #[schemars(extend("x-lmoxide-ref" = ["npc", "object"]))]
        template: Ref,
        #[serde(default)]
        at: SpawnAt,
        #[serde(default = "one_u32")]
        count: u32,
        /// Spread around the spawn point (m).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f32>,
        /// Gone after this long.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lifetime: Option<Duration>,
        /// Partial template fields for these spawns.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        overrides: Option<serde_json::Value>,
    },
    Despawn {
        #[serde(default, skip_serializing_if = "is_default_target")]
        target: Target,
    },
    Damage {
        /// Share of the attacker's power (the client's AbilityMod).
        #[serde(default, rename = "mod", skip_serializing_if = "Option::is_none")]
        ability_mod: Option<f32>,
        /// A fixed amount instead.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amount: Option<f32>,
        /// Damage type: `normal`, `fire`, ... or a client damage type number.
        #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
        damage_type: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f32>,
        #[serde(default, skip_serializing_if = "is_default_target")]
        target: Target,
    },
    Heal {
        /// Share of maximum health.
        amount: Ratio,
        #[serde(default, skip_serializing_if = "is_default_target")]
        target: Target,
    },
    ApplyBuff {
        #[schemars(extend("x-lmoxide-ref" = ["buff"]))]
        buff: Ref,
        /// Overrides the buff's own `amount`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amount: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stacks: Option<u32>,
        #[serde(default, skip_serializing_if = "is_default_target")]
        target: Target,
    },
    RemoveBuff {
        #[schemars(extend("x-lmoxide-ref" = ["buff"]))]
        buff: Ref,
        #[serde(default, skip_serializing_if = "is_default_target")]
        target: Target,
    },
    Knockback {
        distance: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f32>,
        #[serde(default, skip_serializing_if = "is_default_target")]
        target: Target,
    },
    /// Shows a client effect package.
    Effect {
        #[schemars(extend("x-lmoxide-client" = "effect_package"))]
        effect_package: u32,
        #[serde(default, skip_serializing_if = "is_default_target")]
        target: Target,
    },
    Teleport {
        to: Destination,
    },
    /// Opens a client teleport window.
    OpenWindow {
        #[schemars(extend("x-lmoxide-client" = "window"))]
        window: u32,
    },
    Dialogue {
        #[schemars(extend("x-lmoxide-ref" = ["dialogue"]))]
        dialogue: Ref,
    },
    Message {
        text: String,
    },
    Music {
        #[schemars(extend("x-lmoxide-client" = "music"))]
        track: u32,
    },
    /// Changes an object's state (doors and gates `open`/`closed`, levers, buildies `built`).
    SetState {
        target: Target,
        state: String,
    },
    StartScript {
        #[schemars(extend("x-lmoxide-ref" = ["script"]))]
        script: Ref,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wave: Option<String>,
    },
    EndScript {
        #[schemars(extend("x-lmoxide-ref" = ["script"]))]
        script: Ref,
    },
    /// Counts a location as found (an exploration area's name), for its achievements.
    Discover {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        location: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(extend("x-lmoxide-ref" = ["achievement"]))]
        achievement: Option<Ref>,
    },
    GrantAchievement {
        #[schemars(extend("x-lmoxide-ref" = ["achievement"]))]
        achievement: Ref,
    },
    /// One collectible: a collect achievement (`achievement/...`) or a `drop:<name>` key.
    Collect {
        key: String,
    },
    /// Rolls a loot table where the owner is.
    Drop {
        #[schemars(extend("x-lmoxide-ref" = ["loot"]))]
        loot: Ref,
    },
    GiveItem {
        #[schemars(extend("x-lmoxide-ref" = ["item"]))]
        item: Ref,
        #[serde(default = "one_u32")]
        count: u32,
    },
    Cinematic {
        name: String,
    },
    /// A server command with arguments (the server lists what it knows).
    Command {
        name: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        args: BTreeMap<String, serde_json::Value>,
    },
    /// Not known yet: what it should do goes in `comment`.
    Other {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        comment: String,
    },
}

fn one_u32() -> u32 {
    1
}
