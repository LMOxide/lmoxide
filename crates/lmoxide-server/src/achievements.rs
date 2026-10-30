//! Achievements: the client's lore tree tags under "Achievements" (content/achievements,
//! tools/re/extract_achievements.py). The server grants a tag with
//! `GameCharInterface::AddTag(uint32 tag, uint32 timestamp)`; the client then shows it, adds its
//! points and lists it as done. At every login live re-sent AddTag for each tag the character
//! had (recorded session, timestamp 0 as when they were earned).
//!
//! Counters (TagCounterStat) are stats the client shows as progress: the player's own
//! (TaCoPlayerNPCKills 3000116, TaCoSmashSmashies 3000126: `StatIIR_t`) and two on each minifigure
//! item (InventoryItemGeneric1/2: `SetItemStat(inventory, pos, stat, value)`, raised while that
//! minifigure is active). Recorded per enemy kill: SetItemStat 3000112 on the active minifigure,
//! StatIIR_t 3000116, then AddTag for what that completed.
//!
//! What earns the other achievements was server script; the rules here (`kind` in the data) are
//! inferred from their names. Most exploration, seasonal events and Lost Creations have no rule.
//! Rewards: the lore node's reward script (content `reward`) drops a loot bag at the player
//! from the "Stars Small/Medium/Large/Very Large" tables; achievements without one give their
//! AchievementRewardValue stars directly. The minifigure achievements' scripts also count them on
//! the minifigure item (InventoryItemGeneric3 3000114, `minifigure_counter`): the Heroic and
//! Legendary "Complete Any 5/10" go by that count, which is sent again at every login (as the
//! client's login fix script 3729 recomputed it).
//!
//! Which enemies count toward a smash achievement can be set by the world editor's smash rules
//! (approved world content, [`set_smash_rules`]): a defeat then counts only when the defeated one
//! is named in the rule, instead of the achievement's own match or target.
//!
//! Collectible counters ("A Nose for Gold": 3 goblets, ... [`collect_achievements`]) count
//! what the world editor says gives one: a trigger wall that collects it (once per wall per
//! player, `Event::Collected`), or defeating an enemy its rule names (each defeat, or once per
//! enemy name when the rule says `once`). A `drop:<name>` rule with an item template drops that
//! item from the enemies it names (a loot bag; picking it up gives nothing yet: there is no
//! mission item inventory).
//!
//! Every event also goes into the character's journal (`defeats`, `visits`, `finds`): what it
//! defeated and how often, the playfields it entered, the collectibles and places it found.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{LazyLock, RwLock},
};

use bytes::Bytes;
use lmoxide_net::{Endpoint, Writer, methods::game_char_interface, types::Id32};
use lmoxide_survey::SmashRule;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::store::{Character, Defeat, Visit};

const STAT_PLAYER_KILLS: u32 = 3_000_116;
const STAT_PLAYER_SMASHES: u32 = 3_000_126;
/// Minifigure item counters (InventoryItemGeneric1/2).
const STAT_MINIFIGURE_COUNTER1: u32 = 3_000_112;
const STAT_MINIFIGURE_COUNTER2: u32 = 3_000_113;
/// InventoryItemGeneric3: the minifigure's achievements done.
const STAT_MINIFIGURE_ACHIEVEMENTS: u32 = 3_000_114;

#[derive(Debug, Deserialize)]
struct Achievement {
    id: u32,
    parent: u32,
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    invisible: bool,
    #[serde(default)]
    stars: u64,
    /// The reward loot table (`loot/...`), instead of `stars`.
    #[serde(default)]
    reward: Option<String>,
    #[serde(default)]
    minifigure: Option<u32>,
    #[serde(default)]
    minifigure_counter: bool,
    kind: String,
    /// counter: [stat, target]
    #[serde(default)]
    counter: Option<[u64; 2]>,
    #[serde(default)]
    counts: Option<Counts>,
    /// any / upgrade
    #[serde(default)]
    n: u32,
    #[serde(default)]
    series: Option<u32>,
    #[serde(default)]
    target: Option<String>,
    /// find: the world object's name, and its playfield when the name isn't unique
    #[serde(default)]
    find: Option<String>,
    #[serde(default)]
    playfield: Option<u32>,
    #[serde(default)]
    children: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct Counts {
    /// kills, smashes, builds or kills_or_smashes
    what: String,
    #[serde(rename = "match")]
    matching: Option<String>,
}

#[derive(Deserialize)]
struct Data {
    achievements: Vec<Achievement>,
    /// The client's playfield names (TDB 52000).
    playfields: HashMap<u32, String>,
}

struct Tree {
    all: Vec<Achievement>,
    by_id: HashMap<u32, usize>,
    children: HashMap<u32, Vec<u32>>,
    /// Playfield names for dungeon targets, lowercase: the content tree's worlds' (display name,
    /// else the playfield name), else the client's achievement playfield names.
    playfields: HashMap<u32, String>,
}

static TREE: LazyLock<Tree> = LazyLock::new(|| {
    let data: Data = serde_json::from_value(lmoxide_content::compat::achievements_file(crate::gamedata::get())).expect("achievements");
    let by_id = data.achievements.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for a in &data.achievements {
        children.entry(a.parent).or_default().push(a.id);
    }
    let mut playfields: HashMap<u32, String> = data.playfields.iter().map(|(&k, v)| (k, v.to_lowercase())).collect();
    // the content tree's worlds: their display name, else the client's playfield name
    playfields.extend(crate::gamedata::get().worlds.values().map(|w| (w.playfield, w.display_name.as_ref().unwrap_or(&w.name).to_lowercase())));
    Tree { all: data.achievements, by_id, children, playfields }
});

/// One smash achievement as the world editor lists it (survey.rs): a `counter` of kills or
/// smashes, or a `defeat`. The per-minifigure copies of one achievement ("Smash 50 Witches as
/// Judge", "... as Cyclops") are one entry.
#[derive(Debug, Clone, Serialize)]
pub struct SmashAchievement {
    /// Its id, or `minifigure:<what>:<match>:<n>` for the per-minifigure copies.
    pub key: String,
    pub ids: Vec<u32>,
    /// The copies' shared title part ("Witch Hunter").
    pub title: String,
    /// The copies' text without " as <minifigure>".
    pub text: String,
    /// The parent category's title ("Minifigures" for the per-minifigure copies).
    pub group: Option<String>,
    /// counter, defeat or collect ([`collect_achievements`]; the survey server adds find,
    /// category and drop)
    pub kind: &'static str,
    /// kills, smashes or kills_or_smashes (counters)
    pub what: Option<String>,
    /// What the defeated one's name must contain (counters), or the defeat target.
    #[serde(rename = "match")]
    pub matching: Option<String>,
    /// How many (1 for a defeat).
    pub n: u32,
    /// How many per-minifigure copies (0: not one of those).
    pub minifigures: u32,
}

/// Every smash achievement (counters of kills or smashes, defeats), in the data's order.
pub fn smash_achievements() -> &'static [SmashAchievement] {
    static LIST: LazyLock<Vec<SmashAchievement>> = LazyLock::new(|| {
        let title_of = |id: u32| TREE.by_id.get(&id).and_then(|&i| TREE.all[i].title.clone());
        let mut list: Vec<SmashAchievement> = Vec::new();
        for a in &TREE.all {
            let (kind, what, matching, n) = match (a.kind.as_str(), &a.counts, a.counter) {
                ("counter", Some(k), Some([_, n])) if k.what != "builds" => ("counter", k.what.clone(), k.matching.clone(), n as u32),
                ("counter", None, Some([stat, n])) if stat == STAT_PLAYER_KILLS as u64 => ("counter", "kills".into(), None, n as u32),
                ("counter", None, Some([stat, n])) if stat == STAT_PLAYER_SMASHES as u64 => ("counter", "smashes".into(), None, n as u32),
                ("defeat", _, _) => ("defeat", String::new(), a.target.clone(), 1),
                _ => continue,
            };
            let title = a.title.clone().unwrap_or_else(|| a.name.clone());
            let text = a.text.clone().unwrap_or_default();
            let what = (!what.is_empty()).then_some(what);
            if a.minifigure.is_none() || kind != "counter" {
                list.push(SmashAchievement { key: a.id.to_string(), ids: vec![a.id], title, text, group: title_of(a.parent), kind, what, matching, n, minifigures: 0 });
                continue;
            }
            let key = format!("minifigure:{}:{}:{n}", what.as_deref().unwrap_or(""), matching.as_deref().unwrap_or("").to_lowercase());
            if let Some(e) = list.iter_mut().find(|e| e.key == key) {
                e.ids.push(a.id);
                e.minifigures += 1;
                continue;
            }
            // "Judge, the Witch Hunter", "Barbarian the Dragontamer"
            let shared = title.split_once(", ").map(|(_, t)| t).or_else(|| title.rfind(" the ").map(|i| &title[i + 1..])).unwrap_or(&title);
            let shared = shared.strip_prefix("the ").unwrap_or(shared).to_string();
            // "Smash 50 Witches as Judge."
            let text = text.rsplit_once(" as ").map_or(text.clone(), |(t, _)| format!("{t}."));
            list.push(SmashAchievement { key, ids: vec![a.id], title: shared, text, group: Some("Minifigures".into()), kind, what, matching, n, minifigures: 1 });
        }
        list
    });
    &LIST
}

