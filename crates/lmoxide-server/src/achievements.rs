//! Achievements: the client's lore tree tags under "Achievements" (data/achievements.json,
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
//! inferred from their names. Exploration, collectibles, seasonal events and Lost Creations have
//! no rule. Rewards: AchievementRewardValue stars where the reward icon is the stars icon.

use std::{
    collections::{BTreeMap, HashMap},
    sync::LazyLock,
};

use bytes::Bytes;
use lmoxide_net::{Endpoint, Writer, methods::game_char_interface, types::Id32};
use serde::Deserialize;
use tracing::info;

use crate::store::Character;

const STAT_PLAYER_KILLS: u32 = 3_000_116;
const STAT_PLAYER_SMASHES: u32 = 3_000_126;
/// Minifigure item counters (InventoryItemGeneric1/2).
const STAT_MINIFIGURE_COUNTER1: u32 = 3_000_112;
const STAT_MINIFIGURE_COUNTER2: u32 = 3_000_113;

#[derive(Debug, Deserialize)]
struct Achievement {
    id: u32,
    parent: u32,
    name: String,
    #[serde(default)]
    invisible: bool,
    #[serde(default)]
    stars: u64,
    #[serde(default)]
    minifigure: Option<u32>,
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
}

struct Tree {
    all: Vec<Achievement>,
    by_id: HashMap<u32, usize>,
    children: HashMap<u32, Vec<u32>>,
    /// Playfield names (world_content.json) for dungeon targets, lowercase.
    playfields: HashMap<u32, String>,
}

static TREE: LazyLock<Tree> = LazyLock::new(|| {
    let data: Data = serde_json::from_str(include_str!("../../../data/achievements.json")).expect("achievements.json");
    let by_id = data.achievements.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for a in &data.achievements {
        children.entry(a.parent).or_default().push(a.id);
    }
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/world_content.json")).expect("world_content.json");
    let playfields = worlds
        .iter()
        .filter_map(|(k, v)| Some((k.parse().ok()?, v.get("name")?.as_str()?.to_lowercase())))
        .collect();
    Tree { all: data.achievements, by_id, children, playfields }
});

/// Something a player did that achievements count.
#[derive(Debug, Clone)]
pub enum Event {
    /// Defeated an enemy of `level` (`boss`: a boss, `champion`: a champion) or smashed a
    /// smashable.
    Defeated { name: String, smashable: bool, boss: bool, champion: bool, level: u32, playfield: u32 },
    /// Finished building a buildie.
    Built { name: String },
    /// A server script gave the player tag `id` (NPC dialogues' `addtagtoplayer`).
    Tag(u32),
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
}

/// The character's view the rules need.
pub struct Ctx<'a> {
    pub character: u32,
    /// The active minifigure.
    pub active: Option<u32>,
    /// Where a minifigure item is (inventory, position), for SetItemStat.
    pub locate: &'a dyn Fn(u32) -> (u32, u32),
    /// A minifigure's level.
    pub level: &'a dyn Fn(u32) -> u32,
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
    evaluate(c, ctx, &mut out);
    out
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
    if let Event::Tag(id) = event {
        if TREE.by_id.contains_key(id) {
            grant(c, ctx, &mut out, *id);
        } else if !c.tags.contains_key(id) {
            // a plain tag (story progress), not an achievement
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            c.tags.insert(*id, now);
            out.rpcs.push(add_tag(*id));
        }
        return out;
    }
    let (name, kind) = match event {
        Event::Defeated { name, smashable: false, .. } => (name, "kills"),
        Event::Defeated { name, smashable: true, .. } => (name, "smashes"),
        Event::Built { name } => (name, "builds"),
        Event::Tag(_) => unreachable!(),
    };
    match kind {
        "kills" => bump(c, ctx, &mut out, STAT_PLAYER_KILLS, 0, times),
        "smashes" => bump(c, ctx, &mut out, STAT_PLAYER_SMASHES, 0, times),
        _ => {}
    }
    if let Some(active) = ctx.active {
        // the active minifigure's two item counters, by what its achievements count
        for stat in [STAT_MINIFIGURE_COUNTER1, STAT_MINIFIGURE_COUNTER2] {
            let counts = TREE.all.iter().any(|a| {
                a.minifigure == Some(active)
                    && a.counter.is_some_and(|[s, _]| s == stat as u64)
                    && a.counts.as_ref().is_some_and(|k| counts_event(k, kind, name))
            });
            if counts {
                bump(c, ctx, &mut out, stat, active, times);
            }
        }
    }
    if let Event::Defeated { name, boss, champion, playfield, .. } = event
        && (*boss || *champion)
    {
        let name = name.to_lowercase();
        let dungeon = TREE.playfields.get(playfield).cloned().unwrap_or_default();
        let hits: Vec<u32> = TREE
            .all
            .iter()
            .filter(|a| match (a.kind.as_str(), &a.target) {
                ("defeat", Some(t)) => same_name(t, &name),
                ("target", Some(t)) => {
                    a.minifigure == ctx.active && (same_name(t, &name) || (!dungeon.is_empty() && same_name(t, &dungeon)))
                }
                _ => false,
            })
            .map(|a| a.id)
            .collect();
        for id in hits {
            grant(c, ctx, &mut out, id);
        }
    }
    evaluate(c, ctx, &mut out);
    out
}

