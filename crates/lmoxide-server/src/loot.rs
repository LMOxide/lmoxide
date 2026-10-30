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
//! server-side on live; the rates are fitted to the live capture where it had data
//! (docs/TUNABLES.md).
//!
//! As live (capture 2016): a bag is its killer's; one nobody has looted from opens to everyone
//! after `loot.bag_public_after` (a second `NewAllowedLooterIIR_c` with (0,0)); an emptied bag
//! goes about 4 s after its last item is taken.

use crate::instance::Pf;
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_content::MinifigureSource;
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
/// An emptied bag goes this long after its last item is taken (live capture: about 4 s).
const EMPTIED_BAG_LINGER: Duration = Duration::from_secs(4);
/// The live bags' inventory id type (instance: the bag's) and size.
const BAG_INVENTORY_TYPE: u32 = 115;
const BAG_INVENTORY_SIZE: u32 = 100;

struct Bag {
    owner: Id32,
    /// Anyone may loot it (`loot.bag_public_after` passed with nothing taken).
    public: bool,
    playfield: Pf,
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
    /// pocket adventure playfield -> its own minifigures (world `parts`): parts only, no bags
    parts_only: HashMap<u32, Vec<u32>>,
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
        parts_only: HashMap::new(),
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
    // a pocket adventure's own minifigure is only given for completing it (game.rs), never dropped
    let rewards: Vec<u32> = c.worlds.values().filter_map(|w| Some(c.minifigures.get(w.minifigure.as_ref()?)?.template)).collect();
    for list in t.world_minifigures.values_mut() {
        list.retain(|(_, m)| !rewards.contains(m));
    }
    // a pocket adventure drops its own minifigures' (world `parts`) instead of its region's
    for w in c.worlds.values().filter(|w| !w.parts.is_empty()) {
        let list = w
            .parts
            .iter()
            .filter_map(|id| c.minifigures.get(id))
            .map(|m| {
                let tier = m.obtain.as_ref().and_then(|o| o.rarity.as_ref()).unwrap_or(&rarity.default);
                (*rarity.tiers.get(tier).unwrap_or_else(|| panic!("{}: unknown rarity tier {tier}", m.id)), m.template)
            })
            .collect::<Vec<_>>();
        t.parts_only.insert(w.playfield, list.iter().map(|&(_, m)| m).collect());
        t.world_minifigures.insert(w.playfield, list);
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
    /// the Lost Creations' chest: opening it drops the run's rewards (lost_creations.rs)
    RewardChest,
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
            } else if template_of(&crate::gamedata::get().tunables.lost_creations.reward_chest) == Some(template) {
                Reward::RewardChest
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

/// The loot power level a chest rolls at for a character of `level` on playfield `model` (the
/// Lua's DoSuperChestPowerLevel, quest-spawned branch): `level × power_per_level`, at most its
/// super-region's cap (tunables `loot.power_caps`); the Treasure Chamber (7001) and unknown
/// super-regions aren't capped. Every roll takes a power (tunables `loot.power` says what it
/// changes; [`chest_power`], [`quest_power`], [`npc_power`]).
pub fn power_level(level: u32, model: u32) -> u32 {
    let c = crate::gamedata::get();
    let power = level * c.tunables.loot.power_per_level;
    let cap = c.world_by_playfield(model).and_then(|w| w.super_region).filter(|_| model != 7001).and_then(|r| c.tunables.loot.power_caps.get(&r.to_string()).copied());
    cap.map_or(power, |cap| power.min(cap))
}

/// A Super Chest's power (DoSuperChestPowerLevel, script 3727) opened by a character of `level`
/// in `playfield`: in a Lost Creations level (an Endless playfield: `endless_progress` set) the
/// fallback, `level × power_per_level`; else the chest's own `loot_power_level` (LootPowerLevel
/// 1431) when it has one; else [`power_level`]. (The Lua's "scaleable dungeon" branch,
/// min(endlesslevel, level) × 100, needs a playfield `endlesslevel` LMOxide never sets.)
pub fn chest_power(level: u32, playfield: Pf, chest: u32) -> u32 {
    let c = crate::gamedata::get();
    if crate::lost_creations::element(playfield).is_some() {
        return level * c.tunables.loot.power_per_level;
    }
    let own = c.items.values().find(|i| i.template == Some(chest)).and_then(|i| i.chest.as_ref()).and_then(|ch| ch.loot_power_level).filter(|&p| p > 0);
    own.unwrap_or_else(|| power_level(level, playfield.model()))
}

/// A quest reward's power (the "Quest Completion Loot Spawner" scripts 555, 987, 988, 1063-1068,
/// 1121, 1287, 1756): `level × power_per_level`, at most the cap of the playfield's thousands
/// (tunables `loot.quest_power_caps`: by playfield number, not super-region, so micro-dungeons
/// and the hub aren't capped), at least its floor (`quest_power_floors`: the scripts' Dino branch
/// uses math.max, so Dino quests reward at 3000 or more, logged).
pub fn quest_power(level: u32, model: u32) -> u32 {
    let loot = &crate::gamedata::get().tunables.loot;
    let power = level * loot.power_per_level;
    let key = (model / 1000 * 1000).to_string();
    let capped = loot.quest_power_caps.get(&key).map_or(power, |&cap| power.min(cap));
    match loot.quest_power_floors.get(&key) {
        Some(&floor) if floor > capped => {
            info!(level, model, power = floor, "quest reward power raised to the floor (the client Lua's Dino math.max)");
            floor
        }
        _ => capped,
    }
}

/// An enemy's or smashable's power: its template's `stats.power_level` (the client's NPC
/// PowerLevel 499), else `level × loot.npc_power`.
pub fn npc_power(level: u32, power_level: Option<u32>) -> u32 {
    power_level.unwrap_or(level * crate::gamedata::get().tunables.loot.npc_power)
}

/// True with probability `p` (0..1), to 0.01%.
pub fn chance_fraction(p: f64) -> bool {
    (random() % 10_000) < (p * 10_000.0).round() as u64
}

fn pick(items: &[u32]) -> Option<u32> {
    (!items.is_empty()).then(|| items[(random() % items.len() as u64) as usize])
}

/// A random number below `n` (> 0).
pub fn random_below(n: u32) -> u32 {
    (random() % u64::from(n.max(1))) as u32
}

/// One of minifigure `template`'s parts (item template), by where it is (rarity
/// `part_locations`).
pub fn part_item(template: u32) -> Option<u32> {
    TABLES.part_items.get(&template).and_then(|p| pick_weighted(p))
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
fn pick_minifigure(playfield: Pf) -> Option<u32> {
    TABLES.world_minifigures.get(&playfield.model()).and_then(|m| pick_weighted(m))
}

/// A part of a minifigure dropped in `playfield` (its location by the location weights).
fn pick_part(playfield: Pf) -> Option<u32> {
    let minifigure = pick_minifigure(playfield)?;
    TABLES.part_items.get(&minifigure).and_then(|p| pick_weighted(p))
}

/// A minifigure whose whole bag drops in `playfield`: as `pick_minifigure`, but not a pocket
/// adventure's own (world `parts`), which drop only as parts (the user, 2026-10-29).
fn pick_bag_minifigure(playfield: Pf) -> Option<u32> {
    let list = TABLES.world_minifigures.get(&playfield.model())?;
    match TABLES.parts_only.get(&playfield.model()) {
        Some(parts) => pick_weighted(&list.iter().filter(|(_, m)| !parts.contains(m)).copied().collect::<Vec<_>>()),
        None => pick_weighted(list),
    }
}

/// The bag of a minifigure dropped in `playfield`.
fn pick_bag(playfield: Pf) -> Option<u32> {
    let minifigure = pick_bag_minifigure(playfield)?;
    TABLES.minifigure_bag.get(&minifigure).copied()
}

/// The bag of pocket adventure `playfield`'s minifigure (world `minifigure`): its reward for
/// clearing it. None where a story conversation there gives it (its `{spawn <hash>}`, tunables
/// `story.spawn_minifigures`: the Monkey Cave's Island Warrior, ...): only the conversation does,
/// once. With tunables `story.repeat_conversation_minifigures` a later clear gives the bag again
/// once the player (`has_tag`) has had that conversation. None too where a loot table names its
/// bag (a world's story conclusion gives its last story dungeon's: the Sorcerer's Keep's Evil
/// Wizard, ...): only that table does.
pub fn pocket_reward(playfield: u32, has_tag: impl Fn(u32) -> bool) -> Option<u32> {
    let c = crate::gamedata::get();
    let world = c.worlds.values().find(|w| w.playfield == playfield)?;
    let id = world.minifigure.as_deref()?;
    if table_gives_bag(id) {
        return None;
    }
    if let Some(played) = story_gives(playfield, id, &has_tag)
        && !(played && c.tunables.story.repeat_conversation_minifigures)
    {
        return None;
    }
    let minifigure = c.minifigures.get(id)?;
    TABLES.minifigure_bag.get(&minifigure.template).copied()
}

/// Whether a loot table gives minifigure `id`'s bag by name (`minifig_bags.from` listing it).
fn table_gives_bag(id: &str) -> bool {
    crate::gamedata::get().loot.values().flat_map(|t| &t.entries).any(|e| {
        matches!(&e.drop.minifig_bags, Some(lmoxide_content::MinifigureDrop { from: MinifigureSource::Some(list), .. }) if list.iter().any(|m| m == id))
    })
}

/// Whether a dialogue on `playfield` gives minifigure `id` (`spawn <hash>` of a story
/// minifigure): Some(whether the player has had it, all its tags), None if none does.
fn story_gives(playfield: u32, id: &str, has_tag: &impl Fn(u32) -> bool) -> Option<bool> {
    let story = &crate::gamedata::get().tunables.story.spawn_minifigures;
    let hashes: Vec<&String> = story.iter().filter(|(_, m)| *m == id).map(|(h, _)| h).collect();
    let spawners: Vec<Vec<u32>> = hashes.iter().flat_map(|h| crate::knubot::spawners(playfield, h)).collect();
    (!spawners.is_empty()).then(|| spawners.iter().any(|tags| !tags.is_empty() && tags.iter().all(|t| has_tag(*t))))
}

/// The bag item that holds minifigure `template`.
pub fn minifigure_bag(template: u32) -> Option<u32> {
    TABLES.minifigure_bag.get(&template).copied()
}

/// A minifigure from a loot entry's `from`: the world's, or one of those listed.
fn pick_from(from: &MinifigureSource, playfield: Pf) -> Option<u32> {
    match from {
        MinifigureSource::World(_) => pick_minifigure(playfield),
        MinifigureSource::Some(list) => {
            let c = crate::gamedata::get();
            let templates: Vec<u32> = list.iter().filter_map(|m| c.minifigures.get(m)).map(|m| m.template).collect();
            pick(&templates)
        }
    }
}

/// One bag's contents in `playfield` at loot power `power`: the loot table of `source`
/// (tunables `loot.tables`). `orb_bonus`: extra chance in percent for entries raised by
/// `orb_chance` (the Resourceful upgrade).
pub fn roll(source: Source, orb_bonus: u64, playfield: Pf, power: u32) -> Vec<u32> {
    let c = crate::gamedata::get();
    let mut items = Vec::new();
    match c.tunables.loot.tables.get(source.key()) {
        Some(table) => roll_table(table, orb_bonus, playfield, power, &mut items),
        None => tracing::warn!(source = source.key(), "tunables.json loot.tables names no table"),
    }
    items
}

/// Rolls loot table `id` (`loot/...`) at loot power `power` for a bag in `playfield` into
/// `out` (upgrade procs' `drop`, achievement and quest rewards).
pub fn roll_table_into(id: &str, orb_bonus: u64, playfield: Pf, power: u32, out: &mut Vec<u32>) {
    roll_table(id, orb_bonus, playfield, power, out);
}

/// Rolls loot table `id` into `out` (lmoxide_content::roll: the rules), each drop as a client
/// item: minifigure parts and bags of the minifigures it names or the world's, one of its chests.
fn roll_table(id: &str, orb_bonus: u64, playfield: Pf, power: u32, out: &mut Vec<u32>) {
    use lmoxide_content::roll::{Drop, Roller};
    let c = crate::gamedata::get();
    let t = &*TABLES;
    let table = |id: &str| c.loot.get(id).cloned();
    let in_worlds = |worlds: &[String]| worlds.iter().filter_map(|w| c.worlds.get(w)).any(|w| w.playfield == playfield.model());

    let mut random = random;
    let mut roller = Roller { table: &table, in_worlds: &in_worlds, orb_bonus, random: &mut random, power, rules: &c.tunables.loot.power, warnings: Vec::new() };
    let mut drops = Vec::new();
    roller.roll(id, &mut drops);
    for w in roller.warnings {
        tracing::warn!(table = id, "{w}");
    }
    for d in drops {
        match d {
            Drop::Star => out.push(t.star),
            Drop::Heart => out.push(t.health_orb),
            Drop::Potion => out.push(t.potion),
            Drop::Part(from) => out.extend(pick_from(&from, playfield).and_then(|m| TABLES.part_items.get(&m).and_then(|p| pick_weighted(p)))),
            Drop::Bag(MinifigureSource::World(_)) => out.extend(pick_bag(playfield)),
            Drop::Bag(from) => out.extend(pick_from(&from, playfield).and_then(|m| TABLES.minifigure_bag.get(&m).copied())),
            Drop::Chest(list) => {
                let chests: Vec<u32> = list.iter().filter_map(|c| template_of(c)).collect();
                out.extend(pick(&chests));
            }
            Drop::Item(i) => out.extend(template_of(&i)),
        }
    }
}

/// `!loot` kinds.
pub const TEST_KINDS: &[&str] = &["potion", "part", "bag", "chest", "memberchest", "boss", "enemy", "smashable"];

/// A test bag of `kind` (TEST_KINDS) in `playfield`, rolled at loot power `power`.
pub fn test_items(kind: &str, playfield: Pf, power: u32) -> Vec<u32> {
    let t = &*TABLES;
    match kind {
        "potion" => vec![t.potion],
        "part" => pick_part(playfield).into_iter().collect(),
        "bag" => pick_bag(playfield).into_iter().collect(),
        "chest" => pick(&t.chests).into_iter().collect(),
        "memberchest" => pick(&t.member_chests).into_iter().collect(),
        "boss" => roll(Source::Boss, 0, playfield, power),
        "enemy" => roll(Source::Enemy, 0, playfield, power),
        "smashable" => roll(Source::Smashable, 0, playfield, power),
        table => {
            let mut out = Vec::new();
            roll_table_into(table, 0, playfield, power, &mut out);
            out
        }
    }
}

/// What `items` are, for `!loot` and `!power`: "3 stars, 25 stars, health orb, part ...".
pub fn describe(items: &[u32]) -> String {
    let c = crate::gamedata::get();
    let mut stars = 0;
    let mut other: Vec<String> = Vec::new();
    for &i in items {
        match reward(i) {
            Reward::Stars(n) => stars += n,
            _ => other.push(c.items.values().find(|it| it.template == Some(i)).map_or_else(|| i.to_string(), |it| it.name.clone())),
        }
    }
    let mut parts = vec![format!("{stars} stars")];
    parts.extend(other);
    parts.join(", ")
}

/// Loot for a kill: a bag for the killer and one for each teammate in play nearby, each
/// rolled separately at the dropper's loot power `power` ([`npc_power`]).
/// `table`: the dropper's own loot table instead of `source`'s.
pub fn drop_loot(killer: Id32, playfield: Pf, pos: [f32; 3], dropper: Id32, source: Source, table: Option<&str>, power: u32) {
    let mut owners = vec![killer];
    for mate in crate::community::teammates(killer.instance) {
        let id = Id32::new(killer.kind, mate);
        if world::whereabouts(id).is_some_and(|(pf, p)| pf == playfield && distance(p, pos) <= crate::gamedata::get().tunables.loot.share_range) {
            owners.push(id);
        }
    }
    // inside a Lost Creations run enemies drop hearts only
    let lc_table = (source != Source::Smashable && crate::lost_creations::element(playfield).is_some()).then(|| crate::gamedata::get().tunables.lost_creations.enemy_loot.clone());
    let table = lc_table.as_deref().or(table);
    for owner in owners {
        let bonus = crate::procs::orb_bonus(owner);
        let items = match table {
            Some(t) => {
                let mut items = Vec::new();
                roll_table(t, bonus, playfield, power, &mut items);
                items
            }
            None => roll(source, bonus, playfield, power),
        };
        drop_items(owner, playfield, pos, dropper, items);
    }
}

/// An opened chest (item `template`, from bag `bag`) rolled at loot power `power`
/// ([`chest_power`]): a new bag at `pos`, the looter's position, as other loot (the user's
/// decision 2026-10-29: not at the chest's Loot_Offset, docs/TUNABLES.md).
pub fn open_chest(owner: Id32, playfield: Pf, pos: [f32; 3], bag: Id32, template: u32, member: bool, power: u32) {
    let source = if member { Source::MemberChest } else { Source::Chest };
    let items = roll(source, 0, playfield, power);
    info!(?owner, template, power, items = %describe(&items), "chest opened");
    drop_items(owner, playfield, pos, bag, items);
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

static BAGS: LazyLock<Mutex<HashMap<Id32, Bag>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_BAG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Drops `items` (templates) for `owner` at `pos`, sent to the owner only. `_dropper`: what
/// dropped them (live's bags named none).
pub fn drop_items(owner: Id32, playfield: Pf, pos: [f32; 3], _dropper: Id32, items: Vec<u32>) {
    if items.is_empty() {
        return;
    }
    let id = Id32::new(crate::gamedata::get().tunables.loot.bag_dynel_type, NEXT_BAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    info!(?owner, ?id, ?items, "loot dropped");
    let records = vec![
        create_game_object(id, playfield, pos),
        bag_record(id, playfield, pos, owner, &items),
        // as the original server sent after every bag: who may loot it (ID32)
        allowed_looter(id, owner),
    ];
    BAGS.lock().unwrap().insert(id, Bag { owner, public: false, playfield, pos, items: items.into_iter().map(Some).collect() });
    world::send_to(owner, world::Out::Records(records));
    // (unit tests that complete quests run without a runtime: no lifetime there)
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    rt.spawn(async move {
        let loot = &crate::gamedata::get().tunables.loot;
        let public_after = loot.bag_public_after.to_std();
        tokio::time::sleep(public_after).await;
        make_public(id);
        tokio::time::sleep(loot.bag_lifetime.to_std().saturating_sub(public_after)).await;
        remove(id);
    });
}

/// `NewAllowedLooterIIR_c`: who may loot `bag` ((0,0): everyone).
fn allowed_looter(bag: Id32, looter: Id32) -> Bytes {
    iir::record("NewAllowedLooterIIR_c", bag, false, |w| looter.write(w))
}

/// A bag nobody has looted from opens to everyone, as live did after 90 s.
fn make_public(bag: Id32) {
    let mut bags = BAGS.lock().unwrap();
    let Some(b) = bags.get_mut(&bag).filter(|b| b.items.iter().all(Option::is_some)) else { return };
    b.public = true;
    let owner = b.owner;
    drop(bags);
    world::send_to(owner, world::Out::Records(vec![allowed_looter(bag, Id32::new(0, 0))]));
}

/// `LootBagFullUpdateIIR_c` (ReadSubClass 0x123a0de): ProtoGameCodeDynelIIR, ProtoSimpleDynelIIR,
/// VisualData (u32 resId, u32 resType, u32 n + n × 5 u32, 6 × f32 bbox, f32 scale, u32, u8;
/// the rest can be zero, each item brings its own mesh), ProtoLootBagIIR. Item positions must be 0..n-1.
fn bag_record(id: Id32, playfield: Pf, pos: [f32; 3], owner: Id32, items: &[u32]) -> Bytes {
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield.key()),
        running_effects: vec![],
    };
    let id32 = |i: Id32| ProtoId32 { r#type: Some(i.kind), instance: Some(i.instance) };
    let bag = ProtoLootBagIir {
        cash: Some(0),
        owner_id: Some(id32(owner)),
        // live sent no dropper
        dropper_id: Some(id32(Id32::new(0, 0))),
        inventory: Some(ProtoInventoryGame {
            inventory_id: ProtoId32 { r#type: Some(BAG_INVENTORY_TYPE), instance: Some(id.instance) },
            max_size: Some(BAG_INVENTORY_SIZE),
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
    pub playfield: Pf,
    /// where the bag lies
    pub pos: [f32; 3],
}

/// The player loots item `pos` of `bag`. Returns the item, or `None` if the bag or item is gone
/// or isn't theirs. Confirms the pickup to the client.
pub fn take(player: Id32, bag: Id32, pos: u32) -> Option<Taken> {
    let mut bags = BAGS.lock().unwrap();
    let b = bags.get_mut(&bag)?;
    if b.owner != player && !b.public {
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
            tokio::time::sleep(EMPTIED_BAG_LINGER).await;
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
            let m = pick_minifigure(Pf::main(4200)).unwrap();
            assert!(pirate.contains(&m));
            let part = pick_part(Pf::main(4200)).unwrap();
            assert!(matches!(reward(part), Reward::Part(_)));
            assert!(matches!(reward(pick_bag(Pf::main(4200)).unwrap()), Reward::MinifigureBag(m) if pirate.contains(&m)));
        }
        assert!(pick_minifigure(Pf::main(1200)).is_none());
        // a pocket adventure drops the minifigures its window lists under "Parts for:" (Beach
        // Cave: Cyclops, Sea Captain); clearing it gives the one it shows (Lifeguard Guy)
        let c = crate::gamedata::get();
        let mut beach: Vec<u32> = TABLES.world_minifigures[&1642].iter().map(|&(_, m)| m).collect();
        beach.sort();
        let mut parts = vec![c.minifigures["minifig/cyclops"].template, c.minifigures["minifig/sea-captain"].template];
        parts.sort();
        assert_eq!(beach, parts);
        let lifeguard = c.minifigures["minifig/lifeguard-guy"].template;
        assert!(matches!(reward(pocket_reward(1642, |_| false).unwrap()), Reward::MinifigureBag(m) if m == lifeguard));
        // and only so: its parts and bag drop nowhere
        assert!(TABLES.world_minifigures.values().flatten().all(|&(_, m)| m != lifeguard));
        assert!(pocket_reward(4200, |_| false).is_none());
    }

    #[test]
    fn pocket_adventure_minifigures_drop_as_parts_only() {
        // Monkey Cave (parts: Hollywood Starlet, Saxophone Player): their parts drop, never a
        // whole bag, also not from a table's world bag (Monkey Kong's loot had Saxophone
        // Player's bag 358537)
        let c = crate::gamedata::get();
        let parts = [c.minifigures["minifig/hollywood-starlet"].template, c.minifigures["minifig/saxophone-player"].template];
        let cave = Pf::main(1643);
        for _ in 0..200 {
            assert!(pick_bag(cave).is_none());
            assert!(matches!(reward(pick_part(cave).unwrap()), Reward::Part(_)));
            for item in roll(Source::Boss, 0, cave, 1000) {
                assert!(!matches!(reward(item), Reward::MinifigureBag(_)), "a bag in the Monkey Cave");
            }
        }
        assert!(parts.iter().all(|m| TABLES.world_minifigures[&1643].iter().any(|&(_, w)| w == *m)));
    }

    #[test]
    fn a_story_conversation_gives_the_pocket_minifigure() {
        // Monkey Cave (Island Warrior, 774 `spawn SIKZ`), Dragoon Dwelling (Revolutionary
        // Soldier, 779 `spawn RG5K`), Jacket Cellar (Pizza Delivery Man, 788 `spawn 5WM9`): no
        // reward bag besides; Beach Cave's Lifeguard Guy still comes as one
        for pf in [1643, 1632, 1670] {
            assert!(pocket_reward(pf, |_| false).is_none(), "{pf}");
            // later clears neither (tunables `story.repeat_conversation_minifigures` off)
            assert!(pocket_reward(pf, |_| true).is_none(), "{pf}");
        }
        assert!(pocket_reward(1642, |_| false).is_some());
        // whether the conversation has played: the Island Warrior rescue gives tag 4141
        assert_eq!(story_gives(1643, "minifig/island-warrior", &|_| false), Some(false));
        assert_eq!(story_gives(1643, "minifig/island-warrior", &|t| t == 4141), Some(true));
        assert_eq!(story_gives(1642, "minifig/lifeguard-guy", &|_| true), None);
    }

    #[test]
    fn story_dungeons_drop_their_window_parts() {
        // the Ruined Fort's window: Rewards Jester, Parts for Judge (the user found King and
        // Jester parts in its chest); the Jester's parts drop nowhere, his story conversation
        // (`spawn GQKS`) gives him
        let c = crate::gamedata::get();
        let judge = c.minifigures["minifig/judge"].template;
        let jester = c.minifigures["minifig/jester"].template;
        let fort = Pf::main(5300);
        for _ in 0..200 {
            assert!(matches!(reward(pick_part(fort).unwrap()), Reward::Part(_)));
            assert_eq!(pick_minifigure(fort), Some(judge));
        }
        assert!(TABLES.world_minifigures.values().flatten().all(|&(_, m)| m != jester));
        assert!(pocket_reward(5300, |_| false).is_none());
        // the Sorcerer's Keep's Evil Wizard comes with the Medieval story's conclusion
        // (loot/story-medieval-conclusion), not also for clearing it
        assert!(pocket_reward(5310, |_| false).is_none());
    }

    #[test]
    fn live_bag_rates() {
        // about a quarter of regular kills and 71% of smashes drop a bag; regular bags hold
        // 1-3 single stars
        let n = 4000;
        let bags = |source| (0..n).filter(|_| !roll(source, 0, Pf::main(4200), 1000).is_empty()).count() as f32 / n as f32;
        assert!((0.21..0.30).contains(&bags(Source::Enemy)), "{}", bags(Source::Enemy));
        assert!((0.66..0.76).contains(&bags(Source::Smashable)), "{}", bags(Source::Smashable));
        for _ in 0..500 {
            let stars = roll(Source::Enemy, 0, Pf::main(4200), 1000).iter().filter(|&&i| matches!(reward(i), Reward::Stars(1))).count();
            assert!(stars <= 3);
        }
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
        // a boss's bag: 2 stars and an orb, usually a Member Super Chest (live)
        let boss = roll(Source::Boss, 0, Pf::main(4200), 1000);
        assert!(boss.iter().filter(|&&i| matches!(reward(i), Reward::Stars(1))).count() >= 2);
        assert!(boss.iter().filter(|&&i| matches!(reward(i), Reward::Chest { .. })).count() <= 2);
        // summoned adds drop nothing
        let mut none = Vec::new();
        roll_table_into("loot/none", 0, Pf::main(4200), 1000, &mut none);
        assert!(none.is_empty());
        assert_eq!(achievement_minifigures().count(), 1);
        assert_eq!(membership_minifigures().len(), 2);
    }
}

#[cfg(test)]
mod power_tests {
    #[test]
    fn power_level_is_capped_by_super_region() {
        assert_eq!(super::power_level(30, 4200), 1000, "Pirate");
        assert_eq!(super::power_level(5, 4200), 500);
        assert_eq!(super::power_level(30, 1643), 1000, "a Pirate micro-dungeon");
        assert_eq!(super::power_level(30, 2210), 2500, "Mythology");
        assert_eq!(super::power_level(30, 7001), 3000, "the Treasure Chamber isn't capped");
    }

    #[test]
    fn power_per_source() {
        use super::*;
        // chests: DoSuperChestPowerLevel (no client chest has its own LootPowerLevel)
        assert_eq!(chest_power(30, Pf::main(4200), 580_943), 1000);
        assert_eq!(chest_power(3, Pf::main(1643), 580_943), 300);
        assert_eq!(chest_power(30, Pf::main(7001), 531_717), 3000);
        // quest rewards: capped by the playfield's thousands, not the super-region; Dino's
        // math.max makes 3000 a floor
        assert_eq!(quest_power(30, 4200), 1000);
        assert_eq!(quest_power(30, 1643), 3000, "micro-dungeons aren't capped");
        assert_eq!(quest_power(12, 5200), 1200);
        assert_eq!(quest_power(30, 2210), 2500);
        assert_eq!(quest_power(5, 6200), 3000, "the Lua's Dino max()");
        assert_eq!(quest_power(40, 6200), 4000);
        // enemies: PowerLevel 499 when the template has one, else level x 100
        assert_eq!(npc_power(18, None), 1800);
        assert_eq!(npc_power(18, Some(250)), 250);
    }

    /// The live capture's chests and quest rewards at the powers they were opened at (levels
    /// 1-3 and 30, Pirate: 100-1000): the content tables at those powers give the same ranges.
    #[test]
    fn rolls_match_the_live_capture() {
        use super::*;
        let stars = |items: &[u32]| items.iter().map(|&i| if let Reward::Stars(n) = reward(i) { n } else { 0 }).sum::<u64>();
        for power in [100, 300, 1000] {
            for _ in 0..300 {
                let chest = roll(Source::MemberChest, 0, Pf::main(4200), power);
                assert!((85..=186).contains(&stars(&chest)), "member chest at {power}: {}", stars(&chest));
                let parts = chest.iter().filter(|&&i| matches!(reward(i), Reward::Part(_))).count();
                assert!((1..=2).contains(&parts));
                let mut quest = Vec::new();
                roll_table_into("loot/quest-normal", 0, Pf::main(4200), power, &mut quest);
                assert!((78..=79).contains(&stars(&quest)), "quest reward at {power}: {}", stars(&quest));
                assert!(quest.iter().any(|&i| matches!(reward(i), Reward::HealthOrb)));
                let mut micro = Vec::new();
                roll_table_into("loot/micro-dungeon-super-chest", 0, Pf::main(1643), 0, &mut micro);
                assert!(matches!(micro[..], [c] if matches!(reward(c), Reward::Chest { member: true })));
            }
        }
        // every quest reward item names a table that loads
        let c = crate::gamedata::get();
        for r in c.tunables.loot.quest_rewards.values() {
            assert!(c.loot.contains_key(&r.table), "{}", r.table);
        }
    }

    #[test]
    fn world_conclusions_give_a_member_chest_and_their_dungeon_minifigure() {
        use super::*;
        // the last story dungeon's teleport-window reward minifigure, as a bag (docs/TUNABLES.md)
        for (world, minifigure) in [("pirate", 576_081), ("medieval", 550_363), ("space", 357_945), ("mythology", 41_894), ("dino", 498_330)] {
            let mut items = Vec::new();
            roll_table_into(&format!("loot/story-{world}-conclusion"), 0, Pf::main(1200), 0, &mut items);
            assert_eq!(items.len(), 2, "{world}: {items:?}");
            assert!(items.iter().any(|&i| matches!(reward(i), Reward::Chest { member: true })), "{world}");
            assert!(items.iter().any(|&i| matches!(reward(i), Reward::MinifigureBag(m) if m == minifigure)), "{world}: {items:?}");
        }
    }
}
