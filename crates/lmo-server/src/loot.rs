//! Loot bags: what defeated enemies and smashed objects drop.
//!
//! A bag is a dynel of type 51322 (ProjectLootBag) created with UpdateMessage op 0 and
//! `LootBagFullUpdateIIR_c`. When its owner's client sees it, it splits the items into local
//! ClientLootBags that burst out and land, auto-collects them when the player walks close, and
//! sends `GameCharAgent::TryLootItem(bag, item position, owner, 0)` per item. The client never
//! adds the reward itself: the server sends it (token update or health) and confirms the pickup
//! with `LootRemoveItemsIIR_c`.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmo_net::{
    iir,
    proto::{
        ProtoGameCodeDynelIir, ProtoId32, ProtoInventoryGame, ProtoInventoryGameEntry, ProtoLootBagIir, ProtoQuat,
        ProtoSimpleDynelIir, ProtoVec3, proto_inventory_game::InventoryListEntry,
    },
    types::Id32,
};
use tracing::info;

use crate::{
    game::{acg_item, create_game_object, despawn_record},
    world,
};

const LOOT_BAG_TYPE: u32 = 51322;
/// "XX_Minifigure XP": one star (GameAction token 100, amount 1).
pub const STAR_ITEM: u32 = 20634;
/// "Health Orb": heals the active minifigure.
pub const HEALTH_ORB_ITEM: u32 = 20908;
/// Bags nobody empties disappear after this long.
const BAG_LIFETIME: Duration = Duration::from_secs(60);

struct Bag {
    owner: Id32,
    /// Item template per position; `None` once looted.
    items: Vec<Option<u32>>,
}

static BAGS: LazyLock<Mutex<HashMap<Id32, Bag>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_BAG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Drops `items` (templates) for `owner` at `pos`, sent to the owner only.
pub fn drop_items(owner: Id32, playfield: u32, pos: [f32; 3], dropper: Id32, items: Vec<u32>) {
    if items.is_empty() {
        return;
    }
    let id = Id32::new(LOOT_BAG_TYPE, NEXT_BAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let records = vec![create_game_object(id, playfield, pos), bag_record(id, playfield, pos, owner, dropper, &items)];
    BAGS.lock().unwrap().insert(id, Bag { owner, items: items.into_iter().map(Some).collect() });
    world::send_to(owner, world::Out::Records(records));
    tokio::spawn(async move {
        tokio::time::sleep(BAG_LIFETIME).await;
        remove(id);
    });
}

/// `LootBagFullUpdateIIR_c` (ReadSubClass 0x123a0de): ProtoGameCodeDynelIIR, ProtoSimpleDynelIIR,
/// VisualData (u32 resId, u32 resType, u32 n + n × 5 u32, 6 × f32 bbox, f32 scale, u32, u8;
/// the rest can be zero, each item brings its own mesh), ProtoLootBagIIR. Item positions must be 0..n-1.
fn bag_record(id: Id32, playfield: u32, pos: [f32; 3], owner: Id32, dropper: Id32, items: &[u32]) -> Bytes {
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield),
        running_effects: vec![],
    };
    let id32 = |i: Id32| ProtoId32 { r#type: Some(i.kind), instance: Some(i.instance) };
    let bag = ProtoLootBagIir {
        cash: Some(0),
        owner_id: Some(id32(owner)),
        dropper_id: Some(id32(dropper)),
        inventory: Some(ProtoInventoryGame {
            inventory_id: id32(id),
            max_size: Some(items.len() as u32),
            disabled_slots_count: None,
            items: items
                .iter()
                .enumerate()
                .map(|(pos, &template)| InventoryListEntry {
                    pos: pos as u32,
                    item: ProtoInventoryGameEntry {
                        item_id: Some(u64::from(template)),
                        item_data: Some(acg_item(template, 1)),
                        ..Default::default()
                    },
                })
                .collect(),
        }),
        ..Default::default()
    };
    iir::record("LootBagFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&ProtoSimpleDynelIir::default());
        let w = w.u32(0).u32(0).u32(0);
        let w = (0..6).fold(w, |w, _| w.f32(0.0));
        // scale (must be non-zero), u32, u8
        w.f32(1.0).u32(0).u8(0).proto(&bag)
    })
}

/// The player loots item `pos` of `bag`. Returns the item's template, or `None` if the bag or
/// item is gone or isn't theirs. Confirms the pickup to the client.
pub fn take(player: Id32, bag: Id32, pos: u32) -> Option<u32> {
    let mut bags = BAGS.lock().unwrap();
    let b = bags.get_mut(&bag)?;
    if b.owner != player {
        return None;
    }
    let template = b.items.get_mut(pos as usize)?.take()?;
    let empty = b.items.iter().all(Option::is_none);
    drop(bags);
    info!(?player, ?bag, pos, template, "loot");
    // LootRemoveItemsIIR_c: u32 cash removed, container of u32 item positions
    let removed = iir::record("LootRemoveItemsIIR_c", bag, false, |w| w.u32(0).u32(2 * 1009).u32(pos));
    world::send_to(player, world::Out::Records(vec![removed]));
    if empty {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            remove(bag);
        });
    }
    Some(template)
}

fn remove(bag: Id32) {
    if let Some(b) = BAGS.lock().unwrap().remove(&bag) {
        world::send_to(b.owner, world::Out::Records(vec![despawn_record(bag)]));
    }
}