fn counts_event(k: &Counts, kind: &str, name: &str) -> bool {
    let kind_ok = k.what == kind || (k.what == "kills_or_smashes" && kind != "builds");
    kind_ok && k.matching.as_ref().is_none_or(|m| name.to_lowercase().contains(&m.to_lowercase()))
}

/// Names compare without case and a leading "the " ("Defeat the Alien Queen").
fn same_name(target: &str, name: &str) -> bool {
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

fn add_tag(tag: u32) -> (Endpoint, u32, Bytes) {
    // AddTag(uint32 tagID, uint32 timestamp): live sent 0
    (Endpoint::GameCharInterface, game_char_interface::ADD_TAG, Writer::new().u32(tag).u32(0).finish())
}

fn grant(c: &mut Character, _ctx: &Ctx, out: &mut Output, id: u32) {
    if c.tags.contains_key(&id) {
        return;
    }
    let Some(&i) = TREE.by_id.get(&id) else { return };
    let a = &TREE.all[i];
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    c.tags.insert(id, now);
    out.rpcs.push(add_tag(id));
    if a.stars > 0 {
        c.stars += a.stars;
        out.stars += a.stars;
    }
    info!(character = _ctx.character, achievement = id, name = %a.name, stars = a.stars, "achievement");
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
                "upgrade" => a.minifigure.is_some_and(|t| (ctx.owned)(t) && (ctx.level)(t) >= a.n),
                "series" => series_complete(a.series, ctx),
                "any" => {
                    let siblings = TREE.children.get(&a.parent).map_or(0, |s| {
                        s.iter().filter(|&&s| s != a.id && kind_of(s) != "any" && c.tags.contains_key(&s)).count()
                    });
                    siblings >= a.n as usize
                }
                "all" => !a.children.is_empty() && a.children.iter().all(|ch| c.tags.contains_key(ch)),
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
            playfield: 1200,
            pos: [0.0; 3],
            team: vec![],
            stars: 0,
            upgrades: Default::default(),
            tokens: Default::default(),
            xp: 0,
            level: 0,
            minifigure_levels: Default::default(),
            tags: Default::default(),
            counters: Default::default(),
        }
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
        let ctx = Ctx { character: 1, active: Some(cyclops), locate: &|_| (101, 6), level: &|_| 1, owned: &|t| t == cyclops };
        let mut c = character();
        let ev = Event::Defeated { name: "Skeleton".into(), smashable: false, boss: false, champion: false, level: 1, playfield: 1200 };
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
        let boss = Event::Defeated { name: "Hades".into(), smashable: false, boss: true, champion: false, level: 20, playfield: 0 };
        let out = on_event(&mut c, &ctx, &boss, 1);
        assert!(c.tags.contains_key(&847), "Mythology_Story_Defeat Hades");
        assert!(out.stars >= 500);
    }
}
