//! The minifigure collection: every minifigure is owned; fully ranked and upgraded unless
//! `--progression` (progression.rs) is on.
//!
//! The client's Collection screen needs two inventories (`asc::ASMinifigureList`): the team,
//! inventory 101 (positions 6, 8 and 20 are the three team slots), and the collection,
//! inventory 110001 (storage). Each minifigure item carries its level (item stat 3000058, max 10),
//! XP (52) and upgrade bitmasks (3000098 active, 3000106 purchased). Bit `n` is the minifigure's
//! upgrade ability at `ability_list` position `n` (data/minifigure_abilities.json); bits 0..11
//! form 8 tiers, four of them choose-one pairs (1,2), (4,5), (7,8), (10,11).

use std::{collections::HashMap, sync::LazyLock};

use lmoxide_net::proto::{
    ProtoId32, ProtoInventoryGame, ProtoInventoryGameEntry,
    proto_inventory_game::InventoryListEntry,
    proto_inventory_game_entry::StatValue,
};

use crate::game::acg_item;

pub const TEAM_INVENTORY: u32 = 101;
pub const COLLECTION_INVENTORY: u32 = 110_001;
/// The backpack: the default inventory for items (ItemHolder+0x50, InventoryGame_c 104).
pub const BACKPACK_INVENTORY: u32 = 104;
const COLLECTION_SIZE: u32 = 256;

