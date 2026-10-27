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
//! a bag per member. What drops is content (docs/CONTENT.md): the loot tables in content/loot
//! (which table each source rolls: tunables.json `loot.tables`), the items they name, the
//! minifigures' parts, bags and where they drop (regions, rarity). The drop tables were
//! server-side on live; the rates are proposals (docs/TUNABLES.md).

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_content::{LootBundle, LootEntry, MinifigureDrop, MinifigureSource, Range};
use lmoxide_net::{
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

/// The recorded bags' name text (of template `loot.bag_template`, "XX_Loot Bag").
const BAG_DISPLAY_NAME: &str = r#"<remoteformat id="1209" category="50200" key="S2L)UkVW7#3EI46]q1Bq" knubot="0"  ></remoteformat>"#;
/// Stars ("minifigure XP").
const STAR_TOKEN: u32 = 100;
/// Loot tables name others at most this deep.
const MAX_DEPTH: u32 = 8;

struct Bag {
    owner: Id32,
    playfield: u32,
    pos: [f32; 3],
    /// Item template per position; `None` once looted.
    items: Vec<Option<u32>>,
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
    /// the items the loot rules name (tunables `loot.items`)
    star: u32,
    health_orb: u32,
    potion: u32,
}

/// An item's client template by its id (`item/...`).
fn template_of(id: &str) -> Option<u32> {
    crate::gamedata::get().items.get(id).and_then(|i| i.template)
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    use lmoxide_content::ItemKind;
    let c = crate::gamedata::get();
    let tunables = &c.tunables;
    let named = |key: &str| {
        let id = tunables.loot.items.get(key).unwrap_or_else(|| panic!("tunables.json loot.items has no {key:?}"));
        template_of(id).unwrap_or_else(|| panic!("tunables.json loot.items.{key}: {id} has no template"))
    };
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
        star: named("star"),
        health_orb: named("health_orb"),
        potion: named("potion"),
    };
    for item in c.items.values() {
        let Some(template) = item.template else { continue };
        let gives = |token: u32| item.tokens.iter().filter(move |g| g.token == token);
        match item.kind {
            // ControlledLootType 3: the regular bags (4/5 are the tutorial's)
            ItemKind::MinifigBag if item.controlled_loot_type == Some(3) => {
                if let Some(m) = item.minifigure.as_ref().and_then(|m| c.minifigures.get(m)) {
                    t.bag_minifigure.insert(template, m.template);
                }
            }
            ItemKind::Chest => match item.chest.as_ref().filter(|ch| !ch.unused).and_then(|ch| ch.loot_type) {
                Some(1) => t.chests.push(template),
                Some(2) => t.member_chests.push(template),
                _ => {}
            },
            _ => {}
        }
        if let Some(g) = gives(crate::game::TOKEN_HEALTH_POTIONS).find(|g| g.amount.unwrap_or(1) > 0) {
            t.potion_items.insert(template, g.amount.unwrap_or(1) as u64);
        }
        if let Some(n) = gives(STAR_TOKEN).find_map(|g| g.amount.filter(|a| *a > 0)) {
            t.star_items.insert(template, n as u64);
        }
    }
    let rarity = &tunables.rarity;
    for m in c.minifigures.values() {
        let mut tokens = Vec::new();
        for p in &m.parts {
            let Some(item) = c.items.get(&p.item) else { continue };
            let (Some(template), Some(token)) = (item.template, item.tokens.first()) else { continue };
            let weight = rarity.part_locations.get(&p.location).copied().unwrap_or(1);
            t.part_items.entry(m.template).or_default().push((weight, template));
            t.part_token.insert(template, token.token);
            tokens.push(token.token);
        }
        t.minifigure_parts.insert(m.template, tokens);
        if let Some(bag) = m.bag.as_deref().and_then(template_of) {
            t.minifigure_bag.insert(m.template, bag);
        }
        let Some(o) = &m.obtain else { continue };
        match o.source.as_str() {
            "world" => {
                let tier = o.rarity.as_ref().unwrap_or(&rarity.default);
                let weight = *rarity.tiers.get(tier).unwrap_or_else(|| panic!("{}: unknown rarity tier {tier}", m.id));
                let playfields = o.regions.iter().filter_map(|r| c.regions.get(r)).flat_map(|r| &r.worlds).filter_map(|w| c.worlds.get(w)).map(|w| w.playfield);
                for playfield in playfields {
                    t.world_minifigures.entry(playfield).or_default().push((weight, m.template));
                }
            }
            "achievement" => {
                if let Some(tag) = o.achievement {
                    t.achievement_minifigures.insert(tag, m.template);
                }
            }
            "membership" => t.membership_minifigures.push(m.template),
            _ => {}
        }
    }
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

/// The health orb item (tunables `loot.items.health_orb`).
pub fn health_orb_item() -> u32 {
    TABLES.health_orb
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
        template if template == t.health_orb => Reward::HealthOrb,
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

impl Source {
    /// Its key in tunables `loot.tables`.
    fn key(self) -> &'static str {
        match self {
            Source::Smashable => "smashable",
            Source::Enemy => "enemy",
            Source::Champion => "champion",
            Source::Boss => "boss",
            Source::Chest => "chest",
            Source::MemberChest => "member_chest",
        }
    }
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

/// True with probability `p` (0..1).
fn chance_of(p: f64) -> bool {
    (random() % 1_000_000) < (p * 1_000_000.0).round().max(0.0) as u64
}

/// A number in `r`.
fn in_range(r: Range) -> u32 {
    r.min + (random() % u64::from(r.max - r.min + 1)) as u32
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

/// A minifigure dropped in `playfield`: one of its region's (minifigures' `obtain.regions`), by
/// rarity (`obtain.rarity`, tunables `rarity.tiers`); none where no minifigure drops (the hub).
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

/// A minifigure from a loot entry's `from`: the world's, or one of those listed.
fn pick_from(from: &MinifigureSource, playfield: u32) -> Option<u32> {
    match from {
        MinifigureSource::World(_) => pick_minifigure(playfield),
        MinifigureSource::Some(list) => {
            let c = crate::gamedata::get();
            let templates: Vec<u32> = list.iter().filter_map(|m| c.minifigures.get(m)).map(|m| m.template).collect();
            pick(&templates)
        }
    }
}

/// One bag's contents in `playfield`: the loot table of `source` (tunables `loot.tables`).
/// `orb_bonus`: extra chance in percent for entries raised by `orb_chance` (the Resourceful
/// upgrade).
pub fn roll(source: Source, orb_bonus: u64, playfield: u32) -> Vec<u32> {
    let c = crate::gamedata::get();
    let mut items = Vec::new();
    match c.tunables.loot.tables.get(source.key()) {
        Some(table) => roll_table(table, orb_bonus, playfield, 0, &mut items),
        None => tracing::warn!(source = source.key(), "tunables.json loot.tables names no table"),
    }
    items
}

/// Rolls loot table `id` into `out`: entries in order; those with a `chance` roll on their own,
/// those without `chance` or `weight` always drop, and the weighted ones (`rolls` times, one each)
/// where the first of them is listed.
fn roll_table(id: &str, orb_bonus: u64, playfield: u32, depth: u32, out: &mut Vec<u32>) {
    let c = crate::gamedata::get();
    let Some(table) = c.loot.get(id) else {
        tracing::warn!(table = id, "no such loot table");
        return;
    };
    if depth > MAX_DEPTH {
        tracing::warn!(table = id, "loot tables nest too deep");
        return;
    }
    let applies = |e: &LootEntry| {
        let Some(w) = &e.when else { return true };
        // everyone is a member here; once_per_player isn't kept yet
        w.worlds.is_empty() || w.worlds.iter().filter_map(|w| c.worlds.get(w)).any(|w| w.playfield == playfield)
    };
    let weighted: Vec<&LootEntry> = table.entries.iter().filter(|e| e.weight.is_some() && applies(e)).collect();
    let mut weighted_done = false;
    for e in &table.entries {
        if e.weight.is_some() {
            if !weighted_done {
                weighted_done = true;
                let choices: Vec<(u32, u32)> = weighted.iter().enumerate().map(|(i, e)| (e.weight.unwrap_or(0), i as u32)).collect();
                for _ in 0..table.rolls {
                    if let Some(i) = pick_weighted(&choices) {
                        give(&weighted[i as usize].drop, orb_bonus, playfield, depth, out);
                    }
                }
            }
            continue;
        }
        if !applies(e) {
            continue;
        }
        let drops = match e.chance {
            None => true,
            Some(p) => {
                let bonus = e.when.as_ref().and_then(|w| w.bonus_stat.as_deref()) == Some("orb_chance");
                chance_of(p.fraction() + if bonus { orb_bonus as f64 / 100.0 } else { 0.0 })
            }
        };
        if drops {
            give(&e.drop, orb_bonus, playfield, depth, out);
        }
    }
}

/// The items of one loot bundle.
fn give(b: &LootBundle, orb_bonus: u64, playfield: u32, depth: u32, out: &mut Vec<u32>) {
    let t = &*TABLES;
    let times = |r: Option<Range>| r.map_or(0, in_range) as usize;
    out.extend(std::iter::repeat_n(t.star, times(b.stars)));
    out.extend(std::iter::repeat_n(t.health_orb, times(b.hearts)));
    out.extend(std::iter::repeat_n(t.potion, times(b.potions)));
    let minifigures = |d: &MinifigureDrop, out: &mut Vec<u32>, of: fn(u32) -> Option<u32>| {
        for _ in 0..in_range(d.count) {
            out.extend(pick_from(&d.from, playfield).and_then(of));
        }
    };
    if let Some(d) = &b.minifig_parts {
        minifigures(d, out, |m| TABLES.part_items.get(&m).and_then(|p| pick_weighted(p)));
    }
    if let Some(d) = &b.minifig_bags {
        minifigures(d, out, |m| TABLES.minifigure_bag.get(&m).copied());
    }
    if !b.chest.is_empty() {
        let chests: Vec<u32> = b.chest.iter().filter_map(|c| template_of(c)).collect();
        out.extend(pick(&chests));
    }
    out.extend(b.items.iter().filter_map(|i| template_of(i)));
    if let Some(table) = &b.table {
        roll_table(table, orb_bonus, playfield, depth + 1, out);
    }
}

/// `!loot` kinds.
pub const TEST_KINDS: &[&str] = &["potion", "part", "bag", "chest", "memberchest", "boss", "enemy", "smashable"];

/// A test bag of `kind` (TEST_KINDS) in `playfield`.
pub fn test_items(kind: &str, playfield: u32) -> Vec<u32> {
    let t = &*TABLES;
    match kind {
        "potion" => vec![t.potion],
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
        if world::whereabouts(id).is_some_and(|(pf, p)| pf == playfield && distance(p, pos) <= crate::gamedata::get().tunables.loot.share_range) {
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
    let id = Id32::new(crate::gamedata::get().tunables.loot.bag_dynel_type, NEXT_BAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
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
        tokio::time::sleep(crate::gamedata::get().tunables.loot.bag_lifetime.to_std()).await;
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
        let w = w.proto(&dynel).proto(&bag_dynel());
        let w = w.u32(0).u32(0).u32(0);
        let w = (0..6).fold(w, |w, _| w.f32(0.0));
        // scale (must be non-zero), u32, u8
        w.f32(1.0).u32(0).u8(0).proto(&bag)
    })
}

/// The bag's SimpleDynel part, as every recorded bag had it. `is_closed` matters: a bag can be
/// opened (SimpleDynel::CanOpen: CARS use, CanBeOpened and IsClosed) only while closed, and
/// clicking an item works only when its bag can be opened (SimpleChar_t::GetDefaultCharacterAction
/// for ClientLootBag); without it a click on a part did nothing.
fn bag_dynel() -> ProtoSimpleDynelIir {
    ProtoSimpleDynelIir {
        kdtree: Some(0),
        name: Some("XX_Loot Bag".into()),
        display_name: Some(BAG_DISPLAY_NAME.into()),
        cars_group: Some(0),
        template_item_id: Some(crate::gamedata::get().tunables.loot.bag_template),
        is_closed: Some(true),
        interaction_radius: Some(crate::gamedata::get().tunables.loot.bag_interaction_radius),
        ..Default::default()
    }
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
        assert!(matches!(reward(399_661), Reward::Potions(1)));
        assert!(matches!(reward(20_634), Reward::Stars(1)));
        assert!(matches!(reward(386_272), Reward::Stars(1000)));
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