/// The collectible counters ("Pirate_Collectibles_Goblets": 3 goblets, ...): counters of their
/// own stat (not kills, smashes or builds), not the global or holiday ones. Kind `collect`, `n`
/// the counter's target.
pub fn collect_achievements() -> &'static [SmashAchievement] {
    static LIST: LazyLock<Vec<SmashAchievement>> = LazyLock::new(|| {
        let title_of = |id: u32| TREE.by_id.get(&id).and_then(|&i| TREE.all[i].title.clone());
        TREE.all
            .iter()
            .filter(|a| a.kind == "counter" && a.counts.is_none() && a.minifigure.is_none())
            .filter(|a| !a.name.starts_with("Global_") && !a.name.contains("Holidays"))
            .filter_map(|a| {
                let [stat, n] = a.counter?;
                (stat != STAT_PLAYER_KILLS as u64 && stat != STAT_PLAYER_SMASHES as u64).then(|| SmashAchievement {
                    key: a.id.to_string(),
                    ids: vec![a.id],
                    title: a.title.clone().unwrap_or_else(|| a.name.clone()),
                    text: a.text.clone().unwrap_or_default(),
                    group: title_of(a.parent),
                    kind: "collect",
                    what: None,
                    matching: None,
                    n: n as u32,
                    minifigures: 0,
                })
            })
            .collect()
    });
    &LIST
}

/// The counter stat of collect achievement `key` (its id).
fn collect_stat(key: &str) -> Option<u32> {
    let a = collect_achievements().iter().find(|a| a.key == key)?;
    let [stat, _] = TREE.all[TREE.by_id[&a.ids[0]]].counter?;
    Some(stat as u32)
}

/// Achievement id -> the enemies its approved, active smash rule names (lowercase).
static RULES: LazyLock<RwLock<HashMap<u32, Vec<String>>>> = LazyLock::new(Default::default);

/// A collect achievement's or a `drop:<name>` rule: which enemies give one.
#[derive(Debug, Clone)]
struct DropRule {
    key: String,
    /// lowercase, categories expanded
    enemies: Vec<String>,
    once: bool,
    item: Option<u32>,
}

static DROPS: LazyLock<RwLock<Vec<DropRule>>> = LazyLock::new(Default::default);
/// The smash rules as last set (quests expand `@<category>` with them).
static ALL_RULES: LazyLock<RwLock<Vec<SmashRule>>> = LazyLock::new(Default::default);

/// Enemy names with `@<category>` replaced by the category's enemies (quest goals).
pub fn expand_enemies(enemies: &[String]) -> Vec<String> {
    lmoxide_survey::expand_enemies(enemies, &ALL_RULES.read().unwrap())
}

/// The smash rules to count by (the approved, active ones; content.rs). Rules without enemies,
/// and those of discover locations, change nothing. `@<category>` in a rule's enemies stands
/// for the enemies of the rule `category:<category>` (one level; unknown: none).
pub fn set_smash_rules(rules: &[SmashRule]) {
    let mut by_id = HashMap::new();
    let mut drops = Vec::new();
    for r in rules.iter().filter(|r| !r.enemies.is_empty()) {
        let enemies: Vec<String> = lmoxide_survey::expand_enemies(&r.enemies, rules).iter().map(|e| e.to_lowercase()).collect();
        if r.id.starts_with(lmoxide_survey::DROP_PREFIX) || collect_stat(&r.id).is_some() {
            drops.push(DropRule { key: r.id.clone(), enemies, once: r.once, item: r.item });
            continue;
        }
        let Some(a) = smash_achievements().iter().find(|a| a.key == r.id) else { continue };
        for &id in &a.ids {
            by_id.insert(id, enemies.clone());
        }
    }
    *RULES.write().unwrap() = by_id;
    *DROPS.write().unwrap() = drops;
    *ALL_RULES.write().unwrap() = rules.to_vec();
}

/// The item template a `drop:<name>` rule drops, if it says.
fn drop_item(key: &str) -> Option<u32> {
    DROPS.read().unwrap().iter().find(|d| d.key == key).and_then(|d| d.item)
}

/// A trigger wall that collects `key` (walls.rs) was crossed by `player`: counted in their
/// session (`Event::Collected`; once per wall and player, whatever the wall's `once`).
pub fn wall_collect(player: Id32, key: Option<&str>, wall: &str) {
    match key.filter(|k| !k.is_empty()) {
        Some(key) => crate::world::send_to(player, crate::world::Out::Progress(Event::Collected { key: key.into(), wall: wall.into() })),
        None => info!(wall, "trigger wall: collects nothing (no rule key set)"),
    }
}