pub const STAT_MINIFIGURE_LEVEL: u32 = 3_000_058;
/// MinifigureXP: the stars put into the minifigure (cumulative MinifigureXPTables thresholds).
pub const STAT_MINIFIGURE_XP: u32 = 3_000_057;
pub const STAT_ACTIVE_UPGRADES: u32 = 3_000_098;
pub const STAT_PURCHASED_UPGRADES: u32 = 3_000_106;
const STAT_MINIFIGURE_LIFE: u32 = 3_000_091;
const STAT_MINIFIGURE_HEALTH: u32 = 3_000_092;
/// Item level sent without progression (ACGItem level; the Collection shows 8 ranks).
const MAX_LEVEL: u32 = 10;
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
/// special spells, their combo steps (the recorded session taught 317739, 317740, 317741), its
/// upgrade abilities (some replace the normal or special spell) and the spells its buffs put on
/// the shortcut bar (combo steps, mode switches: `actions::override_targets`).
pub fn spells_to_learn(template: u32) -> Vec<u32> {
    let mut all = spells(template);
    // (castable steps have an attack time; the hits' nanos don't)
    all.extend(crate::combat::combo_steps(template).into_iter().filter(|s| crate::actions::hit_delay(*s).is_some()));
    if let Some(list) = ABILITIES.get(&template.to_string()) {
        all.extend(list.iter().map(|&(_, ability, _)| ability));
    }
    all.extend(crate::actions::override_targets(&all));
    let mut seen = std::collections::HashSet::new();
    all.retain(|s| seen.insert(*s));
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

/// A minifigure's item template by its name (case-insensitive, data/minifig_parts.json).
pub fn template_by_name(name: &str) -> Option<u32> {
    #[derive(serde::Deserialize)]
    struct Parts {
        minifigures: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        minifigure_template_id: u32,
        name: Option<String>,
    }
    static NAMES: LazyLock<Vec<(String, u32)>> = LazyLock::new(|| {
        let parts: Parts = serde_json::from_str(include_str!("../../../data/minifig_parts.json")).expect("minifig_parts.json");
        parts.minifigures.into_iter().filter_map(|m| Some((m.name?.to_ascii_lowercase(), m.minifigure_template_id))).collect()
    });
    let wanted = name.to_ascii_lowercase();
    NAMES.iter().find(|(n, _)| *n == wanted).map(|&(_, t)| t)
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

/// A minifigure item of `rank` (its tiers purchased) with `active` upgrades selected: `life` =
/// (its own life, its full health). The client adds level x HealthPerLevel to its own life
/// (SimpleChar_t::CalculateLife), so the full health counted it already.
fn entry(template: u32, active: u32, (rank, purchased): (u32, u32), life: (u32, u32), id_salt: u64) -> ProtoInventoryGameEntry {
    let stat = |stat, value| StatValue { stat: Some(stat), value: Some(value) };
    let level = if crate::progression::enabled() { rank } else { MAX_LEVEL };
    ProtoInventoryGameEntry {
        item_id: Some(u64::from(template) | id_salt),
        item_data: Some(acg_item(template, level.max(1))),
        stat_list: vec![
            stat(STAT_MINIFIGURE_LEVEL, level),
            stat(STAT_MINIFIGURE_XP, crate::progression::rank_xp(rank).unwrap_or(0) as u32),
            stat(STAT_ACTIVE_UPGRADES, active),
            stat(STAT_PURCHASED_UPGRADES, purchased),
            // the client takes a minifigure's health from these when it switches to it (also
            // on its own after a minifigure is smashed): unset, they read 0, so each switch
            // flashed low health and a smashed minifigure's teammates were smashed in turn
            stat(STAT_MINIFIGURE_LIFE, life.0),
            stat(STAT_MINIFIGURE_HEALTH, life.1),
        ],
        ..Default::default()
    }
}

/// The character's inventories for FullPlayerCharacterIIR, collection (110001) and team (101) first:
/// the client builds and caches its minifigure list (ASMinifigureList::GetMinifigureList) as soon
/// as the team is equipped, so the collection must already be filled or every other minifigure
/// stays locked.
pub fn inventories(
    character: u32,
    team: &[(u32, u32)],
    collection: &Collection,
    upgrades: &impl Fn(u32) -> u32,
    ranks: &impl Fn(u32) -> (u32, u32),
    lives: &impl Fn(u32) -> (u32, u32),
) -> Vec<ProtoInventoryGame> {
    let inventory = |kind, max_size, items: Vec<(u32, u32)>| ProtoInventoryGame {
        inventory_id: ProtoId32 { r#type: Some(kind), instance: Some(character) },
        max_size: Some(max_size),
        disabled_slots_count: None,
        items: items.into_iter().map(|(pos, t)| InventoryListEntry { pos, item: entry(t, upgrades(t), ranks(t), lives(t), 0) }).collect(),
    };
    let stored = collection.slots.iter().enumerate().filter_map(|(p, t)| t.map(|t| (p as u32, t))).collect();
    // The rest as live sent them (empty, live sizes). The client refuses to loot an item when
    // the inventory it would go to (ItemHolder::GetItemTypeInventory) has no free slot:
    // minifigure bags go to 110002, minifigure parts and other items to the backpack 104.
    vec![
        inventory(COLLECTION_INVENTORY, COLLECTION_SIZE, stored),
        inventory(TEAM_INVENTORY, 64, team.to_vec()),
        inventory(BACKPACK_INVENTORY, 50, vec![]),
        inventory(112, 48, vec![]),
        inventory(113, 48, vec![]),
        inventory(120, 20, vec![]),
        inventory(119, 24, vec![]),
        inventory(crate::game::MINIFIGURE_BAG_INVENTORY, 200, vec![]),
    ]
}

/// The team again, like `collection_refresh` (new item ids so the client replaces every entry):
/// after a test setup changed the team (`!test`), the HUD keeps the old one otherwise.
pub fn team_refresh(
    character: u32,
    team: &[(u32, u32)],
    upgrades: &impl Fn(u32) -> u32,
    ranks: &impl Fn(u32) -> (u32, u32),
    lives: &impl Fn(u32) -> (u32, u32),
    salt: u64,
) -> ProtoInventoryGame {
    ProtoInventoryGame {
        inventory_id: ProtoId32 { r#type: Some(TEAM_INVENTORY), instance: Some(character) },
        max_size: Some(64),
        disabled_slots_count: None,
        items: team.iter().map(|&(pos, t)| InventoryListEntry { pos, item: entry(t, upgrades(t), ranks(t), lives(t), salt) }).collect(),
    }
}

/// The collection again, for an `UpdateInventoryIIR_t` after the character is in play. The
/// item ids differ from the first copy, so the client replaces every entry and signals each
/// as added, which unlocks it in an already built minifigure list (an identical copy is
/// skipped).
pub fn collection_refresh(
    character: u32,
    collection: &Collection,
    upgrades: &impl Fn(u32) -> u32,
    ranks: &impl Fn(u32) -> (u32, u32),
    lives: &impl Fn(u32) -> (u32, u32),
) -> ProtoInventoryGame {
    ProtoInventoryGame {
        inventory_id: ProtoId32 { r#type: Some(COLLECTION_INVENTORY), instance: Some(character) },
        max_size: Some(COLLECTION_SIZE),
        disabled_slots_count: None,
        items: collection
            .slots
            .iter()
            .enumerate()
            .filter_map(|(p, t)| t.map(|t| InventoryListEntry { pos: p as u32, item: entry(t, upgrades(t), ranks(t), lives(t), 1 << 32) }))
            .collect(),
    }
}
