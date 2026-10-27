//! What every record carries (id, inheritance, feature, provenance, notes, revision) and the
//! small types shared by the templates.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A position in a playfield (m).
pub type Vec3 = [f32; 3];

/// A reference to another record by its id (`npc/pirate-specter`). Checked when the content
/// loads: it must name a record of a kind that fits.
pub type Ref = String;

/// Which way something faces: a heading (radians about the up axis) or a quaternion
/// `[x, y, z, w]` as the client stores it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Rot {
    Heading(f32),
    Quat([f32; 4]),
}

impl Default for Rot {
    fn default() -> Self {
        Rot::Heading(0.0)
    }
}

impl Rot {
    /// The heading about the up axis.
    pub fn heading(self) -> f32 {
        match self {
            Rot::Heading(h) => h,
            Rot::Quat([_, y, _, w]) => 2.0 * y.atan2(w),
        }
    }
}

/// Where a value came from, so guesses stay visible next to what live did.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Extracted from the game client's data.
    Client,
    /// Seen in a capture of the live server (LMO_Captures.cap).
    Live,
    /// Seen in the recorded 2015 demo session.
    Demo,
    /// Taken from video footage of live.
    Footage,
    /// Values the server used before they were moved into content (docs/TUNABLES.md).
    Server,
    /// Proposed by a tool from client data (navmesh, names, ...), not seen live.
    Proposed,
    /// Placed or confirmed by someone in the world editor.
    Surveyed,
    /// A plain guess.
    Guess,
}

/// A client game action as the client's data writes it (`type` and its arguments). Numbers in
/// `args` are client ids (nanos, items, effect packages).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GameAction {
    #[serde(rename = "type")]
    pub kind: String,
    /// Run by the client itself (the server doesn't need to).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub client: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, serde_json::Value>,
    /// When it runs: the client's criteria, `[list, value, operator]` each (e.g. `[0, 448001,
    /// 101]`: the caster has upgrade 448001; 91: hasn't).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<serde_json::Value>,
}

/// A point with a facing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub pos: Vec3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rot: Option<Rot>,
}

pub fn live() -> String {
    "live".into()
}

/// `live`, or not set (a record made with `Default` has an empty feature: it is `live`).
pub fn is_live(f: &String) -> bool {
    f.is_empty() || f == "live"
}

pub fn is_zero(n: &u64) -> bool {
    *n == 0
}

pub fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == T::default()
}

pub fn is_false(b: &bool) -> bool {
    !*b
}

pub fn is_one(n: &f32) -> bool {
    *n == 1.0
}

pub fn one() -> f32 {
    1.0
}

/// What every record shares, whatever its kind.
pub trait Record {
    const PREFIX: &'static str;
    fn id(&self) -> &str;
    fn feature(&self) -> &str;
}

/// Declares a record: the struct with the shared fields around its own (`id`, `extends`,
/// `name` first; `feature`, `source`, `tags`, `comment`, `rev`, `author`, `updated` last).
macro_rules! record {
    (
        $(#[$meta:meta])*
        pub struct $name:ident: $prefix:literal {
            $( $(#[$fmeta:meta])* pub $field:ident : $ty:ty, )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            #[doc = concat!("Its id: `", $prefix, "/<slug>`. Stable: other records refer to it.")]
            pub id: String,
            /// Another record of the same kind this one starts from: its fields, then this one's on top.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            #[schemars(extend("x-lmoxide-ref" = [$prefix]))]
            pub extends: Option<$crate::record::Ref>,
            #[serde(default, skip_serializing_if = "String::is_empty")]
            pub name: String,
            $( $(#[$fmeta])* pub $field: $ty, )*
            /// The feature it belongs to: `live` (recreating live), `feature/...` or `other/...`.
            /// A record with the id of one in a parent feature changes it while this feature is on.
            #[serde(default = "crate::record::live", skip_serializing_if = "crate::record::is_live")]
            pub feature: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub source: Option<$crate::record::Source>,
            /// Labels for sorting work out ("approximate", "todo", "verified", ...).
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            pub tags: Vec<String>,
            #[serde(default, skip_serializing_if = "String::is_empty")]
            pub comment: String,
            /// Revision, raised by every saved edit (for merging concurrent edits).
            #[serde(default, skip_serializing_if = "crate::record::is_zero")]
            pub rev: u64,
            #[serde(default, skip_serializing_if = "String::is_empty")]
            pub author: String,
            /// Unix time of the last edit.
            #[serde(default, skip_serializing_if = "crate::record::is_zero")]
            pub updated: u64,
        }

        impl $crate::record::Record for $name {
            const PREFIX: &'static str = $prefix;
            fn id(&self) -> &str {
                &self.id
            }
            fn feature(&self) -> &str {
                &self.feature
            }
        }

    };
}

pub(crate) use record;
