//! The minifigure collection: every minifigure is owned and fully upgraded.
//!
//! The client's Collection screen needs two inventories (`asc::ASMinifigureList`): the team,
//! inventory 101 (positions 6, 8 and 20 are the three team slots), and the collection,
//! inventory 110001 (storage). Each minifigure item carries its level (item stat 3000058, max 10),
//! XP (52) and upgrade bitmasks (3000098 active, 3000106 purchased). Bit `n` is the minifigure's
//! upgrade ability at `ability_list` position `n` (data/minifigure_abilities.json); bits 0..11
//! form 8 tiers, four of them choose-one pairs (1,2), (4,5), (7,8), (10,11).

use std::{collections::HashMap, sync::LazyLock};

use lmo_net::proto::{
    ProtoId32, ProtoInventoryGame, ProtoInventoryGameEntry,
    proto_inventory_game::InventoryListEntry,
    proto_inventory_game_entry::StatValue,
};

use crate::game::acg_item;

pub const TEAM_INVENTORY: u32 = 101;
pub const COLLECTION_INVENTORY: u32 = 110_001;
const COLLECTION_SIZE: u32 = 256;

const STAT_MINIFIGURE_LEVEL: u32 = 3_000_058;
const STAT_XP: u32 = 52;
pub const STAT_ACTIVE_UPGRADES: u32 = 3_000_098;
const STAT_PURCHASED_UPGRADES: u32 = 3_000_106;
/// The client levels minifigures up to 10 (CanLevelUpMinifigure); XP for level 10.
const MAX_LEVEL: u32 = 10;
const MAX_XP: u32 = 3000;
/// Every upgrade purchased.
const ALL_PURCHASED: u32 = 0x1fff;
/// One of each choose-one pair active: bits 0, 1, 3, 4, 6, 7, 9, 10.
pub const DEFAULT_ACTIVE: u32 = 0x6db;
/// Upgrade bits that exclude each other.
const PAIRS: [(u32, u32); 4] = [(1, 2), (4, 5), (7, 8), (10, 11)];

static SPELLS: LazyLock<HashMap<String, Vec<u32>>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../data/minifigure_spells.json")).expect("minifigure_spells.json"));
static ABILITIES: LazyLock<HashMap<String, Vec<(u32, u32, i32)>>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../data/minifigure_abilities.json")).expect("minifigure_abilities.json")
});

/// Every minifigure item template, in order.
pub fn all_minifigures() -> Vec<u32> {
    let mut all: Vec<u32> = SPELLS.keys().filter_map(|k| k.parse().ok()).collect();
    all.sort_unstable();
    all
}

/// Left- and right-click (normal and special) spells of a minifigure.
pub fn spells(template: u32) -> Vec<u32> {
    SPELLS.get(&template.to_string()).cloned().unwrap_or_default()
}

/// Everything a minifigure can cast or apply, to learn before it is used: its normal and
/// special spells and its upgrade abilities (some replace the normal or special spell).
pub fn spells_to_learn(template: u32) -> Vec<u32> {
    let mut all = spells(template);
    if let Some(list) = ABILITIES.get(&template.to_string()) {
        all.extend(list.iter().map(|&(_, ability, _)| ability));
    }
    all
}

/// Activates upgrade `position` in `active`, deactivating its pair partner.
pub fn activate(active: u32, position: u32) -> u32 {
    let mut active = active | (1 << position);
    for (a, b) in PAIRS {
        if position == a {
            active &= !(1 << b);
        } else if position == b {
            active &= !(1 << a);
        }
    }
    active
}

/// The owned minifigures outside the team (inventory 110001), by position. Positions change as
/// minifigures move in and out of the team during a session.
pub struct Collection {
    pub slots: Vec<Option<u32>>,
}

impl Collection {
    pub fn new(team: &[(u32, u32)]) -> Self {
        let slots = all_minifigures().into_iter().filter(|t| !team.iter().any(|&(_, m)| m == *t)).map(Some).collect();
        Self { slots }
    }

    pub fn take(&mut self, pos: u32) -> Option<u32> {
        self.slots.get_mut(pos as usize)?.take()
    }

    pub fn put(&mut self, pos: u32, template: u32) {
        let pos = pos as usize;
        if self.slots.len() <= pos {
            self.slots.resize(pos + 1, None);
        }
        self.slots[pos] = Some(template);
    }

    pub fn position_of(&self, template: u32) -> Option<u32> {
        self.slots.iter().position(|s| *s == Some(template)).map(|p| p as u32)
    }
}

/// A minifigure item at max level with all upgrades purchased and `active` selected.
fn entry(template: u32, active: u32, id_salt: u64) -> ProtoInventoryGameEntry {
    let stat = |stat, value| StatValue { stat: Some(stat), value: Some(value) };
    ProtoInventoryGameEntry {
        item_id: Some(u64::from(template) | id_salt),
        item_data: Some(acg_item(template, MAX_LEVEL)),
        stat_list: vec![
            stat(STAT_MINIFIGURE_LEVEL, MAX_LEVEL),
            stat(STAT_XP, MAX_XP),
            stat(STAT_ACTIVE_UPGRADES, active),
            stat(STAT_PURCHASED_UPGRADES, ALL_PURCHASED),
        ],
        ..Default::default()
    }
}

/// The collection (110001) and team (101) inventories for FullPlayerCharacterIIR, in that order:
/// the client builds and caches its minifigure list (ASMinifigureList::GetMinifigureList) as soon
/// as the team is equipped, so the collection must already be filled or every other minifigure
/// stays locked.
pub fn inventories(
    character: u32,
    team: &[(u32, u32)],
    collection: &Collection,
    upgrades: &impl Fn(u32) -> u32,
) -> Vec<ProtoInventoryGame> {
    let inventory = |kind, max_size, items: Vec<(u32, u32)>| ProtoInventoryGame {
        inventory_id: ProtoId32 { r#type: Some(kind), instance: Some(character) },
        max_size: Some(max_size),
        disabled_slots_count: None,
        items: items.into_iter().map(|(pos, t)| InventoryListEntry { pos, item: entry(t, upgrades(t), 0) }).collect(),
    };
    let stored = collection.slots.iter().enumerate().filter_map(|(p, t)| t.map(|t| (p as u32, t))).collect();
    vec![inventory(COLLECTION_INVENTORY, COLLECTION_SIZE, stored), inventory(TEAM_INVENTORY, 64, team.to_vec())]
}

/// The collection again, for an `UpdateInventoryIIR_t` after the character is in play. The
/// item ids differ from the first copy, so the client replaces every entry and signals each
/// as added, which unlocks it in an already built minifigure list (an identical copy is
/// skipped).
pub fn collection_refresh(character: u32, collection: &Collection, upgrades: &impl Fn(u32) -> u32) -> ProtoInventoryGame {
    ProtoInventoryGame {
        inventory_id: ProtoId32 { r#type: Some(COLLECTION_INVENTORY), instance: Some(character) },
        max_size: Some(COLLECTION_SIZE),
        disabled_slots_count: None,
        items: collection
            .slots
            .iter()
            .enumerate()
            .filter_map(|(p, t)| t.map(|t| InventoryListEntry { pos: p as u32, item: entry(t, upgrades(t), 1 << 32) }))
            .collect(),
    }
}
