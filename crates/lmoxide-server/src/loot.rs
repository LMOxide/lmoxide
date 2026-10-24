//! Loot bags: what defeated enemies and smashed objects drop.
//!
//! A bag is a dynel of type 51322 (ProjectLootBag) created with UpdateMessage op 0 and
//! `LootBagFullUpdateIIR_c`. When its owner's client sees it, it splits the items into local
//! ClientLootBags that burst out and land, auto-collects them when the player walks close, and
//! sends `GameCharAgent::TryLootItem(bag, item position, owner, 0)` per item. The client never
//! adds the reward itself: the server sends it (token update or health) and confirms the pickup
//! with `LootRemoveItemsIIR_c`.
//!
//! Only the bag's owner sees its items (`ProjectLootBag::OnEnteringPlayfield`), so team loot is
//! a bag per member. Minifigure parts (tokens), minifigure bags and chests: data/minifig_parts.json,
//! data/loot_items.json (tools/re/extract_minifig_parts.py, extract_loot_items.py); the drop
//! tables were server-side, the rates here are proposals (docs/TUNABLES.md).

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{
        ProtoGameCodeDynelIir, ProtoId32, ProtoInventoryGame, ProtoInventoryGameEntry, ProtoLootBagIir, ProtoQuat,
        ProtoSimpleDynelIir, ProtoVec3, proto_inventory_game::InventoryListEntry,
    },
    types::Id32,
};
use serde::Deserialize;
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
/// "XX_25_Minifigure XP": 25 stars (a recorded quest reward).
pub const STARS_25_ITEM: u32 = 386269;
/// Bags nobody empties disappear after this long.
const BAG_LIFETIME: Duration = Duration::from_secs(60);
/// Teammates within this distance of a kill get their own bag.
const SHARE_RANGE: f32 = 60.0;

struct Bag {
    owner: Id32,
    playfield: u32,
    pos: [f32; 3],
    /// Item template per position; `None` once looted.
    items: Vec<Option<u32>>,
}

#[derive(Deserialize)]
struct PartsFile {
    minifigures: Vec<MinifigureParts>,
}

#[derive(Deserialize)]
struct MinifigureParts {
    minifigure_template_id: u32,
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    part_template_id: u32,
    part_token_id: u32,
}

#[derive(Deserialize)]
struct LootItemsFile {
    minifigure_bags: Vec<BagDef>,
    chests: Vec<ChestDef>,
}

#[derive(Deserialize)]
struct BagDef {
    bag_template: u32,
    minifigure_template: Option<u32>,
    controlled_loot_type: Option<u32>,
}

#[derive(Deserialize)]
struct ChestDef {
    template: u32,
    loot_type: Option<u32>,
    name: String,
}

struct Tables {
    /// part item template -> part token
    part_token: HashMap<u32, u32>,
    part_items: Vec<u32>,
    /// minifigure template -> its head/torso/legs tokens
    minifigure_parts: HashMap<u32, Vec<u32>>,
    /// minifigure bag item -> minifigure template
    bag_minifigure: HashMap<u32, u32>,
    bag_items: Vec<u32>,
    /// free chests (LootType 1); member chests need a membership check the original server made
    chests: Vec<u32>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    let parts: PartsFile = serde_json::from_str(include_str!("../../../data/minifig_parts.json")).expect("minifig_parts.json");
    let items: LootItemsFile = serde_json::from_str(include_str!("../../../data/loot_items.json")).expect("loot_items.json");
    let mut t = Tables {
        part_token: HashMap::new(),
        part_items: Vec::new(),
        minifigure_parts: HashMap::new(),
        bag_minifigure: HashMap::new(),
        bag_items: Vec::new(),
        chests: Vec::new(),
    };
    for m in parts.minifigures {
        for p in &m.parts {
            t.part_token.insert(p.part_template_id, p.part_token_id);
            t.part_items.push(p.part_template_id);
        }
        t.minifigure_parts.insert(m.minifigure_template_id, m.parts.iter().map(|p| p.part_token_id).collect());
    }
    // ControlledLootType 3: the regular bags (4/5 are the tutorial's)
    for b in items.minifigure_bags.iter().filter(|b| b.controlled_loot_type == Some(3)) {
        if let Some(m) = b.minifigure_template {
            t.bag_minifigure.insert(b.bag_template, m);
            t.bag_items.push(b.bag_template);
        }
    }
    t.chests = items.chests.iter().filter(|c| c.loot_type == Some(1) && !c.name.contains("Endless")).map(|c| c.template).collect();
    t
});

/// Head/torso/legs part tokens of a minifigure.
pub fn minifigure_part_tokens(template: u32) -> &'static [u32] {
    TABLES.minifigure_parts.get(&template).map_or(&[], Vec::as_slice)
}

/// Every minifigure part token.
pub fn all_part_tokens() -> impl Iterator<Item = u32> {
    TABLES.minifigure_parts.values().flatten().copied()
}

/// What picking up an item gives.
pub enum Reward {
    Stars(u64),
    HealthOrb,
    /// one of this minifigure part token
    Part(u32),
    /// a minifigure bag for this minifigure template
    MinifigureBag(u32),
    /// a chest: opening it drops another bag
    Chest,
    Nothing,
}