/// One collectible (or drop) of `key` for the character, from `from` (`wall:<id>`,
/// `enemy:<name>`); `from` is remembered in the journal's finds when `remember`, and then gives
/// nothing again.
fn collect(c: &mut Character, ctx: &Ctx, out: &mut Output, key: &str, from: &str, remember: bool, dropper: Id32) {
    let seen = format!("collect:{key}:{from}");
    if remember {
        if c.finds.contains_key(&seen) {
            return;
        }
        c.finds.insert(seen, unix_now());
    }
    if let Some(stat) = collect_stat(key) {
        info!(character = ctx.character, key, from, "collected");
        bump(c, ctx, out, stat, 0, 1);
    } else if let Some(item) = drop_item(key) {
        info!(character = ctx.character, key, from, item, "drop");
        out.drops.push((dropper, item));
    } else {
        info!(character = ctx.character, key, from, "collect: no such collect achievement, or the drop names no item");
    }
}

/// Whether achievement `id`'s smash rule names `name` (lowercase); `None`: it has no rule.
fn rule_names(id: u32, name: &str) -> Option<bool> {
    RULES.read().unwrap().get(&id).map(|enemies| enemies.iter().any(|e| same_name(e, name)))
}

/// Whether a kill or smash raises the player's own counter `stat`. The counter is shared by its
/// achievements ("Smash 1000 Monsters", "... 10000 ..."): when any of them has a smash rule,
/// only the enemies of their rules count.
fn player_counts(stat: u32, name: &str) -> bool {
    let rules: Vec<bool> = TREE
        .all
        .iter()
        .filter(|a| a.kind == "counter" && a.minifigure.is_none() && a.counter.is_some_and(|[s, _]| s == stat as u64))
        .filter_map(|a| rule_names(a.id, name))
        .collect();
    rules.is_empty() || rules.contains(&true)
}

/// Something a player did that achievements count.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Defeated an enemy of `level` (`boss`: a boss, `champion`: a champion) or smashed a
    /// smashable.
    Defeated { id: Id32, name: String, smashable: bool, boss: bool, champion: bool, level: u32, playfield: u32 },
    /// Finished building a buildie.
    Built { name: String },
    /// A server script gave the player tag `id` (NPC dialogues' `addtagtoplayer`).
    Tag(u32),
    /// Entered a playfield.
    Visited { playfield: u32 },
    /// Found a world object (a collectible, a boombox).
    Found { playfield: u32, name: String },
    /// Defeated every enemy of a dungeon in one visit.
    Cleared { playfield: u32 },
    /// Crossed trigger wall `wall` that collects `key` (a collect achievement's id, or a
    /// `drop:<name>` rule's key).
    Collected { key: String, wall: String },
    /// Completed level `level` of the Lost Creations (lost_creations.rs, `!lc`): it counts for
    /// `Ctx::lost_creations_team`.
    LostCreations { level: u32 },
}

/// What the session must send after an update.
#[derive(Default)]
pub struct Output {
    /// RPCs (endpoint, method, args).
    pub rpcs: Vec<(Endpoint, u32, Bytes)>,
    /// IIR records for the player.
    pub records: Vec<Bytes>,
    /// Stars rewarded (already added to the character).
    pub stars: u64,
    /// Reward loot tables to drop at the player (`loot/...`).
    pub loot: Vec<String>,
    /// Items to drop for the player (dropper, item template): `drop:<name>` rules.
    pub drops: Vec<(Id32, u32)>,
}

/// The character's view the rules need.
pub struct Ctx<'a> {
    pub character: u32,
    /// The active minifigure.
    pub active: Option<u32>,
    /// The minifigures a completed Lost Creations level counts for (tunables
    /// `lost_creations.achievement_minifigs`).
    pub lost_creations_team: &'a [u32],
    /// The minifigures a defeated boss or conquered dungeon counts for (tunables
    /// `achievements.target_minifigs`).
    pub target_team: &'a [u32],
    /// Where a minifigure item is (inventory, position), for SetItemStat.
    pub locate: &'a dyn Fn(u32) -> (u32, u32),
    /// How many upgrades were bought for a minifigure.
    pub upgrades: &'a dyn Fn(u32) -> u32,
    /// Whether a minifigure is owned.
    pub owned: &'a dyn Fn(u32) -> bool,
}

fn player_id(ctx: &Ctx) -> Id32 {
    Id32::new(crate::territory::CHARACTER_TYPE, ctx.character)
}

/// Login: every earned tag and the counters again, then whatever the character qualifies for
/// already (collections, minifigure levels).
pub fn login(c: &mut Character, ctx: &Ctx) -> Output {
    let mut out = Output::default();
    for &tag in c.tags.keys() {
        out.rpcs.push(add_tag(tag));
    }
    for (&stat, values) in &c.counters {
        for (&template, &value) in values {
            push_counter(&mut out, ctx, stat, template, value);
        }
    }
    for (template, n) in minifigure_achievements(c) {
        push_counter(&mut out, ctx, STAT_MINIFIGURE_ACHIEVEMENTS, template, n);
    }
    evaluate(c, ctx, &mut out);
    out
}

/// Each minifigure's achievements done that its counter counts (InventoryItemGeneric3).
fn minifigure_achievements(c: &Character) -> BTreeMap<u32, u64> {
    let mut n = BTreeMap::new();
    for id in c.tags.keys() {
        if let Some(&i) = TREE.by_id.get(id)
            && TREE.all[i].minifigure_counter
            && let Some(t) = TREE.all[i].minifigure
        {
            *n.entry(t).or_default() += 1;
        }
    }
    n
}

/// Grants what holds now (after a minifigure rank-up, ...).
pub fn check(c: &mut Character, ctx: &Ctx) -> Output {
    let mut out = Output::default();
    evaluate(c, ctx, &mut out);
    out
}

