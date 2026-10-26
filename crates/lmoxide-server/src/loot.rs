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
/// Stars ("minifigure XP").
const STAR_TOKEN: u32 = 100;
/// "Health Orb": heals the active minifigure.
pub const HEALTH_ORB_ITEM: u32 = 20908;
/// "XX_25_Minifigure XP": 25 stars (a recorded quest reward).
pub const STARS_25_ITEM: u32 = 386269;
/// "XX_100_Minifigure XP": 100 stars.
pub const STARS_100_ITEM: u32 = 386270;
/// "XX_250_Minifigure XP" and "XX_1000_Minifigure XP".
const STARS_250_ITEM: u32 = 386271;
const STARS_1000_ITEM: u32 = 386272;
/// "Health Potion": one potion (token 150).
pub const HEALTH_POTION_ITEM: u32 = 399661;
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
    location: String,
    part_template_id: u32,
    part_token_id: u32,
}

#[derive(Deserialize)]
struct DropsFile {
    worlds: HashMap<String, Vec<u32>>,
    minifigures: Vec<DropDef>,
}

#[derive(Deserialize)]
struct DropDef {
    template: u32,
    name: Option<String>,
    source: String,
    #[serde(default)]
    worlds: Vec<String>,
    achievement: Option<u32>,
}

#[derive(Deserialize)]
struct RarityFile {
    tiers: HashMap<String, u32>,
    default: String,
    minifigures: HashMap<String, String>,
    part_locations: HashMap<String, u32>,
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
    /// minifigure template -> its parts: (location weight, part item template)
    part_items: HashMap<u32, Vec<(u32, u32)>>,
    /// minifigure template -> its head/torso/legs tokens
    minifigure_parts: HashMap<u32, Vec<u32>>,
    /// minifigure bag item -> minifigure template
    bag_minifigure: HashMap<u32, u32>,
    /// minifigure template -> its bag item
    minifigure_bag: HashMap<u32, u32>,
    /// playfield -> the minifigures dropped there: (rarity weight, minifigure template)
    world_minifigures: HashMap<u32, Vec<(u32, u32)>>,
    /// free chests (LootType 1)
    chests: Vec<u32>,
    /// member chests (LootType 2): every player is a member here
    member_chests: Vec<u32>,
    /// token items of Health Potions -> potions
    potion_items: HashMap<u32, u64>,
    /// token items of stars (minifigure XP, token 100) -> stars
    star_items: HashMap<u32, u64>,
    /// minifigures given for an achievement (lore tag) and to members
    achievement_minifigures: HashMap<u32, u32>,
    membership_minifigures: Vec<u32>,
}