pub fn reward(template: u32) -> Reward {
    match template {
        STAR_ITEM => Reward::Stars(1),
        STARS_25_ITEM => Reward::Stars(25),
        HEALTH_ORB_ITEM => Reward::HealthOrb,
        t => {
            if let Some(&token) = TABLES.part_token.get(&t) {
                Reward::Part(token)
            } else if let Some(&m) = TABLES.bag_minifigure.get(&t) {
                Reward::MinifigureBag(m)
            } else if TABLES.chests.contains(&t) {
                Reward::Chest
            } else {
                Reward::Nothing
            }
        }
    }
}

/// What dropped the loot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Smashable,
    Enemy,
    Champion,
    Boss,
    Chest,
}

static RNG: Mutex<u64> = Mutex::new(0);

fn random() -> u64 {
    let mut s = RNG.lock().unwrap();
    if *s == 0 {
        *s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    }
    // xorshift64*
    *s ^= *s >> 12;
    *s ^= *s << 25;
    *s ^= *s >> 27;
    s.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

pub fn chance(percent: u64) -> bool {
    random() % 100 < percent
}

fn pick(items: &[u32]) -> Option<u32> {
    (!items.is_empty()).then(|| items[(random() % items.len() as u64) as usize])
}

/// One bag's contents. Stars as recorded (smashables 1-2, enemies 3); everything else proposed.
/// `orb_bonus`: extra health orb chance in percent (the Resourceful upgrade).
pub fn roll(source: Source, orb_bonus: u64) -> Vec<u32> {
    let t = &*TABLES;
    let (stars, orb, part_chance, parts, bag_chance, chest) = match source {
        Source::Smashable => (1 + random() as usize % 2, 0, 4, 1, 0, false),
        Source::Enemy => (3, 35, 6, 1, 0, false),
        Source::Champion => (6, 100, 100, 1, 15, false),
        Source::Boss => (10, 100, 100, 2, 40, true),
        Source::Chest => (5, 0, 100, 2, 25, false),
    };
    let mut items = vec![STAR_ITEM; stars];
    if source == Source::Chest {
        items.push(STARS_25_ITEM);
    }
    if orb > 0 && chance(orb + orb_bonus) {
        items.push(HEALTH_ORB_ITEM);
    }
    if chance(part_chance) {
        items.extend((0..parts).filter_map(|_| pick(&t.part_items)));
    }
    if chance(bag_chance) {
        items.extend(pick(&t.bag_items));
    }
    if chest {
        items.extend(pick(&t.chests));
    }
    items
}

/// Loot for a kill: a bag for the killer and one for each teammate in play nearby, each
/// rolled separately.
pub fn drop_loot(killer: Id32, playfield: u32, pos: [f32; 3], dropper: Id32, source: Source) {
    let mut owners = vec![killer];
    for mate in crate::community::teammates(killer.instance) {
        let id = Id32::new(killer.kind, mate);
        if world::whereabouts(id).is_some_and(|(pf, p)| pf == playfield && distance(p, pos) <= SHARE_RANGE) {
            owners.push(id);
        }
    }
    for owner in owners {
        drop_items(owner, playfield, pos, dropper, roll(source, crate::procs::orb_bonus(owner)));
    }
}

/// An opened chest: a new bag where the chest stood.
pub fn open_chest(owner: Id32, playfield: u32, pos: [f32; 3], chest: Id32) {
    drop_items(owner, playfield, pos, chest, roll(Source::Chest, 0));
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

static BAGS: LazyLock<Mutex<HashMap<Id32, Bag>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_BAG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Drops `items` (templates) for `owner` at `pos`, sent to the owner only.
pub fn drop_items(owner: Id32, playfield: u32, pos: [f32; 3], dropper: Id32, items: Vec<u32>) {
    if items.is_empty() {
        return;
    }
    let id = Id32::new(LOOT_BAG_TYPE, NEXT_BAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    info!(?owner, ?id, ?items, "loot dropped");
    let records = vec![
        create_game_object(id, playfield, pos),
        bag_record(id, playfield, pos, owner, dropper, &items),
        // as the original server sent after every bag: who may loot it (ID32)
        iir::record("NewAllowedLooterIIR_c", id, false, |w| owner.write(w)),
    ];
    BAGS.lock().unwrap().insert(id, Bag { owner, playfield, pos, items: items.into_iter().map(Some).collect() });
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

/// An item taken from a bag.
pub struct Taken {
    pub template: u32,
    pub playfield: u32,
    /// where the bag lies
    pub pos: [f32; 3],
}

/// The player loots item `pos` of `bag`. Returns the item, or `None` if the bag or item is gone
/// or isn't theirs. Confirms the pickup to the client.
pub fn take(player: Id32, bag: Id32, pos: u32) -> Option<Taken> {
    let mut bags = BAGS.lock().unwrap();
    let b = bags.get_mut(&bag)?;
    if b.owner != player {
        return None;
    }
    let template = b.items.get_mut(pos as usize)?.take()?;
    let (playfield, bag_pos) = (b.playfield, b.pos);
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
    Some(Taken { template, playfield, pos: bag_pos })
}

fn remove(bag: Id32) {
    if let Some(b) = BAGS.lock().unwrap().remove(&bag) {
        world::send_to(b.owner, world::Out::Records(vec![despawn_record(bag)]));
    }
}