/// Counts `event` (`times` times) and grants what it completes.
pub fn on_event(c: &mut Character, ctx: &Ctx, event: &Event, times: u64) -> Output {
    let mut out = Output::default();
    let now = unix_now();
    match event {
        Event::Defeated { name, smashable, boss, champion, .. } => {
            let kind = match (smashable, boss, champion) {
                (true, ..) => "smashable",
                (_, true, _) => "boss",
                (_, _, true) => "champion",
                _ => "enemy",
            };
            let d = c.defeats.entry(name.clone()).or_insert_with(|| Defeat { kind: kind.into(), count: 0, first_at: now, last_at: now });
            d.count += times;
            d.last_at = now;
            // collectibles and drops the editor's rules give for this enemy
            let lower = name.to_lowercase();
            let id = match event {
                Event::Defeated { id, .. } => *id,
                _ => unreachable!(),
            };
            let hits: Vec<DropRule> = DROPS.read().unwrap().iter().filter(|d| d.enemies.iter().any(|e| same_name(e, &lower))).cloned().collect();
            for d in hits {
                for _ in 0..if d.once { 1 } else { times } {
                    collect(c, ctx, &mut out, &d.key, &format!("enemy:{lower}"), d.once, id);
                }
            }
        }
        Event::Collected { key, wall } => {
            collect(c, ctx, &mut out, key, &format!("wall:{wall}"), true, player_id(ctx));
            evaluate(c, ctx, &mut out);
            return out;
        }
        Event::Visited { playfield } => {
            let v = c.visits.entry(*playfield).or_insert(Visit { count: 0, first_at: now, last_at: now });
            v.count += 1;
            v.last_at = now;
            evaluate(c, ctx, &mut out);
            return out;
        }
        Event::Found { playfield, name } => {
            let key = format!("{playfield}:{name}");
            if c.finds.contains_key(&key) {
                return out;
            }
            info!(character = ctx.character, find = %key, "found");
            c.finds.insert(key, now);
            evaluate(c, ctx, &mut out);
            return out;
        }
        Event::Cleared { playfield } => {
            let area = TREE.playfields.get(playfield).cloned().unwrap_or_default();
            info!(character = ctx.character, playfield, %area, "cleared");
            let hits: Vec<u32> = TREE
                .all
                .iter()
                .filter(|a| a.kind == "clear" && a.target.as_ref().is_some_and(|t| !area.is_empty() && same_name(t, &area)))
                .map(|a| a.id)
                .collect();
            // "Conquer the <dungeon> as <minifigure>"
            let hits = hits.into_iter().chain(as_minifigure(ctx, &area, ""));
            for id in hits.collect::<Vec<_>>() {
                grant(c, ctx, &mut out, id);
            }
            evaluate(c, ctx, &mut out);
            return out;
        }
        Event::LostCreations { level } => {
            info!(character = ctx.character, level, minifigures = ?ctx.lost_creations_team, "lost creations level");
            let hits: Vec<u32> = TREE
                .all
                .iter()
                .filter(|a| a.kind == "lost_creations" && a.minifigure.is_some_and(|m| ctx.lost_creations_team.contains(&m)) && a.n <= *level)
                .map(|a| a.id)
                .collect();
            for id in hits {
                grant(c, ctx, &mut out, id);
            }
            evaluate(c, ctx, &mut out);
            return out;
        }
        Event::Built { .. } | Event::Tag(_) => {}
    }
    if let Event::Tag(id) = event {
        if TREE.by_id.contains_key(id) {
            grant(c, ctx, &mut out, *id);
        } else if !c.tags.contains_key(id) {
            // a plain tag (story progress), not an achievement
            c.tags.insert(*id, now);
            out.rpcs.push(add_tag(*id));
        }
        return out;
    }
    let (name, kind) = match event {
        Event::Defeated { name, smashable: false, .. } => (name, "kills"),
        Event::Defeated { name, smashable: true, .. } => (name, "smashes"),
        Event::Built { name } => (name, "builds"),
        _ => unreachable!(),
    };
    let lower = name.to_lowercase();
    match kind {
        "kills" if player_counts(STAT_PLAYER_KILLS, &lower) => bump(c, ctx, &mut out, STAT_PLAYER_KILLS, 0, times),
        "smashes" if player_counts(STAT_PLAYER_SMASHES, &lower) => bump(c, ctx, &mut out, STAT_PLAYER_SMASHES, 0, times),
        _ => {}
    }
    if let Some(active) = ctx.active {
        // the active minifigure's two item counters, by what its achievements count
        for stat in [STAT_MINIFIGURE_COUNTER1, STAT_MINIFIGURE_COUNTER2] {
            let counts = TREE.all.iter().any(|a| {
                a.minifigure == Some(active)
                    && a.counter.is_some_and(|[s, _]| s == stat as u64)
                    && a.counts.as_ref().is_some_and(|k| counts_event(a.id, k, kind, &lower))
            });
            if counts {
                bump(c, ctx, &mut out, stat, active, times);
            }
        }
    }
    if let Event::Defeated { boss, champion, playfield, .. } = event {
        let big = *boss || *champion;
        let dungeon = if big { TREE.playfields.get(playfield).cloned().unwrap_or_default() } else { String::new() };
        // a defeat achievement with a smash rule counts the rule's enemies, bosses or not
        let hits: Vec<u32> = TREE
            .all
            .iter()
            .filter(|a| {
                a.kind == "defeat"
                    && rule_names(a.id, &lower).unwrap_or_else(|| big && a.target.as_ref().is_some_and(|t| same_name(t, &lower)))
            })
            .map(|a| a.id)
            .chain(if big { as_minifigure(ctx, &dungeon, &lower) } else { Vec::new() })
            .collect();
        for id in hits {
            grant(c, ctx, &mut out, id);
        }
    }
    evaluate(c, ctx, &mut out);
    out
}

/// The "<minifigure>_<target>" achievements of `Ctx::target_team` for the dungeon `area` or the
/// boss `name` (either may be empty).
fn as_minifigure(ctx: &Ctx, area: &str, name: &str) -> Vec<u32> {
    TREE.all
        .iter()
        .filter(|a| {
            a.kind == "target"
                && a.minifigure.is_some_and(|m| ctx.target_team.contains(&m))
                && a.target.as_ref().is_some_and(|t| (!name.is_empty() && same_name(t, name)) || (!area.is_empty() && same_name(t, area)))
        })
        .map(|a| a.id)
        .collect()
}

/// Whether achievement `id` (counting `k`) counts a `kind` event of `name` (lowercase): by its
/// smash rule if it has one, else by its match.
fn counts_event(id: u32, k: &Counts, kind: &str, name: &str) -> bool {
    let kind_ok = k.what == kind || (k.what == "kills_or_smashes" && kind != "builds");
    kind_ok && rule_names(id, name).unwrap_or_else(|| k.matching.as_ref().is_none_or(|m| name.contains(&m.to_lowercase())))
}

/// Names compare without case and a leading "the " ("Defeat the Alien Queen"); `name` is
/// lowercase.
pub fn same_name(target: &str, name: &str) -> bool {
    let t = target.to_lowercase();
    let t = t.strip_prefix("the ").unwrap_or(&t);
    let n = name.strip_prefix("the ").unwrap_or(name);
    !t.is_empty() && t == n
}

fn bump(c: &mut Character, ctx: &Ctx, out: &mut Output, stat: u32, template: u32, times: u64) {
    let v = c.counters.entry(stat).or_default().entry(template).or_insert(0);
    *v += times;
    let v = *v;
    push_counter(out, ctx, stat, template, v);
}

fn push_counter(out: &mut Output, ctx: &Ctx, stat: u32, template: u32, value: u64) {
    let value = value.min(i32::MAX as u64) as u32;
    if template == 0 {
        out.records.push(crate::game::stat_record(player_id(ctx), stat, value));
    } else {
        // SetItemStat(const ID32& inventory, int pos, int stat, int value)
        let (inv, pos) = (ctx.locate)(template);
        let args = Id32::new(inv, ctx.character).write(Writer::new()).i32(pos as i32).i32(stat as i32).i32(value as i32).finish();
        out.rpcs.push((Endpoint::GameCharInterface, game_char_interface::SET_ITEM_STAT, args));
    }
}

pub fn add_tag(tag: u32) -> (Endpoint, u32, Bytes) {
    // AddTag(uint32 tagID, uint32 timestamp): live sent 0
    (Endpoint::GameCharInterface, game_char_interface::ADD_TAG, Writer::new().u32(tag).u32(0).finish())
}

pub fn remove_tag(tag: u32) -> (Endpoint, u32, Bytes) {
    (Endpoint::GameCharInterface, game_char_interface::REMOVE_TAG, Writer::new().u32(tag).finish())
}

/// Whether tag `id` is an achievement (other tags are story and tutorial progress).
pub fn is_achievement(id: u32) -> bool {
    TREE.by_id.contains_key(&id)
}

