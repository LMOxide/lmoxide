//! Record ids, typed: the kind, the world for records kept in a world's folder, and the slug.
//! Written as text everywhere (`spawn/4200/scallywag-7`, `npc/pirate-specter`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Every kind of record (the first part of an id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecordKind {
    World,
    Spawn,
    Portal,
    Bouncer,
    Area,
    Trigger,
    Puzzle,
    Script,
    Region,
    Npc,
    Faction,
    Object,
    Skill,
    Buff,
    Loot,
    Item,
    Minifig,
    Dialogue,
    Achievement,
    Quest,
}

impl RecordKind {
    pub const ALL: [RecordKind; 20] = [
        RecordKind::World,
        RecordKind::Spawn,
        RecordKind::Portal,
        RecordKind::Bouncer,
        RecordKind::Area,
        RecordKind::Trigger,
        RecordKind::Puzzle,
        RecordKind::Script,
        RecordKind::Region,
        RecordKind::Npc,
        RecordKind::Faction,
        RecordKind::Object,
        RecordKind::Skill,
        RecordKind::Buff,
        RecordKind::Loot,
        RecordKind::Item,
        RecordKind::Minifig,
        RecordKind::Dialogue,
        RecordKind::Achievement,
        RecordKind::Quest,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RecordKind::World => "world",
            RecordKind::Spawn => "spawn",
            RecordKind::Portal => "portal",
            RecordKind::Bouncer => "bouncer",
            RecordKind::Area => "area",
            RecordKind::Trigger => "trigger",
            RecordKind::Puzzle => "puzzle",
            RecordKind::Script => "script",
            RecordKind::Region => "region",
            RecordKind::Npc => "npc",
            RecordKind::Faction => "faction",
            RecordKind::Object => "object",
            RecordKind::Skill => "skill",
            RecordKind::Buff => "buff",
            RecordKind::Loot => "loot",
            RecordKind::Item => "item",
            RecordKind::Minifig => "minifig",
            RecordKind::Dialogue => "dialogue",
            RecordKind::Achievement => "achievement",
            RecordKind::Quest => "quest",
        }
    }

    /// Kept in a world's folder, with the playfield in its id (`spawn/4200/...`). Worlds
    /// themselves are in their folder but named by slug only.
    pub fn per_world(self) -> bool {
        matches!(
            self,
            RecordKind::Spawn
                | RecordKind::Portal
                | RecordKind::Bouncer
                | RecordKind::Area
                | RecordKind::Trigger
                | RecordKind::Puzzle
                | RecordKind::Script
        )
    }
}

impl FromStr for RecordKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        RecordKind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| format!("no kind of record {s:?}"))
    }
}

impl fmt::Display for RecordKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A record's id: `<kind>/<slug>`, or `<kind>/<playfield>/<slug>` for world records.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordId {
    pub kind: RecordKind,
    /// The playfield of a record kept in a world's folder.
    pub world: Option<u32>,
    pub slug: String,
}

impl RecordId {
    pub fn new(kind: RecordKind, world: Option<u32>, slug: impl Into<String>) -> Self {
        RecordId {
            kind,
            world,
            slug: slug.into(),
        }
    }

    /// The smallest id of a kind (for ranges over a sorted map).
    pub fn first_of(kind: RecordKind) -> Self {
        RecordId {
            kind,
            world: None,
            slug: String::new(),
        }
    }
}

impl FromStr for RecordId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (kind, rest) = s
            .split_once('/')
            .ok_or_else(|| format!("not a record id: {s:?}"))?;
        let kind: RecordKind = kind.parse()?;
        let bad = || format!("not a {kind} id: {s:?}");
        if rest.is_empty() || rest.contains(' ') {
            return Err(bad());
        }
        if kind.per_world() {
            let (pf, slug) = rest.split_once('/').ok_or_else(bad)?;
            let pf: u32 = pf.parse().map_err(|_| bad())?;
            if slug.is_empty() {
                return Err(bad());
            }
            Ok(RecordId {
                kind,
                world: Some(pf),
                slug: slug.to_string(),
            })
        } else {
            Ok(RecordId {
                kind,
                world: None,
                slug: rest.to_string(),
            })
        }
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.world {
            Some(pf) => write!(f, "{}/{pf}/{}", self.kind, self.slug),
            None => write!(f, "{}/{}", self.kind, self.slug),
        }
    }
}

impl Serialize for RecordId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RecordId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl PartialEq<str> for RecordId {
    fn eq(&self, other: &str) -> bool {
        other.parse::<RecordId>().is_ok_and(|o| o == *self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_read_and_write() {
        let a: RecordId = "spawn/4200/scallywag-7".parse().unwrap();
        assert_eq!(
            a,
            RecordId::new(RecordKind::Spawn, Some(4200), "scallywag-7")
        );
        assert_eq!(a.to_string(), "spawn/4200/scallywag-7");
        let b: RecordId = "npc/pirate-specter".parse().unwrap();
        assert_eq!(
            (b.kind, b.world, b.slug.as_str()),
            (RecordKind::Npc, None, "pirate-specter")
        );
        assert_eq!(
            "world/isle-of-yarr".parse::<RecordId>().unwrap().world,
            None
        );
        for bad in [
            "spawn/x/y",
            "spawn/4200",
            "npcs/x",
            "npc/",
            "Pirate World",
            "npc/a b",
        ] {
            assert!(bad.parse::<RecordId>().is_err(), "{bad}");
        }
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            "\"spawn/4200/scallywag-7\""
        );
        assert_eq!(
            serde_json::from_str::<RecordId>("\"npc/x\"").unwrap().kind,
            RecordKind::Npc
        );
    }

    #[test]
    fn every_id_in_the_tree_parses() {
        // the kinds agree with KINDS
        for (k, _) in crate::KINDS {
            assert_eq!(k.parse::<RecordKind>().unwrap().as_str(), *k);
        }
    }
}