#[derive(Deserialize)]
struct TokenItem {
    template: u32,
    token: u32,
    /// (negative for items that take tokens)
    amount: Option<i64>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct Tokens {
        token_items: Vec<TokenItem>,
    }
    let parts: PartsFile = serde_json::from_str(include_str!("../../../data/minifig_parts.json")).expect("minifig_parts.json");
    let items: LootItemsFile = serde_json::from_str(include_str!("../../../data/loot_items.json")).expect("loot_items.json");
    let tokens: Tokens = serde_json::from_str(include_str!("../../../data/loot_items.json")).expect("loot_items.json");
    let drops: DropsFile = serde_json::from_str(include_str!("../../../data/minifigure_drops.json")).expect("minifigure_drops.json");
    let rarity: RarityFile =
        serde_json::from_str(include_str!("../../../data/minifigure_rarity.json")).expect("minifigure_rarity.json");
    let mut t = Tables {
        part_token: HashMap::new(),
        part_items: HashMap::new(),
        minifigure_parts: HashMap::new(),
        bag_minifigure: HashMap::new(),
        minifigure_bag: HashMap::new(),
        world_minifigures: HashMap::new(),
        chests: Vec::new(),
        member_chests: Vec::new(),
        potion_items: HashMap::new(),
        star_items: HashMap::new(),
        achievement_minifigures: HashMap::new(),
        membership_minifigures: Vec::new(),
    };
    for m in parts.minifigures {
        for p in &m.parts {
            t.part_token.insert(p.part_template_id, p.part_token_id);
            let weight = rarity.part_locations.get(&p.location).copied().unwrap_or(1);
            t.part_items.entry(m.minifigure_template_id).or_default().push((weight, p.part_template_id));
        }
        t.minifigure_parts.insert(m.minifigure_template_id, m.parts.iter().map(|p| p.part_token_id).collect());
    }
    // ControlledLootType 3: the regular bags (4/5 are the tutorial's)
    for b in items.minifigure_bags.iter().filter(|b| b.controlled_loot_type == Some(3)) {
        if let Some(m) = b.minifigure_template {
            t.bag_minifigure.insert(b.bag_template, m);
            t.minifigure_bag.entry(m).or_insert(b.bag_template);
        }
    }
    for d in &drops.minifigures {
        match d.source.as_str() {
            "world" => {
                let tier = d.name.as_ref().and_then(|n| rarity.minifigures.get(n)).unwrap_or(&rarity.default);
                let weight = *rarity.tiers.get(tier).unwrap_or_else(|| panic!("minifigure_rarity.json: unknown tier {tier}"));
                for playfield in d.worlds.iter().flat_map(|w| drops.worlds.get(w).into_iter().flatten()) {
                    t.world_minifigures.entry(*playfield).or_default().push((weight, d.template));
                }
            }
            "achievement" => {
                if let Some(tag) = d.achievement {
                    t.achievement_minifigures.insert(tag, d.template);
                }
            }
            "membership" => t.membership_minifigures.push(d.template),
            _ => {}
        }
    }
    let usable = |c: &&ChestDef| !c.name.contains("Endless");
    t.chests = items.chests.iter().filter(usable).filter(|c| c.loot_type == Some(1)).map(|c| c.template).collect();
    t.member_chests = items.chests.iter().filter(usable).filter(|c| c.loot_type == Some(2)).map(|c| c.template).collect();
    t.potion_items = tokens
        .token_items
        .iter()
        .filter(|i| i.token == crate::game::TOKEN_HEALTH_POTIONS && i.amount.unwrap_or(1) > 0)
        .map(|i| (i.template, i.amount.unwrap_or(1) as u64))
        .collect();
    t.star_items = tokens
        .token_items
        .iter()
        .filter(|i| i.token == STAR_TOKEN && i.amount.is_some_and(|a| a > 0))
        .map(|i| (i.template, i.amount.unwrap_or(1) as u64))
        .collect();
    t
});

/// The minifigures given for an achievement: (lore tag, minifigure) (Chicken Suit Guy).
pub fn achievement_minifigures() -> impl Iterator<Item = (u32, u32)> {
    TABLES.achievement_minifigures.iter().map(|(&tag, &m)| (tag, m))
}

/// The minifigures every member owns (long-term membership benefits).
pub fn membership_minifigures() -> &'static [u32] {
    &TABLES.membership_minifigures
}

/// Head/torso/legs part tokens of a minifigure.
pub fn minifigure_part_tokens(template: u32) -> &'static [u32] {
    TABLES.minifigure_parts.get(&template).map_or(&[], Vec::as_slice)
}

/// Every minifigure part token: (minifigure template, part token).
pub fn all_part_tokens() -> impl Iterator<Item = (u32, u32)> {
    TABLES.minifigure_parts.iter().flat_map(|(&m, tokens)| tokens.iter().map(move |&t| (m, t)))
}

/// What picking up an item gives.
pub enum Reward {
    Stars(u64),
    HealthOrb,
    /// Health Potions (token 150)
    Potions(u64),
    /// one of this minifigure part token
    Part(u32),
    /// a minifigure bag for this minifigure template
    MinifigureBag(u32),
    /// a chest: opening it drops another bag (`member`: a Member Super Chest)
    Chest { member: bool },
    Nothing,
}