/// Takes achievements away (testing, or a moderator's fix): `which` (none: all of them), and the
/// earned "all of" achievements above them. Other tags (story and tutorial progress) stay, and so
/// do stars and XP already given. So that the next event doesn't give them straight back, a
/// removed `find` achievement's finds go too, and removing all clears the journal (finds,
/// defeats, visits) and the achievement counters. Returns the removed ids and the RemoveTag RPCs
/// for a character online (counters show again after its next login).
pub fn remove(c: &mut Character, which: Option<&[u32]>) -> (Vec<u32>, Vec<(Endpoint, u32, Bytes)>) {
    let mut gone: Vec<u32> = match which {
        None => c.tags.keys().copied().filter(|t| is_achievement(*t)).collect(),
        Some(ids) => ids.iter().copied().filter(|t| is_achievement(*t) && c.tags.contains_key(t)).collect(),
    };
    if which.is_some() {
        // the earned "all of" achievements above them aren't done any more
        let mut i = 0;
        while i < gone.len() {
            let parent = TREE.all[TREE.by_id[&gone[i]]].parent;
            if let Some(&p) = TREE.by_id.get(&parent)
                && TREE.all[p].kind == "all"
                && c.tags.contains_key(&parent)
                && !gone.contains(&parent)
            {
                gone.push(parent);
            }
            i += 1;
        }
    }
    for id in &gone {
        c.tags.remove(id);
        let a = &TREE.all[TREE.by_id[id]];
        if let Some(stat) = collect_stat(&id.to_string()) {
            // a collectible counter starts again
            c.counters.remove(&stat);
            let prefix = format!("collect:{id}:");
            c.finds.retain(|key, _| !key.starts_with(&prefix));
        }
        if a.kind == "find"
            && let Some(name) = &a.find
        {
            c.finds.retain(|key, _| match (a.playfield, key.split_once(':')) {
                (Some(pf), Some((kpf, kname))) => !(kname == name && kpf == pf.to_string()),
                (None, Some((_, kname))) => kname != name,
                _ => true,
            });
        }
    }
    if which.is_none() {
        c.finds.clear();
        c.defeats.clear();
        c.visits.clear();
        c.counters.clear();
    }
    gone.sort();
    let rpcs = gone.iter().map(|&id| remove_tag(id)).collect();
    (gone, rpcs)
}

fn grant(c: &mut Character, _ctx: &Ctx, out: &mut Output, id: u32) {
    if c.tags.contains_key(&id) {
        return;
    }
    let Some(&i) = TREE.by_id.get(&id) else { return };
    let a = &TREE.all[i];
    c.tags.insert(id, unix_now());
    out.rpcs.push(add_tag(id));
    if let Some(table) = &a.reward {
        // a loot bag at the player (the reward script's spawnloot)
        out.loot.push(table.clone());
    } else if a.stars > 0 {
        c.stars += a.stars;
        out.stars += a.stars;
    }
    if a.minifigure_counter
        && let Some(t) = a.minifigure
    {
        let n = minifigure_achievements(c).get(&t).copied().unwrap_or(0);
        push_counter(out, _ctx, STAT_MINIFIGURE_ACHIEVEMENTS, t, n);
    }
    info!(character = _ctx.character, achievement = id, name = %a.name, stars = a.stars, reward = ?a.reward, "achievement");
}