pub fn reward(template: u32) -> Reward {
    let t = &*TABLES;
    match template {
        HEALTH_ORB_ITEM => Reward::HealthOrb,
        template => {
            if let Some(&n) = t.star_items.get(&template) {
                Reward::Stars(n)
            } else if let Some(&token) = t.part_token.get(&template) {
                Reward::Part(token)
            } else if let Some(&m) = t.bag_minifigure.get(&template) {
                Reward::MinifigureBag(m)
            } else if let Some(&n) = t.potion_items.get(&template) {
                Reward::Potions(n)
            } else if t.chests.contains(&template) {
                Reward::Chest { member: false }
            } else if t.member_chests.contains(&template) {
                Reward::Chest { member: true }
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
    MemberChest,
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

/// One of `items` (weight, value), by weight.
fn pick_weighted(items: &[(u32, u32)]) -> Option<u32> {
    let total: u64 = items.iter().map(|&(w, _)| u64::from(w)).sum();
    if total == 0 {
        return None;
    }
    let mut r = random() % total;
    for &(w, v) in items {
        if r < u64::from(w) {
            return Some(v);
        }
        r -= u64::from(w);
    }
    None
}

/// A minifigure dropped in `playfield`: one of its world's (data/minifigure_drops.json), by
/// rarity (data/minifigure_rarity.json); none where no minifigure drops (the hub).
fn pick_minifigure(playfield: u32) -> Option<u32> {
    TABLES.world_minifigures.get(&playfield).and_then(|m| pick_weighted(m))
}

/// A part of a minifigure dropped in `playfield` (its location by the location weights).
fn pick_part(playfield: u32) -> Option<u32> {
    let minifigure = pick_minifigure(playfield)?;
    TABLES.part_items.get(&minifigure).and_then(|p| pick_weighted(p))
}

/// The bag of a minifigure dropped in `playfield`.
fn pick_bag(playfield: u32) -> Option<u32> {
    let minifigure = pick_minifigure(playfield)?;
    TABLES.minifigure_bag.get(&minifigure).copied()
}

/// One bag's contents in `playfield`. Stars as recorded (smashables 1-2, enemies 3); everything
/// else proposed (docs/TUNABLES.md). `orb_bonus`: extra health orb chance in percent (the
/// Resourceful upgrade).
pub fn roll(source: Source, orb_bonus: u64, playfield: u32) -> Vec<u32> {
    let t = &*TABLES;
    // (stars, guaranteed bundle, orb %, potion %, part %, parts, bag %, chests)
    let (stars, bundle, orb, potion, part_chance, parts, bag_chance, chests) = match source {
        Source::Smashable => (1 + random() as usize % 2, None, 0, 3, 4, 1, 0, false),
        Source::Enemy => (3, None, 35, 5, 6, 1, 1, false),
        Source::Champion => (6, Some(STARS_25_ITEM), 100, 50, 100, 1, 15, false),
        Source::Boss => (10, Some(STARS_100_ITEM), 100, 100, 100, 2, 40, true),
        Source::Chest => (5, Some(STARS_25_ITEM), 0, 100, 100, 2, 25, false),
        Source::MemberChest => (5, Some(STARS_100_ITEM), 0, 100, 100, 3, 50, false),
    };
    let mut items = vec![STAR_ITEM; stars];
    items.extend(bundle);
    // bigger star bundles now and then (per mille)
    let bundles: &[(u32, u64)] = match source {
        Source::Smashable => &[(STARS_25_ITEM, 20)],
        Source::Enemy => &[(STARS_25_ITEM, 30), (STARS_100_ITEM, 5)],
        Source::Champion => &[(STARS_100_ITEM, 200)],
        Source::Boss => &[(STARS_250_ITEM, 250), (STARS_1000_ITEM, 50)],
        Source::Chest => &[(STARS_100_ITEM, 200)],
        Source::MemberChest => &[(STARS_250_ITEM, 300), (STARS_1000_ITEM, 50)],
    };
    items.extend(bundles.iter().filter(|&&(_, per_mille)| random() % 1000 < per_mille).map(|&(item, _)| item));
    if orb > 0 && chance(orb + orb_bonus) {
        items.push(HEALTH_ORB_ITEM);
    }
    if chance(potion) {
        items.push(HEALTH_POTION_ITEM);
    }
    if chance(part_chance) {
        items.extend((0..parts).filter_map(|_| pick_part(playfield)));
    }
    if chance(bag_chance) {
        items.extend(pick_bag(playfield));
    }
    if chests {
        // a free and a member chest side by side, as live placed them (LootFreeChestRotation,
        // LootMemberChestRotation and their offsets)
        items.extend(pick(&t.chests));
        items.extend(pick(&t.member_chests));
    }
    items
}

/// `!loot` kinds.
pub const TEST_KINDS: &[&str] = &["potion", "part", "bag", "chest", "memberchest", "boss", "enemy", "smashable"];

/// A test bag of `kind` (TEST_KINDS) in `playfield`.
pub fn test_items(kind: &str, playfield: u32) -> Vec<u32> {
    let t = &*TABLES;
    match kind {
        "potion" => vec![HEALTH_POTION_ITEM],
        "part" => pick_part(playfield).into_iter().collect(),
        "bag" => pick_bag(playfield).into_iter().collect(),
        "chest" => pick(&t.chests).into_iter().collect(),
        "memberchest" => pick(&t.member_chests).into_iter().collect(),
        "boss" => roll(Source::Boss, 0, playfield),
        "enemy" => roll(Source::Enemy, 0, playfield),
        _ => roll(Source::Smashable, 0, playfield),
    }
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
        drop_items(owner, playfield, pos, dropper, roll(source, crate::procs::orb_bonus(owner), playfield));
    }
}

/// An opened chest: a new bag where the chest stood.
pub fn open_chest(owner: Id32, playfield: u32, pos: [f32; 3], chest: Id32, member: bool) {
    let source = if member { Source::MemberChest } else { Source::Chest };
    drop_items(owner, playfield, pos, chest, roll(source, 0, playfield));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_come_from_the_world() {
        // Isle of Yarr: Pirate World minifigures only; the hub drops none
        let pirate: Vec<u32> = TABLES.world_minifigures[&4200].iter().map(|&(_, m)| m).collect();
        assert!(!pirate.is_empty());
        for _ in 0..200 {
            let m = pick_minifigure(4200).unwrap();
            assert!(pirate.contains(&m));
            let part = pick_part(4200).unwrap();
            assert!(matches!(reward(part), Reward::Part(_)));
            assert!(matches!(reward(pick_bag(4200).unwrap()), Reward::MinifigureBag(m) if pirate.contains(&m)));
        }
        assert!(pick_minifigure(1200).is_none());
        // side dungeons drop their world's: Beach Cave (1642) opens into Volcano Island
        assert!(TABLES.world_minifigures[&1642].iter().all(|&(_, m)| pirate.contains(&m)));
    }

    #[test]
    fn rewards_of_new_items() {
        assert!(matches!(reward(HEALTH_POTION_ITEM), Reward::Potions(1)));
        assert!(matches!(reward(STAR_ITEM), Reward::Stars(1)));
        assert!(matches!(reward(STARS_1000_ITEM), Reward::Stars(1000)));
        assert!(matches!(reward(465_756), Reward::Stars(250)));
        assert!(matches!(reward(464_407), Reward::Potions(10)));
        assert!(matches!(reward(580_943), Reward::Chest { member: true }));
        assert!(matches!(reward(482_326), Reward::Chest { member: false }));
        let boss = roll(Source::Boss, 0, 4200);
        assert_eq!(boss.iter().filter(|&&i| matches!(reward(i), Reward::Chest { .. })).count(), 2);
        assert_eq!(achievement_minifigures().count(), 1);
        assert_eq!(membership_minifigures().len(), 2);
    }
}