/// Grants every achievement whose rule holds now, until nothing changes (parents follow their
/// children).
fn evaluate(c: &mut Character, ctx: &Ctx, out: &mut Output) {
    loop {
        let mut new = Vec::new();
        for a in &TREE.all {
            if c.tags.contains_key(&a.id) || a.invisible {
                continue;
            }
            let done = match a.kind.as_str() {
                "counter" => a.counter.is_some_and(|[stat, target]| {
                    let template = if matches!(stat as u32, STAT_MINIFIGURE_COUNTER1 | STAT_MINIFIGURE_COUNTER2) {
                        a.minifigure.unwrap_or(0)
                    } else {
                        0
                    };
                    counter(&c.counters, stat as u32, template) >= target
                }),
                // "Get the third <minifigure> upgrade"
                "upgrade" => a.minifigure.is_some_and(|t| (ctx.owned)(t) && (ctx.upgrades)(t) >= a.n),
                "series" => series_complete(a.series, ctx),
                // the other "Complete Any" ones count too: "Complete Any 10" says "any 11", which
                // is 10 others and "Complete Any 5"
                "any" => {
                    // a minifigure's Heroic/Legendary: its achievements its counter counts
                    // (InventoryItemGeneric3; the Lua adds the tags at 5 and 10, the data's
                    // "any 11" counts Heroic too); others: any done sibling
                    let counted = |s: u32| TREE.by_id.get(&s).is_some_and(|&i| TREE.all[i].minifigure_counter || TREE.all[i].kind == "any");
                    let by_counter = TREE.children.get(&a.parent).is_some_and(|s| s.iter().any(|&s| TREE.by_id.get(&s).is_some_and(|&i| TREE.all[i].minifigure_counter)));
                    let siblings = TREE.children.get(&a.parent).map_or(0, |s| {
                        s.iter().filter(|&&s| s != a.id && c.tags.contains_key(&s) && (!by_counter || counted(s))).count()
                    });
                    siblings >= a.n as usize
                }
                "all" => !a.children.is_empty() && a.children.iter().all(|ch| c.tags.contains_key(ch)),
                "find" => a.find.as_ref().is_some_and(|name| {
                    c.finds.keys().any(|k| {
                        k.split_once(':').is_some_and(|(pf, n)| n == name && a.playfield.is_none_or(|p| pf == p.to_string()))
                    })
                }),
                _ => false,
            };
            if done {
                new.push(a.id);
            }
        }
        if new.is_empty() {
            break;
        }
        for id in new {
            grant(c, ctx, out, id);
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn kind_of(id: u32) -> &'static str {
    TREE.by_id.get(&id).map_or("", |&i| TREE.all[i].kind.as_str())
}

fn counter(counters: &BTreeMap<u32, BTreeMap<u32, u64>>, stat: u32, template: u32) -> u64 {
    counters.get(&stat).and_then(|m| m.get(&template)).copied().unwrap_or(0)
}

/// Every minifigure of `series` (all minifigures when `None`) owned.
fn series_complete(series: Option<u32>, ctx: &Ctx) -> bool {
    let figures = crate::collection::all_minifigures();
    let in_series: Vec<u32> = figures.into_iter().filter(|&t| series.is_none_or(|s| crate::combat::series(t) == Some(s))).collect();
    !in_series.is_empty() && in_series.iter().all(|&t| (ctx.owned)(t))
}

/// The champions of the world a side dungeon (a "Combat_PA_Clear the X" target) belongs to,
/// by their achievements ("Pirate_Combat_Champion_<name>"); empty for other playfields.
pub fn champions_for(playfield: u32) -> Vec<String> {
    let Some(area) = TREE.playfields.get(&playfield) else { return vec![] };
    let Some(world) = TREE
        .all
        .iter()
        .find(|a| a.kind == "clear" && a.target.as_ref().is_some_and(|t| same_name(t, area)))
        .and_then(|a| a.name.split('_').next())
    else {
        return vec![];
    };
    let prefix = format!("{world}_Combat_Champion_");
    TREE.all.iter().filter(|a| a.name.starts_with(&prefix)).filter_map(|a| a.target.clone()).collect()
}

/// The enemies achievements name (`kind` "champion" or "boss", the playfields they appear in):
/// the champions (named after their achievements' targets, "Uggluk Gatekeeper", in the side
/// dungeons of their world, `champions_for`) and the targets of defeat and per-minifigure target
/// achievements that aren't places.
pub fn achievement_enemies() -> Vec<(String, &'static str, Vec<u32>)> {
    let mut pfs: Vec<u32> = TREE.playfields.keys().copied().collect();
    pfs.sort();
    let by_pf: Vec<(u32, Vec<String>)> = pfs.into_iter().map(|pf| (pf, champions_for(pf))).filter(|(_, c)| !c.is_empty()).collect();
    let places: std::collections::HashSet<&str> = TREE.playfields.values().map(String::as_str).collect();
    let mut out: Vec<(String, &'static str, Vec<u32>)> = Vec::new();
    for a in &TREE.all {
        let Some(target) = a.target.as_ref().filter(|t| !t.is_empty()) else { continue };
        let champion = a.name.contains("_Combat_Champion_");
        if !champion && (a.kind != "defeat" && a.kind != "target" || places.contains(target.to_lowercase().as_str())) {
            continue;
        }
        if out.iter().any(|(n, ..)| n == target) {
            continue;
        }
        let playfields = by_pf.iter().filter(|(_, c)| c.contains(target)).map(|(pf, _)| *pf).collect();
        out.push((target.clone(), if champion { "champion" } else { "boss" }, playfields));
    }
    // a target achievement's enemy listed before its champion achievement: still a champion
    for (pf, names) in &by_pf {
        for n in names {
            if let Some(e) = out.iter_mut().find(|(x, ..)| x == n) {
                e.1 = "champion";
                if !e.2.contains(pf) {
                    e.2.push(*pf);
                }
            }
        }
    }
    out
}

/// A minifigure item's achievement counters (item stat, value), for every copy of the item.
pub fn item_counters(c: &Character, template: u32) -> Vec<(u32, u32)> {
    [STAT_MINIFIGURE_COUNTER1, STAT_MINIFIGURE_COUNTER2]
        .into_iter()
        .map(|stat| (stat, counter(&c.counters, stat, template)))
        .filter(|&(_, v)| v > 0)
        .map(|(stat, v)| (stat, v.min(i32::MAX as u64) as u32))
        .collect()
}

/// The player's own achievement counters (stat, value), sent with the character.
pub fn player_counters(c: &Character) -> Vec<(u32, u32)> {
    c.counters
        .iter()
        .filter_map(|(&stat, m)| Some((stat, (*m.get(&0)?).min(i32::MAX as u64) as u32)))
        .collect()
}

/// Earned achievement points and count (for the test command).
pub fn summary(c: &Character) -> (usize, usize) {
    let earned = c.tags.keys().filter(|t| TREE.by_id.contains_key(t)).count();
    (earned, TREE.all.iter().filter(|a| !a.invisible && a.kind != "category").count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character() -> Character {
        Character {
            name: "t".into(),
            mode: "progression".into(),
            playfield: 1200,

            pos: [0.0; 3],
            team: vec![],
            stars: 0,
            upgrades: Default::default(),
            tokens: Default::default(),
            xp: 0,
            level: 0,
            minifigure_levels: Default::default(),
            purchased_upgrades: Default::default(),
            tags: Default::default(),
            counters: Default::default(),
            defeats: Default::default(),
            visits: Default::default(),
            finds: Default::default(),
            smashed: Default::default(),
            collection: None,
            quests: Default::default(),
            story: Default::default(),
            speed: 1.0,
        }
    }

    #[test]
    fn removing() {
        let ctx = Ctx { character: 1, active: None, lost_creations_team: &[], target_team: &[], locate: &|_| (101, 6), upgrades: &|_| 0, owned: &|_| false };
        let mut c = character();
        for name in ["Achievement_Volcano", "Achievement_Swamp", "Achievement_Tarpit"] {
            on_event(&mut c, &ctx, &Event::Found { playfield: 6200, name: name.into() }, 1);
        }
        assert!(c.tags.contains_key(&1613) && c.tags.contains_key(&1539), "Fiery Explorer, and all of Dinosaur Rise");
        c.tags.insert(4121, 0); // story progress (the tutorial's end), not an achievement
        // one: it, the "all of" above it, and its find (so it can be earned again)
        let (gone, rpcs) = remove(&mut c, Some(&[1613]));
        assert_eq!(gone, [1539, 1613]);
        assert_eq!(rpcs.len(), 2);
        assert!(!c.finds.contains_key("6200:Achievement_Volcano") && c.finds.contains_key("6200:Achievement_Swamp"));
        on_event(&mut c, &ctx, &Event::Found { playfield: 6200, name: "Achievement_Volcano".into() }, 1);
        assert!(c.tags.contains_key(&1613) && c.tags.contains_key(&1539), "earned again");
        // all: every achievement and the journal; other tags stay
        let (gone, _) = remove(&mut c, None);
        assert!(gone.contains(&1611) && gone.contains(&1613));
        assert!(c.tags.keys().eq([&4121]) && c.finds.is_empty());
        assert!(remove(&mut c, Some(&[4121])).0.is_empty(), "not an achievement");
    }

    #[test]
    fn data_loads_and_names_match() {
        assert!(TREE.all.len() > 2000);
        assert!(same_name("the Alien Queen", "alien queen"));
        assert!(!same_name("Hades", "hadesx"));
    }

    #[test]
    fn kills_count_and_complete() {
        let cyclops = 41_757;
        let ctx = Ctx { character: 1, active: Some(cyclops), lost_creations_team: &[], target_team: &[cyclops], locate: &|_| (101, 6), upgrades: &|_| 1, owned: &|t| t == cyclops };
        let mut c = character();
        let ev = Event::Defeated { id: Id32::new(0, 0), name: "Skeleton".into(), smashable: false, boss: false, champion: false, level: 1, playfield: 1200 };
        for _ in 0..999 {
            on_event(&mut c, &ctx, &ev, 1);
        }
        assert!(!c.tags.contains_key(&2163));
        on_event(&mut c, &ctx, &ev, 1);
        assert_eq!(counter(&c.counters, STAT_PLAYER_KILLS, 0), 1000);
        // Cyclops' signature counter is monsters smashed
        assert_eq!(counter(&c.counters, STAT_MINIFIGURE_COUNTER1, cyclops), 1000);
        assert!(c.tags.contains_key(&2163), "Global_Smash 1000 Monsters");
        assert!(c.tags.contains_key(&2660), "Cyclops_Smash 1000 Monsters");
        let boss = Event::Defeated { id: Id32::new(0, 0), name: "Hades".into(), smashable: false, boss: true, champion: false, level: 20, playfield: 0 };
        let out = on_event(&mut c, &ctx, &boss, 1);
        assert!(c.tags.contains_key(&847), "Mythology_Story_Defeat Hades");
        // the Gauntlet's T-Rex (spawn/6300/t-rex): Roooooar!, which the party after him waits for
        let t_rex = Event::Defeated { id: Id32::new(0, 0), name: "T-Rex".into(), smashable: false, boss: true, champion: false, level: 21, playfield: 6300 };
        on_event(&mut c, &ctx, &t_rex, 1);
        assert!(c.tags.contains_key(&1540), "Dino_Defeat T-Rex");
        // its reward is a loot bag (the reward script's table), not stars given directly
        assert!(out.loot.iter().any(|t| t.starts_with("loot/achievement-stars")), "{:?}", out.loot);
    }

    #[test]
    fn minifigure_achievements() {
        let plumber = 50_715;
        let ctx = Ctx { character: 1, active: Some(plumber), lost_creations_team: &[plumber], target_team: &[plumber], locate: &|_| (101, 6), upgrades: &|_| 3, owned: &|t| t == plumber };
        let mut c = character();
        check(&mut c, &ctx);
        assert!(c.tags.contains_key(&2519) && !c.tags.contains_key(&2520), "Plumber_Upgrade 3, not 7");
        on_event(&mut c, &ctx, &Event::LostCreations { level: 10 }, 1);
        assert!(c.tags.contains_key(&2522) && c.tags.contains_key(&2523) && !c.tags.contains_key(&2529), "Lost Creations 5 and 10");
        // pocket adventures as Plumber: Siege Command Room has no name in world_content.json
        on_event(&mut c, &ctx, &Event::Cleared { playfield: 1673 }, 1);
        assert!(c.tags.contains_key(&2513), "Plumber_Siege Command Room");
        assert!(!c.tags.contains_key(&2526));
        on_event(&mut c, &ctx, &Event::Cleared { playfield: 1675 }, 1);
        assert!(c.tags.contains_key(&2514), "Plumber_Minotaur Refuge");
        assert!(c.tags.contains_key(&2526), "Plumber_Complete Any 5");
        assert!(!c.tags.contains_key(&2527), "Complete Any 10 needs 11");
        // 10 others and "Complete Any 5" make 11
        on_event(&mut c, &ctx, &Event::LostCreations { level: 15 }, 1);
        for boss in ["Bishop, Master Technician", "Megamind", "Beastmaster Wulfgar"] {
            let ev = Event::Defeated { id: Id32::new(0, 0), name: boss.into(), smashable: false, boss: true, champion: false, level: 1, playfield: 0 };
            on_event(&mut c, &ctx, &ev, 1);
        }
        assert!(!c.tags.contains_key(&2527), "9 others and Complete Any 5");
        on_event(&mut c, &ctx, &Event::Cleared { playfield: 4500 }, 1);
        assert!(c.tags.contains_key(&2527), "10 others and Complete Any 5");
        // InventoryItemGeneric3: the ten Plumber achievements its counter counts
        assert_eq!(super::minifigure_achievements(&c).get(&plumber), Some(&10));
        // the counters every copy of the item carries
        on_event(&mut c, &ctx, &Event::Built { name: "Mythology Statue 3".into() }, 2);
        assert_eq!(item_counters(&c, plumber), vec![(STAT_MINIFIGURE_COUNTER1, 2), (STAT_MINIFIGURE_COUNTER2, 2)]);
        assert_eq!(player_counters(&c), vec![(STAT_PLAYER_KILLS, 3)], "the three bosses");
    }

    #[test]
    fn smash_achievements_listed() {
        let list = smash_achievements();
        let witches = list.iter().find(|a| a.key == "minifigure:kills_or_smashes:witch:50").unwrap();
        assert_eq!((witches.title.as_str(), witches.text.as_str(), witches.minifigures as usize), ("Witch Hunter", "Smash 50 Witches.", witches.ids.len()));
        assert!(witches.ids.contains(&2576) && witches.minifigures > 1);
        let drakes = list.iter().find(|a| a.key == "minifigure:kills_or_smashes:drake:50").unwrap();
        assert_eq!(drakes.title, "Dragontamer");
        let global = list.iter().find(|a| a.key == "2163").unwrap();
        assert_eq!((global.what.as_deref(), global.n, global.group.as_deref()), (Some("kills"), 1000, Some("Global")));
        let skram = list.iter().find(|a| a.key == "770").unwrap();
        assert_eq!((skram.kind, skram.matching.as_deref(), skram.n), ("defeat", Some("Skram Gatecrasher"), 1));
        assert!(!list.iter().any(|a| a.key.contains("builds") || a.key == "2421"), "no builds or collectibles");
    }

    /// A completed Lost Creations level counts for every minifigure it is given (the unsmashed
    /// team), not just the active one.
    #[test]
    fn lost_creations_counts_for_the_team() {
        let mut fives = TREE.all.iter().filter(|a| a.kind == "lost_creations" && a.n == 5);
        let (a, b, other) = (fives.next().unwrap(), fives.next().unwrap(), fives.next().unwrap());
        let team = [a.minifigure.unwrap(), b.minifigure.unwrap()];
        let ctx = Ctx { character: 1, active: Some(team[0]), lost_creations_team: &team, target_team: &team, locate: &|_| (101, 6), upgrades: &|_| 0, owned: &|_| true };
        let mut c = character();
        on_event(&mut c, &ctx, &Event::LostCreations { level: 5 }, 1);
        assert!(c.tags.contains_key(&a.id) && c.tags.contains_key(&b.id), "both teammates");
        assert!(!c.tags.contains_key(&other.id), "not a minifigure outside the team");
    }

    /// The smash rules are global: tests setting them take turns.
    static RULES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn bosses_count_for_the_target_team() {
        // two minifigures' "Defeat X as Y" achievements for the same boss
        let (a, b) = {
            let mut found = TREE.all.iter().filter(|a| a.kind == "target" && a.target.as_deref() == Some("Beastmaster Wulfgar"));
            (found.next().unwrap(), found.next().unwrap())
        };
        let (fig_a, fig_b) = (a.minifigure.unwrap(), b.minifigure.unwrap());
        let boss = Event::Defeated { id: Id32::new(0, 0), name: "Beastmaster Wulfgar".into(), smashable: false, boss: true, champion: false, level: 10, playfield: 0 };
        let mut c = character();
        let ctx = Ctx { character: 1, active: Some(fig_a), lost_creations_team: &[], target_team: &[fig_a, fig_b], locate: &|_| (101, 6), upgrades: &|_| 0, owned: &|_| true };
        on_event(&mut c, &ctx, &boss, 1);
        assert!(c.tags.contains_key(&a.id) && c.tags.contains_key(&b.id), "both team minifigures");
        // a minifigure left out of the team (smashed) gets nothing
        let mut c = character();
        let ctx = Ctx { target_team: &[fig_b], ..ctx };
        on_event(&mut c, &ctx, &boss, 1);
        assert!(!c.tags.contains_key(&a.id) && c.tags.contains_key(&b.id));
    }

    #[test]
    fn smash_rules_choose_the_enemies() {
        let _turn = RULES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let judge = 41_840;
        let ctx = Ctx { character: 1, active: Some(judge), lost_creations_team: &[], target_team: &[judge], locate: &|_| (101, 6), upgrades: &|_| 0, owned: &|t| t == judge };
        let rule = |id: &str, enemies: &[&str]| SmashRule {
            id: id.into(),
            enemies: enemies.iter().map(|e| e.to_string()).collect(),
            once: false,
            item: None,
            comment: String::new(),
            tags: Vec::new(),
            feature: "live".into(),
            author: String::new(),
            updated: 0,
        };
        set_smash_rules(&[rule("minifigure:kills_or_smashes:witch:50", &["Swamp Hag"]), rule("770", &["Troll"])]);
        let mut c = character();
        let defeat = |name: &str| Event::Defeated { id: Id32::new(0, 0), name: name.into(), smashable: false, boss: false, champion: false, level: 1, playfield: 4200 };
        on_event(&mut c, &ctx, &defeat("Witch"), 1);
        assert_eq!(counter(&c.counters, STAT_MINIFIGURE_COUNTER2, judge), 0, "not in the rule");
        on_event(&mut c, &ctx, &defeat("the Swamp Hag"), 49);
        assert_eq!(counter(&c.counters, STAT_MINIFIGURE_COUNTER2, judge), 49);
        on_event(&mut c, &ctx, &defeat("swamp hag"), 1);
        assert!(c.tags.contains_key(&2576), "Judge_Smash 50 Witches");
        // a defeat achievement counts the rule's enemies (bosses or not), not its target
        on_event(&mut c, &ctx, &defeat("Troll"), 1);
        assert!(c.tags.contains_key(&770));
        set_smash_rules(&[]);
    }

    #[test]
    fn categories_and_collectibles() {
        let _turn = RULES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = Ctx { character: 1, active: None, lost_creations_team: &[], target_team: &[], locate: &|_| (101, 6), upgrades: &|_| 0, owned: &|_| false };
        let rule = |id: &str, enemies: &[&str], once: bool, item: Option<u32>| SmashRule {
            id: id.into(),
            enemies: enemies.iter().map(|e| e.to_string()).collect(),
            once,
            item,
            comment: String::new(),
            tags: Vec::new(),
            feature: "live".into(),
            author: String::new(),
            updated: 0,
        };
        let collar = collect_achievements().iter().find(|a| a.key == "2431").expect("Cerberus' Collar");
        assert_eq!((collar.kind, collar.n), ("collect", 3));
        assert_eq!(collect_achievements().iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), ["2421", "2424", "2426", "2428", "2431"]);
        set_smash_rules(&[
            rule("category:Trolls", &["Troll", "Cave Troll"], false, None),
            rule("770", &["@Trolls"], false, None),
            rule("2431", &["@Hounds"], false, None), // unknown category: nothing
            rule("2421", &["Pirate Goon"], true, None),
            rule("drop:Mushroom", &["@Trolls"], true, Some(12345)),
        ]);
        let mut c = character();
        let defeat = |name: &str| Event::Defeated { id: Id32::new(0, 7), name: name.into(), smashable: false, boss: false, champion: false, level: 1, playfield: 4200 };
        // a defeat achievement counting a category's enemies
        let out = on_event(&mut c, &ctx, &defeat("Cave Troll"), 1);
        assert!(c.tags.contains_key(&770));
        assert_eq!(out.drops, [(Id32::new(0, 7), 12345)]);
        assert!(on_event(&mut c, &ctx, &defeat("Cave Troll"), 1).drops.is_empty(), "once per enemy name");
        assert_eq!(on_event(&mut c, &ctx, &defeat("Troll"), 1).drops.len(), 1);
        // goblets: once per enemy name
        on_event(&mut c, &ctx, &defeat("Pirate Goon"), 2);
        on_event(&mut c, &ctx, &defeat("Pirate Goon"), 1);
        assert_eq!(counter(&c.counters, 3_000_119, 0), 1);
        // a wall collecting a collar: once per wall
        for wall in ["t-1", "t-1", "t-2", "t-3"] {
            on_event(&mut c, &ctx, &Event::Collected { key: "2431".into(), wall: wall.into() }, 1);
        }
        assert_eq!(counter(&c.counters, 3_000_122, 0), 3);
        assert!(c.tags.contains_key(&2431), "Collar of the Beast");
        remove(&mut c, Some(&[2431]));
        assert_eq!(counter(&c.counters, 3_000_122, 0), 0);
        on_event(&mut c, &ctx, &Event::Collected { key: "2431".into(), wall: "t-1".into() }, 1);
        assert_eq!(counter(&c.counters, 3_000_122, 0), 1, "removed: collected again");
        set_smash_rules(&[]);
    }

    #[test]
    fn journal_finds_and_clears() {
        let ctx = Ctx { character: 1, active: None, lost_creations_team: &[], target_team: &[], locate: &|_| (101, 6), upgrades: &|_| 1, owned: &|_| false };
        let mut c = character();
        let skeleton = Event::Defeated { id: Id32::new(0, 0), name: "Skeleton".into(), smashable: false, boss: false, champion: false, level: 1, playfield: 1200 };
        on_event(&mut c, &ctx, &skeleton, 2);
        on_event(&mut c, &ctx, &skeleton, 1);
        assert_eq!((c.defeats["Skeleton"].count, c.defeats["Skeleton"].kind.as_str()), (3, "enemy"));
        on_event(&mut c, &ctx, &Event::Visited { playfield: 4200 }, 1);
        on_event(&mut c, &ctx, &Event::Visited { playfield: 4200 }, 1);
        assert_eq!(c.visits[&4200].count, 2);
        // a collectible and a boombox (the boombox only in its own world)
        on_event(&mut c, &ctx, &Event::Found { playfield: 4210, name: "collectible_dreadlegs_treasure".into() }, 1);
        assert!(c.tags.contains_key(&2420), "Pirate_Collectibles_Dreadleg's Treasure");
        on_event(&mut c, &ctx, &Event::Found { playfield: 4200, name: "Achievement_PirateTown".into() }, 1);
        assert!(c.tags.contains_key(&724), "Pirate_Exploration_Find the Hidden Pirate Town");
        on_event(&mut c, &ctx, &Event::Found { playfield: 4200, name: "boombox 1".into() }, 1);
        assert!(c.tags.contains_key(&943), "[Isle of Yarr] Boombox");
        assert!(!c.tags.contains_key(&281), "[Volcano Island] Boombox");
        // dungeon names with an apostrophe (world_content.json has &apos;)
        on_event(&mut c, &ctx, &Event::Cleared { playfield: 1640 }, 1);
        assert!(c.tags.contains_key(&804), "Pirate_Combat_PA_Clear the Natives' Hollow");
        on_event(&mut c, &ctx, &Event::Cleared { playfield: 1600 }, 1);
        assert!(c.tags.contains_key(&825), "Space_Combat_PA_Mercenary Cave");
        let champion = Event::Defeated { id: Id32::new(0, 0), name: "Chunk the Tiger".into(), smashable: false, boss: false, champion: true, level: 5, playfield: 4200 };
        on_event(&mut c, &ctx, &champion, 1);
        assert!(c.tags.contains_key(&2377), "Pirate_Combat_Champion_Chunk the Tiger");
        assert_eq!(c.defeats["Chunk the Tiger"].kind, "champion");
        // Natives' Hollow is a pirate dungeon
        assert!(champions_for(1640).contains(&"Chunk the Tiger".to_string()));
        assert!(champions_for(4200).is_empty());
    }
}
