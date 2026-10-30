//! The story (research/lmoxide-notes/client-survey/story.md): the narrated cinematics, story
//! beats at the client's story trigger areas, talking heads and story goals.
//!
//! - **Cinematics** (tunables.json `story.cinematics`): the 16 narrated cutscenes are client
//!   effect packages (CinematicState timeline, camera paths, the narrator, TDB 50000 subtitles).
//!   Live started them with an `EffectPackageIIR` on the player: the player as the record's
//!   identity, named target (0, 0), no hook, a fresh instance, no position, no spell (the game
//!   intro on arriving in the tutorial, the Pirate World intro on arriving there:
//!   LMO_Captures.cap). Each plays once per character: its lore flag (`tag`, set as it plays) or
//!   the server's own record (`cinematic/<name>`) marks it seen. A trigger's `cinematic` action,
//!   or a dialogue's script step (`BecomeAPirate`, `DanceParty`), names one.
//! - **Story beats**: a trigger's `dialogue` action starts a recorded dialogue unless the player
//!   has every tag it gives; `story_beat` runs a dialogue text's server side (its tags, goals,
//!   teleport) where the conversation before it isn't known.
//! - **Talking heads**: the client shows a pusher (TDB 50000, keyed by tag) when the server adds
//!   its tag (the client's Lua 1597: "shows the tutorial talking [head]" = `player:addtag(385)`):
//!   the trigger command `tag`.
//! - **Story lines** (content/quests `kind: story`, steps in their order): shown in the quest
//!   tracker as live showed them (gold, MainQuestType 5), step by step. A step is done when its
//!   goals are resolved: dialogue texts' `resolvegoal`, the `story_goal` command (the Story_*
//!   locate areas), the area quests' reward scripts (`solvegoal`, QuestTriggerProgress), quest
//!   pickups, or a goal's `counts` (kills, smashes). `spawn <quest hash>` starts a line at that
//!   step. A goal of a later step jumps there (the steps between count as done: many have nothing
//!   that resolves them yet); one of an earlier step changes nothing.
//! - **Journal** (`journal`): the steps done, the area quests done and, while the teleport
//!   windows are unlocked (`--story-windows`, else tunables `story.windows_unlocked`), the
//!   quests the windows require.
//! - **A player's reset** (`/story reset`, `player_reset`): any player can forget their own story
//!   (steps, goals, cinematics seen, the story's lore flags); arriving in a world afterwards
//!   (its intro and first conversation, Pirate World's arrival) puts the line at that world.
//!
//! Kept per character (DB `character_story`): `cinematic/<name>`, `main/<main>` (line started),
//! `done/<quest>` (step done), `goal/<hash>` (resolved), `count/<hash>`, `wait/<hash>` (progress
//! waiting for its step: Lua 1699's buff), `chain` (`migrate` ran).
//!
//! Trigger commands (`register_command`): `cinematic {name}`, `dialogue {dialogue}`, `tag {tag}`,
//! `story_goal {goal, requires}`, `story_beat {text}`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use bytes::Bytes;
use lmoxide_content::{Quest, QuestGoal, QuestKind};
use lmoxide_net::types::Id32;
use serde_json::Value;
use tracing::info;

use crate::instance::Pf;
use crate::store::Character;
use crate::world;

/// What a player's connection does for the story (game.rs: it has the character).
#[derive(Clone, Debug)]
pub enum Order {
    /// Play cinematic `name` if the player hasn't seen it (and may see it now).
    Cinematic(String),
    /// Play cinematic `name` regardless (`!cinematic`); nothing is recorded.
    Replay(String),
    /// Resolve story goal `hash` if the player has all of `requires` (tags).
    Goal { hash: String, requires: Vec<u32> },
    /// `solvegoal <hash>`: only if the player is on that goal.
    Solve(String),
    /// `QuestTriggerProgress(hash, amount)`: only if the player has that goal (`wait`: once
    /// they have it).
    Progress { hash: String, amount: u32, wait: bool },
    /// `spawn <quest hash>`: that story line starts.
    Spawn(String),
    /// Start content dialogue `dialogue/...` unless the player has every tag it gives.
    Dialogue(String),
    /// Run dialogue text `text`'s server side (no window) unless the player has every tag it
    /// gives; only for a player with all of `requires` (tags).
    Beat { text: u32, requires: Vec<u32> },
    /// Give `tags` if the player has one of story goals `goals` (a reward script's Lua).
    TagsIfGoal { tags: Vec<u32>, goals: Vec<String> },
    /// The player picked up item `template`: the story goals that count it (QuestTriggerProgress).
    Pickup(u32),
    /// Test command: put story line `main` at `step` (1: its first; 0: not started; past the
    /// last: done).
    SetStep { main: u32, step: usize },
    /// Test command: resolve the current step's goals of line `main` (none: the first line
    /// running).
    Advance(Option<u32>),
}

/// Whether the teleport windows are open whatever the story (`--story-windows`): `Unlocked`
/// lists the quests they require in the login journal, `Locked` only what the character did
/// (as live: a window opens once the story reaches it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Windows {
    Locked,
    Unlocked,
}

/// The server's `--story-windows` (0: not given, the content's tunable; 1 locked; 2 unlocked).
static WINDOWS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The server's setting (`None`: the content's `story.windows_unlocked`).
pub fn set_windows(w: Option<Windows>) {
    let v = match w {
        None => 0,
        Some(Windows::Locked) => 1,
        Some(Windows::Unlocked) => 2,
    };
    WINDOWS.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the teleport windows are open whatever the story: the server's `--story-windows`,
/// else the content's tunable `story.windows_unlocked`.
pub fn windows_unlocked() -> bool {
    match WINDOWS.load(std::sync::atomic::Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => crate::gamedata::get().tunables.story.windows_unlocked,
    }
}

/// Tell `player`'s connection to do `order`.
pub fn send(player: Id32, order: Order) {
    world::send_to(player, world::Out::Story(order));
}

/// How far from the player a dialogue's speaking NPC may stand (m): the Video Game Guy is 45 m
/// from the LEGO Center's arrival, where the hub cinematics hand over to him.
const SPEAKER_RADIUS: f32 = 80.0;

/// Players watching a cinematic, until when (one at a time).
static WATCHING: LazyLock<Mutex<HashMap<Id32, Instant>>> = LazyLock::new(Default::default);

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The trigger commands this module runs (content.rs calls it when the content is loaded).
pub fn init() {
    use crate::triggers::register_command;
    let arg_str = |args: &BTreeMap<String, Value>, k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    register_command(
        "cinematic",
        Arc::new(move |f, args| match arg_str(args, "name") {
            Some(name) => send(f.player, Order::Cinematic(name)),
            None => info!(trigger = f.trigger, "cinematic: no name"),
        }),
    );
    register_command(
        "dialogue",
        Arc::new(move |f, args| match arg_str(args, "dialogue") {
            Some(d) => send(f.player, Order::Dialogue(d)),
            None => info!(trigger = f.trigger, "dialogue: none named"),
        }),
    );
    register_command(
        "tag",
        Arc::new(|f, args| match args.get("tag").and_then(Value::as_u64) {
            // (the achievements give a plain tag once and send AddTag: the talking head shows)
            Some(tag) => world::send_to(f.player, world::Out::Progress(crate::achievements::Event::Tag(tag as u32))),
            None => info!(trigger = f.trigger, "tag: none given"),
        }),
    );
    register_command(
        "story_goal",
        Arc::new(move |f, args| {
            let Some(hash) = arg_str(args, "goal") else { return info!(trigger = f.trigger, "story_goal: no goal") };
            send(f.player, Order::Goal { hash, requires: tags_arg(args) });
        }),
    );
    register_command(
        "story_beat",
        Arc::new(|f, args| match args.get("text").and_then(Value::as_u64) {
            Some(text) => send(f.player, Order::Beat { text: text as u32, requires: tags_arg(args) }),
            None => info!(trigger = f.trigger, "story_beat: no text"),
        }),
    );
}

/// A command's `requires`: tags.
fn tags_arg(args: &BTreeMap<String, Value>) -> Vec<u32> {
    args.get("requires").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as u32)).collect()).unwrap_or_default()
}

/// The story lines: (main quest, its line).
fn lines() -> impl Iterator<Item = (u32, &'static Quest)> {
    crate::gamedata::get().quests.values().filter(|q| q.kind == QuestKind::Story && !q.steps.is_empty()).filter_map(|q| Some((q.main?, q)))
}

fn line(main: u32) -> Option<&'static Quest> {
    lines().find(|(m, _)| *m == main).map(|(_, q)| q)
}

/// A story goal by hash: (main quest, its line, the step, the goal).
fn find_goal(hash: &str) -> Option<(u32, &'static Quest, usize, &'static QuestGoal)> {
    lines().find_map(|(main, q)| {
        q.steps.iter().enumerate().find_map(|(i, s)| s.goals.iter().find(|g| g.hash.as_deref() == Some(hash)).map(|g| (main, q, i, g)))
    })
}

/// A story step by its quest's hash (`spawn <hash>`): (main quest, its line, the step).
fn find_step(hash: &str) -> Option<(u32, &'static Quest, usize)> {
    lines().find_map(|(main, q)| q.steps.iter().position(|s| s.hash.as_deref() == Some(hash)).map(|i| (main, q, i)))
}

/// The minifigure template `spawn <hash>` gives (tunables `story.spawn_minifigures`), unless the
/// hash is a story quest's. game.rs adds it to the collection directly: the client's AddItem
/// into the Collection inventory opens its "Congratulations! You got a new minifigure." window
/// (BagOpenEvent.swf `SlotMinifigureAdded`), as in the user's live footage.
pub fn spawn_minifigure(order: &Order) -> Option<u32> {
    let Order::Spawn(hash) = order else { return None };
    if find_step(hash).is_some() {
        return None;
    }
    let t = &crate::gamedata::get().tunables.story;
    t.spawn_minifigures.get(hash).and_then(|m| crate::gamedata::get().minifigures.get(m)).map(|m| m.template)
}

/// A line by main quest id or world name (`pirate`, `medieval` ...: the record's id).
pub fn find_line(arg: &str) -> Option<u32> {
    if let Ok(main) = arg.parse::<u32>() {
        return line(main).map(|_| main);
    }
    let a = arg.to_ascii_lowercase();
    lines().find(|(_, q)| q.id.contains(&a) || q.name.to_ascii_lowercase().contains(&a)).map(|(m, _)| m)
}

fn resolved(c: &Character, hash: &str) -> bool {
    c.story.contains_key(&format!("goal/{hash}"))
}

fn started(c: &Character, main: u32) -> bool {
    c.story.contains_key(&format!("main/{main}"))
}

fn passed(c: &Character, quest: u32) -> bool {
    c.story.contains_key(&format!("done/{quest}"))
}

/// The step of `q` the player is on: none before the line started and after its last step.
fn current(c: &Character, main: u32, q: &Quest) -> Option<usize> {
    if !started(c, main) {
        return None;
    }
    q.steps.iter().position(|s| !passed(c, s.quest))
}

/// Whether the player "has" story goal `hash` (the client's Lua `player:hasgoal`): it is a goal
/// of the step they are on.
fn has_goal(c: &Character, hash: &str) -> bool {
    find_goal(hash).is_some_and(|(main, q, i, _)| current(c, main, q) == Some(i))
}

/// The step's place in the tracker's list (a countdown: the first step the highest).
fn countdown(q: &Quest, step: usize) -> u32 {
    (q.steps.len() - 1 - step) as u32
}

/// The line as the records show it, for a player in `place`.
fn view(q: &'static Quest, main: u32, place: Pf) -> crate::quests::Line<'static> {
    let c = crate::gamedata::get();
    let home = q.world.as_ref().and_then(|w| c.worlds.get(w)).map_or(0, |w| w.playfield);
    let super_region = c.world_by_playfield(place.model()).and_then(|w| w.super_region).unwrap_or(0);
    crate::quests::Line { quest: q, main, playfield: home, region: [place.model(), super_region] }
}

/// Counted toward the step's counted goal.
fn counted(c: &Character, q: &Quest, step: usize) -> u32 {
    q.steps[step].goals.iter().find(|g| g.counts.is_some()).and_then(|g| g.hash.as_ref()).and_then(|h| c.story.get(&format!("count/{h}"))).map_or(0, |&n| n as u32)
}

/// Whether the client's journal holds main quest `main` (an entry added before).
fn in_journal(c: &Character, main: u32, q: &Quest) -> bool {
    let t = &crate::gamedata::get().tunables.story;
    (windows_unlocked() && t.window_quests.contains_key(&main.to_string())) || q.steps.iter().any(|s| passed(c, s.quest))
}

/// The line in the tracker: added, at its current step.
fn show_line(c: &Character, character: u32, main: u32, q: &'static Quest, place: Pf) -> Vec<Bytes> {
    let Some(step) = current(c, main, q) else { return Vec::new() };
    let l = view(q, main, place);
    vec![crate::quests::main_quest_added(character, &l, step as u32), crate::quests::full_update(character, &l, step, counted(c, q, step), false)]
}

/// The story lines running, for the tracker (every arrival: the client forgot its quests).
pub fn show(c: &Character, character: u32, place: Pf) -> Vec<Bytes> {
    lines().flat_map(|(main, q)| show_line(c, character, main, q, place)).collect()
}

/// Steps `from..=to` of the line are done (the player was on `from`, which the tracker shows if
/// `shown`): the tracker and journal records, then the next step (or the line removed).
#[allow(clippy::too_many_arguments)]
fn pass(c: &mut Character, character: u32, main: u32, q: &'static Quest, from: usize, to: usize, place: Pf, shown: bool) -> Vec<Bytes> {
    use crate::quests::{full_update, goal_record, journal_record, quest_record};
    let now = unix_now();
    let l = view(q, main, place);
    let mut records = Vec::new();
    for i in from..=to.min(q.steps.len() - 1) {
        let s = &q.steps[i];
        let new_main = (!in_journal(c, main, q)).then_some(l.region);
        c.story.insert(format!("done/{}", s.quest), now);
        info!(character, main, quest = s.quest, step = i + 1, skipped = i != from, "story step done");
        if i == from && from == to {
            // a step done (none skipped: skipped steps give nothing) gives its reward items' bags
            crate::quests::give_rewards(character, c, place, &s.rewards);
        }
        if shown && i == from {
            records.push(goal_record(character, s.quest, s.goals.first().map_or(0, |g| g.goal), 1, 0));
            records.push(quest_record(character, 2, s.quest));
            records.push(journal_record(character, main, s.quest, new_main, countdown(q, i), 1));
            records.push(quest_record(character, 4, s.quest));
        } else {
            records.push(journal_record(character, main, s.quest, new_main, countdown(q, i), 1));
        }
    }
    match current(c, main, q) {
        Some(next) if shown => records.push(full_update(character, &l, next, counted(c, q, next), false)),
        Some(_) => records.extend(show_line(c, character, main, q, place)),
        None => {
            info!(character, main, "story line done");
            crate::quests::give_rewards(character, c, place, &q.rewards);
            if shown {
                records.push(crate::quests::main_quest_removed(character, main));
            }
        }
    }
    // progress that waited for the new step (Lua 1699's buff)
    if let Some(next) = current(c, main, q) {
        for hash in q.steps[next].goals.iter().filter_map(|g| g.hash.clone()) {
            if let Some(n) = c.story.remove(&format!("wait/{hash}")) {
                records.extend(progress(c, character, place, &hash, n as u32));
            }
        }
    }
    records
}

/// Goal `hash` resolved: its step done (and the steps before it the player was still on).
fn resolve(c: &mut Character, character: u32, place: Pf, hash: &str, why: &str) -> Vec<Bytes> {
    let Some((main, q, step, g)) = find_goal(hash) else {
        info!(character, hash, why, "story: not a story goal");
        return Vec::new();
    };
    let now = unix_now();
    if c.story.insert(format!("goal/{hash}"), now).is_none() {
        info!(character, main, goal = g.goal, hash, why, "story goal resolved");
    }
    let shown = started(c, main);
    if !shown {
        c.story.insert(format!("main/{main}"), now);
        info!(character, main, "story line started");
    }
    let Some(on) = current(c, main, q) else { return Vec::new() };
    if step < on {
        return Vec::new();
    }
    let all = q.steps[step].goals.iter().all(|g| g.hash.as_deref().is_none_or(|h| resolved(c, h)));
    match (all, shown) {
        (true, _) => pass(c, character, main, q, on, step, place, shown),
        (false, true) => Vec::new(),
        (false, false) => show_line(c, character, main, q, place),
    }
}

/// `amount` more toward goal `hash` (the player has it).
fn progress(c: &mut Character, character: u32, place: Pf, hash: &str, amount: u32) -> Vec<Bytes> {
    let Some((_, q, step, g)) = find_goal(hash) else { return Vec::new() };
    let key = format!("count/{hash}");
    let n = c.story.get(&key).copied().unwrap_or(0) + i64::from(amount);
    c.story.insert(key, n);
    let need = g.count.max(1);
    info!(character, hash, n, need, "story goal progress");
    if n >= i64::from(need) {
        return resolve(c, character, place, hash, "counted");
    }
    vec![crate::quests::goal_record(character, q.steps[step].quest, g.goal, 2, g.step() * amount)]
}

/// Counts `event` (a defeat) for the story goals the player is on that count it.
pub fn on_event(c: &mut Character, character: u32, place: Pf, event: &crate::achievements::Event) -> Vec<Bytes> {
    // the rebuilt conversations after a boss (story_dialogues.py `start: boss`)
    if let crate::achievements::Event::Defeated { name, boss, champion, smashable: false, playfield, .. } = event {
        for id in crate::knubot::after_defeat(name, *boss, *champion, *playfield) {
            info!(character, dialogue = id, defeated = %name, "story: dialogue after a defeat");
            send(Id32::new(crate::territory::CHARACTER_TYPE, character), Order::Dialogue(id));
        }
    }
    let hashes: Vec<String> = lines()
        .filter_map(|(main, q)| current(c, main, q).map(|i| &q.steps[i]))
        .flat_map(|s| &s.goals)
        .filter(|g| crate::quests::counts(g, event))
        .filter_map(|g| g.hash.clone())
        .collect();
    hashes.iter().flat_map(|h| progress(c, character, place, h, 1)).collect()
}

/// The quest pickups a defeat (`event`, at `at`) drops for the player: one of the pickups of each
/// story goal the player is on whose `dropped_by` it is, inside the goal's `drops_in` area (with
/// tunables `story.pickup_drop_chance`), as `quests::pickup_drops` for area quests.
pub fn pickup_drops(c: &Character, event: &crate::achievements::Event, at: [f32; 3]) -> Vec<u32> {
    use rand::Rng;
    let crate::achievements::Event::Defeated { playfield, .. } = event else { return Vec::new() };
    let content = crate::gamedata::get();
    let chance = content.tunables.story.pickup_drop_chance.fraction().clamp(0.0, 1.0);
    let inside = |area: &str| {
        crate::quests::area_playfield(area) == Some(*playfield)
            && content.areas.get(area).and_then(|a| a.shape.as_ref()).is_some_and(|s| s.contains(at, crate::quests::HEIGHT_SLACK))
    };
    let mut rng = rand::rng();
    lines()
        .filter_map(|(main, q)| current(c, main, q).map(|i| &q.steps[i]))
        .flat_map(|s| &s.goals)
        .filter(|g| g.counts.as_deref() == Some("pickups") && !g.pickups.is_empty())
        .filter_map(|g| {
            let by = g.dropped_by.as_deref()?;
            let dropper = QuestGoal { counts: Some(by.to_string()), ..g.clone() };
            let here = g.drops_in.as_deref().is_none_or(inside);
            (here && crate::quests::counts(&dropper, event) && rng.random_bool(chance)).then(|| g.pickups[rng.random_range(0..g.pickups.len())])
        })
        .collect()
}

/// The journal at login: the steps done, the area quests done and (`windows_unlocked`) the
/// quests the teleport windows require.
pub fn journal(c: &Character) -> Vec<crate::teleport::JournalMain> {
    journal_with(c, windows_unlocked())
}

/// `journal` with the windows unlocked or not.
fn journal_with(c: &Character, unlocked: bool) -> Vec<crate::teleport::JournalMain> {
    let mut mains: BTreeMap<u32, Vec<(u32, u32, u32)>> = BTreeMap::new();
    for (main, q) in lines() {
        for (i, s) in q.steps.iter().enumerate().filter(|(_, s)| passed(c, s.quest)) {
            mains.entry(main).or_default().push((s.quest, countdown(q, i), 1));
        }
    }
    for (main, quests, done) in crate::quests::completed(c) {
        mains.entry(main).or_default().extend(quests.into_iter().map(|q| (q, 0, done.saturating_sub(1))));
    }
    let t = &crate::gamedata::get().tunables.story;
    if unlocked {
        for (main, quests) in &t.window_quests {
            let Ok(main) = main.parse::<u32>() else { continue };
            let list = mains.entry(main).or_default();
            for &q in quests {
                if !list.iter().any(|e| e.0 == q) {
                    list.push((q, 0, 0));
                }
            }
        }
    }
    mains.into_iter().map(|(main, quests)| crate::teleport::JournalMain { main, quests }).collect()
}

/// Test command: forgets the character's story (not migrated again: the lore flags stay); the
/// lines leave the tracker.
pub fn reset(c: &mut Character, character: u32) -> Vec<Bytes> {
    let records = lines().filter(|(main, q)| current(c, *main, q).is_some()).map(|(main, _)| crate::quests::main_quest_removed(character, main)).collect();
    c.story.clear();
    c.story.insert("chain".into(), unix_now());
    records
}

/// The story's lore flags: the tags dialogue texts give, the cinematics' and the talking heads'
/// (trigger `tag` commands); not achievements, and not the tutorial done (where a character
/// arrives).
pub fn story_tags() -> std::collections::BTreeSet<u32> {
    let c = crate::gamedata::get();
    let mut tags = crate::knubot::all_text_tags();
    tags.extend(c.tunables.story.cinematics.values().filter_map(|d| d.tag));
    for t in c.triggers.values() {
        for a in &t.actions {
            if let lmoxide_content::Action::Command { name, args } = a
                && name == "tag"
                && let Some(tag) = args.get("tag").and_then(Value::as_u64)
            {
                tags.insert(tag as u32);
            }
        }
    }
    tags.retain(|&t| !crate::achievements::is_achievement(t) && t != crate::tutorial::DONE_TAG);
    tags
}

/// A player's own reset (`/story reset`): the story forgotten as by `reset`, and its lore flags
/// (`story_tags`) too, so the cinematics, conversations and talking heads play again; the story
/// areas the player set off this session go off again. Achievements, the collection, levels
/// and area quests stay. Returns the tracker records and the RemoveTag RPCs for the client.
pub fn player_reset(c: &mut Character, player: Id32) -> (Vec<Bytes>, Vec<(lmoxide_net::Endpoint, u32, Bytes)>) {
    let records = reset(c, player.instance);
    let gone: Vec<u32> = story_tags().into_iter().filter(|t| c.tags.remove(t).is_some()).collect();
    crate::triggers::forget_player(player);
    WATCHING.lock().unwrap().remove(&player);
    info!(character = player.instance, tags = ?gone, "story reset by the player");
    (records, gone.into_iter().map(crate::achievements::remove_tag).collect())
}

/// Characters from before the story lines were shown: what they did (the goals resolved, the
/// goals the dialogue texts whose tags they have resolve, the tutorial's boss) becomes their
/// place in each line: every step up to the last one resolved is done. Once per character.
pub fn migrate(c: &mut Character) {
    if c.story.contains_key("chain") {
        return;
    }
    let now = unix_now();
    let mut goals: Vec<String> = c.story.keys().filter_map(|k| k.strip_prefix("goal/").map(str::to_string)).collect();
    for (tags, resolves) in crate::knubot::goal_texts() {
        if !tags.is_empty() && tags.iter().all(|t| c.tags.contains_key(t)) {
            goals.extend(resolves);
        }
    }
    // the tutorial's last dialogue (after its boss: "Follow the Video Game Guy" resolved live)
    if c.tags.contains_key(&crate::tutorial::DONE_TAG) {
        goals.extend(lines().filter_map(|(_, q)| q.steps[0].goals.iter().find(|g| g.counts.as_deref() == Some("kills") && g.enemies.iter().any(|e| e == crate::tutorial::BOSS))?.hash.clone()));
    }
    for (main, q) in lines() {
        let last = q.steps.iter().rposition(|s| s.goals.iter().any(|g| g.hash.as_ref().is_some_and(|h| goals.contains(h))));
        if let Some(last) = last {
            c.story.entry(format!("main/{main}")).or_insert(now);
            for s in &q.steps[..=last] {
                c.story.entry(format!("done/{}", s.quest)).or_insert(now);
            }
            info!(main, steps = last + 1, "story line migrated");
        }
    }
    for g in goals {
        c.story.entry(format!("goal/{g}")).or_insert(now);
    }
    c.story.insert("chain".into(), now);
}

/// Does `order` for the player (game.rs, with the character): the records for its client (the
/// quest tracker).
pub fn handle(order: Order, c: &mut Character, player: Id32, name: &str, place: Pf) -> Vec<Bytes> {
    let character = player.instance;
    match order {
        Order::Cinematic(n) => cinematic(c, player, place, &n, false),
        Order::Replay(n) => cinematic(c, player, place, &n, true),
        Order::Goal { hash, requires } => {
            if requires.iter().all(|t| c.tags.contains_key(t)) {
                return resolve(c, character, place, &hash, "trigger or dialogue");
            }
        }
        Order::Solve(hash) => {
            if has_goal(c, &hash) {
                return resolve(c, character, place, &hash, "solvegoal");
            }
        }
        Order::Progress { hash, amount, wait } => {
            if has_goal(c, &hash) {
                return progress(c, character, place, &hash, amount);
            }
            if wait && find_goal(&hash).is_some_and(|(main, q, step, _)| current(c, main, q).is_none_or(|on| on < step)) {
                info!(character, hash, amount, "story progress waits for its step");
                *c.story.entry(format!("wait/{hash}")).or_insert(0) += i64::from(amount);
            } else {
                info!(character, hash, "story progress: the player doesn't have the goal");
            }
        }
        Order::Spawn(hash) => {
            let Some((main, q, step)) = find_step(&hash) else {
                // (a minifigure the dialogue gives: game.rs grants it, see `spawn_minifigure`)
                if !crate::gamedata::get().tunables.story.spawn_minifigures.contains_key(&hash) {
                    info!(character, hash, "spawn: not a story quest or minifigure (an NPC template?)");
                }
                return Vec::new();
            };
            let shown = started(c, main);
            if !shown {
                c.story.insert(format!("main/{main}"), unix_now());
                info!(character, main, hash, "story line started");
            }
            return match current(c, main, q) {
                Some(on) if on < step => pass(c, character, main, q, on, step - 1, place, shown),
                Some(_) if !shown => show_line(c, character, main, q, place),
                _ => Vec::new(),
            };
        }
        Order::Dialogue(id) => {
            let Some(d) = crate::knubot::by_id(&id) else {
                info!(character, dialogue = id, "story: no such dialogue");
                return Vec::new();
            };
            // (a trigger that goes off again while it runs: the cinematic moves the player)
            if crate::knubot::talking(player) {
                return Vec::new();
            }
            let gives = crate::knubot::tags_given(&d);
            let key = format!("{id}");
            if !gives.is_empty() && gives.iter().all(|t| c.tags.contains_key(t)) {
                return Vec::new();
            }
            // the rebuilt ones: their conditions; one that gives no tag plays once
            if crate::knubot::rebuilt(&d) {
                let may = d.requires.iter().all(|t| c.tags.contains_key(t))
                    && !d.unless.iter().any(|t| c.tags.contains_key(t))
                    && d.goals.iter().all(|g| resolved(c, g));
                if !may || (gives.is_empty() && c.story.contains_key(&key)) {
                    return Vec::new();
                }
                c.story.insert(key, unix_now());
            }
            // the speaker: that NPC near the player (else anywhere here), else the player (the
            // cinematics, whose owner wasn't spawned)
            let at = world::live_position(player).unwrap_or(c.pos);
            let owner = crate::npc::nearest(&d.npc, place, at, SPEAKER_RADIUS).or_else(|| crate::npc::find(&d.npc, place)).unwrap_or(player);
            crate::knubot::start(player, name, owner, d);
        }
        Order::Beat { text, requires } => {
            let gives = crate::knubot::text_tags(text);
            if (gives.is_empty() || !gives.iter().all(|t| c.tags.contains_key(t))) && requires.iter().all(|t| c.tags.contains_key(t)) {
                crate::knubot::beat(player, text);
            }
        }
        Order::TagsIfGoal { tags, goals } => {
            if goals.iter().any(|g| has_goal(c, g)) {
                for tag in tags {
                    world::send_to(player, world::Out::Progress(crate::achievements::Event::Tag(tag)));
                }
            }
        }
        Order::Pickup(template) => {
            let goals: Vec<String> = lines()
                .filter_map(|(main, q)| current(c, main, q).map(|i| &q.steps[i]))
                .flat_map(|s| &s.goals)
                .filter(|g| g.counts.as_deref() == Some("pickups") && g.pickups.contains(&template))
                .filter_map(|g| g.hash.clone())
                .collect();
            return goals.iter().flat_map(|h| progress(c, character, place, h, 1)).collect();
        }
        Order::SetStep { main, step } => {
            let Some(q) = line(main) else { return Vec::new() };
            let mut records = Vec::new();
            if current(c, main, q).is_some() {
                records.push(crate::quests::main_quest_removed(character, main));
            }
            let now = unix_now();
            for (i, s) in q.steps.iter().enumerate() {
                for h in s.goals.iter().filter_map(|g| g.hash.as_ref()) {
                    c.story.remove(&format!("count/{h}"));
                    c.story.remove(&format!("wait/{h}"));
                    if i + 1 >= step {
                        c.story.remove(&format!("goal/{h}"));
                    }
                }
                if i + 1 < step {
                    c.story.insert(format!("done/{}", s.quest), now);
                } else {
                    c.story.remove(&format!("done/{}", s.quest));
                }
            }
            if step == 0 {
                c.story.remove(&format!("main/{main}"));
            } else {
                c.story.insert(format!("main/{main}"), now);
            }
            info!(character, main, step, "story line set (test command)");
            records.extend(show_line(c, character, main, q, place));
            return records;
        }
        Order::Advance(main) => {
            let on = lines().filter(|(m, _)| main.is_none_or(|x| x == *m)).find_map(|(m, q)| current(c, m, q).map(|i| &q.steps[i]));
            let Some(step) = on else { return Vec::new() };
            let hashes: Vec<String> = step.goals.iter().filter_map(|g| g.hash.clone()).collect();
            return hashes.iter().flat_map(|h| resolve(c, character, place, h, "test command")).collect();
        }
    }
    Vec::new()
}

/// Plays cinematic `n` for the player if it may (`force`: anyway).
fn cinematic(c: &mut Character, player: Id32, place: Pf, n: &str, force: bool) {
    let character = player.instance;
    let Some(def) = crate::gamedata::get().tunables.story.cinematics.get(n) else {
        return info!(character, cinematic = n, "no such cinematic");
    };
    let key = format!("cinematic/{n}");
    if !force {
        let seen = c.story.contains_key(&key) || def.tag.is_some_and(|t| c.tags.contains_key(&t));
        let may = def.requires.iter().all(|t| c.tags.contains_key(t)) && !def.unless.iter().any(|t| c.tags.contains_key(t));
        if seen || !may {
            return;
        }
    }
    let now = Instant::now();
    {
        let mut watching = WATCHING.lock().unwrap();
        watching.retain(|_, until| *until > now);
        if watching.contains_key(&player) {
            return info!(character, cinematic = n, "cinematic: another one is playing");
        }
        watching.insert(player, now + def.length.to_std());
    }
    info!(character, cinematic = n, package = def.effect_package, force, "cinematic");
    if !force {
        c.story.insert(key, unix_now());
        if let Some(tag) = def.tag {
            world::send_to(player, world::Out::Progress(crate::achievements::Event::Tag(tag)));
        }
    }
    world::send_to(player, world::Out::Records(vec![record(player, def.effect_package)]));
    if let Some(next) = def.then_dialogue.clone().filter(|_| !force) {
        let length = def.length.to_std();
        tokio::spawn(async move {
            tokio::time::sleep(length).await;
            if world::whereabouts(player).is_some_and(|(pf, _)| pf == place) {
                send(player, Order::Dialogue(next));
            }
        });
    }
}

/// The cinematic's `EffectPackageIIR` (as live: LMO_Captures.cap, the game and Pirate World
/// intros).
pub fn record(player: Id32, package: u32) -> bytes::Bytes {
    crate::effects::ground_effect(player, package, [0.0; 3], 0, crate::effects::new_instance())
}

/// A script step of a dialogue (`gameactiononnpc` 599417 `String1=<step>`) that plays a
/// cinematic: true if it was one.
pub fn script_step(player: Id32, step: &str) -> bool {
    let cinematic = match step {
        // (both likely: the cinematic follows the dialogue that runs the step)
        "BecomeAPirate" => "becoming-a-pirate",
        "DanceParty" => "after-t-rex",
        _ => return false,
    };
    send(player, Order::Cinematic(cinematic.to_string()));
    true
}

/// Test command: the character's story (cinematics seen, story lines, goals).
pub fn summary(c: &Character) -> Vec<String> {
    let cinematics = &crate::gamedata::get().tunables.story.cinematics;
    let seen: Vec<&str> = cinematics
        .iter()
        .filter(|(n, d)| c.story.contains_key(&format!("cinematic/{n}")) || d.tag.is_some_and(|t| c.tags.contains_key(&t)))
        .map(|(n, _)| n.as_str())
        .collect();
    let mut lines = vec![format!("cinematics seen ({}/{}): {}", seen.len(), cinematics.len(), seen.join(", "))];
    let keys = |prefix: &str| c.story.iter().filter_map(|(k, v)| k.strip_prefix(prefix).map(|k| (k.to_string(), *v))).collect::<Vec<_>>();
    for (main, q) in self::lines() {
        if !started(c, main) {
            continue;
        }
        let at = match current(c, main, q) {
            Some(i) => {
                let s = &q.steps[i];
                let goal = s.goals.first().map(|g| format!("\"{}\" ({})", g.text.as_deref().unwrap_or("?"), g.hash.as_deref().unwrap_or("?"))).unwrap_or_default();
                format!("step {}/{}: {goal}", i + 1, q.steps.len())
            }
            None => "done".to_string(),
        };
        lines.push(format!("{} ({main}): {at}", q.name));
    }
    lines.push(format!("goals resolved: {}", keys("goal/").iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")));
    let counts = keys("count/");
    if !counts.is_empty() {
        lines.push(format!("counted: {}", counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")));
    }
    lines
}

/// The cinematics, for `!cinematic`: (name, package, text, what it is).
pub fn cinematics() -> Vec<(String, u32, String)> {
    crate::gamedata::get().tunables.story.cinematics.iter().map(|(n, d)| (n.clone(), d.effect_package, d.comment.clone())).collect()
}

/// A cinematic by name, number in `cinematics()` (1-based) or effect package.
pub fn find_cinematic(arg: &str) -> Option<String> {
    let all = cinematics();
    if let Ok(n) = arg.parse::<usize>() {
        if let Some((name, ..)) = all.get(n.wrapping_sub(1)) {
            return Some(name.clone());
        }
        return all.into_iter().find(|(_, p, _)| *p as usize == n).map(|(name, ..)| name);
    }
    let a = arg.to_ascii_lowercase();
    all.into_iter().map(|(name, ..)| name).find(|name| name.contains(&a))
}

/// Test targets: the story trigger areas (cinematics, beats, talking heads, story goals).
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    const STORY: &[&str] = &["cinematic", "dialogue", "tag", "story_goal", "story_beat"];
    let c = crate::gamedata::get();
    let mut out: Vec<(String, u32, [f32; 3])> = c
        .triggers
        .values()
        .filter(|t| !t.removed)
        .filter_map(|t| {
            let what: Vec<String> = t
                .actions
                .iter()
                .filter_map(|a| match a {
                    lmoxide_content::Action::Cinematic { name } => Some(format!("cinematic {name}")),
                    lmoxide_content::Action::Dialogue { dialogue } => Some(dialogue.trim_start_matches("dialogue/").to_string()),
                    lmoxide_content::Action::Command { name, args } if STORY.contains(&name.as_str()) => {
                        let arg = args.values().next().map(|v| v.to_string().trim_matches('"').to_string()).unwrap_or_default();
                        Some(format!("{name} {arg}"))
                    }
                    _ => None,
                })
                .collect();
            if what.is_empty() {
                return None;
            }
            let a = c.areas.get(t.area.as_ref()?)?;
            let pf: u32 = t.area.as_ref()?.split('/').nth(1)?.parse().ok()?;
            let centre = crate::triggers::area_centre(a.shape.as_ref()?);
            Some((format!("{} ({})", a.client_name.clone().unwrap_or_else(|| a.name.clone()), what.join(", ")), pf, centre))
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn quarry_crystals_drop_in_the_quarry() {
        // the Medieval story's "Collect magic crystals at the Quarry" (3UX1): the Quarry's
        // enemies drop them (the user could not collect any), others don't
        let goal = lines().flat_map(|(_, q)| &q.steps).flat_map(|s| &s.goals).find(|g| g.hash.as_deref() == Some("3UX1")).unwrap();
        assert_eq!(goal.dropped_by.as_deref(), Some("kills_or_smashes"));
        let area = goal.drops_in.as_deref().unwrap();
        assert_eq!(crate::quests::area_playfield(area), Some(5210));
        let shape = crate::gamedata::get().areas[area].shape.as_ref().unwrap();
        // a Quarry troll (spawn/5210/troll-melee-swamp), the forest's arrival point
        assert!(shape.contains([277.2, 50.45, 54.7], crate::quests::HEIGHT_SLACK));
        assert!(!shape.contains([32.56, 80.33, 139.64], crate::quests::HEIGHT_SLACK));
    }

    #[test]
    fn cinematics_in_the_content() {
        let all = &crate::gamedata::get().tunables.story.cinematics;
        // the narrated ones without the client-side tutorial ending
        assert_eq!(all.len(), 15);
        assert_eq!(all["pirate-intro"].effect_package, 636_984);
        assert_eq!(all["pirate-intro"].then_dialogue.as_deref(), Some("dialogue/video-game-guy-812"));
        assert!(all.values().all(|d| d.length.to_std() > Duration::from_secs(20)));
        assert_eq!(find_cinematic("636984").as_deref(), Some("pirate-intro"));
        assert_eq!(find_cinematic("lost").as_deref(), Some("lost-creations-portal"));
        // every one is reached by a trigger or a script step
        let c = crate::gamedata::get();
        for name in all.keys() {
            let triggered = c.triggers.values().any(|t| t.actions.iter().any(|a| matches!(a, lmoxide_content::Action::Cinematic { name: n } if n == name)));
            assert!(triggered || ["becoming-a-pirate", "after-t-rex"].contains(&name.as_str()), "{name}");
        }
    }

    #[test]
    fn the_live_record() {
        // LMO_Captures.cap: S->C EffectPackageIIR (player) 0000000000000000 0009b838 0000 0000
        // 00000000 00 <instance> 00000000 00000000 00000000 00000000
        let player = Id32::new(50000, 0x010a_b3af);
        let r = record(player, 636_984);
        let body = &r[13..];
        assert_eq!(&body[..12], &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0x09, 0xb8, 0x38]);
        assert_eq!(&body[12..21], &[0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&body[25..], &[0u8; 16][..]);
    }

    fn names(records: &[Bytes]) -> Vec<String> {
        records.iter().map(|r| lmoxide_net::iir::name(u32::from_be_bytes(r[..4].try_into().unwrap())).unwrap_or("?").to_string()).collect()
    }

    fn step_of(c: &Character, main: u32) -> Option<usize> {
        current(c, main, line(main).unwrap())
    }

    #[test]
    fn story_goals() {
        let mut c = crate::game::new_character("t".into(), crate::instance::Mode::Progression, 6200);
        let player = Id32::new(50000, 7);
        let at = Pf::main(6200);
        // GBWD before the Dino story started: not counted
        handle(Order::Progress { hash: "GBWD".into(), amount: 1, wait: false }, &mut c, player, "t", at);
        assert!(!c.story.contains_key("count/GBWD"));
        // "Take the plane to Dinosaur Rise" resolved: the line starts past it (its first step,
        // the Video Game Guy's, has nothing that resolves it here)
        let r = handle(Order::Goal { hash: "6BRI".into(), requires: vec![] }, &mut c, player, "t", at);
        assert!(c.story.contains_key("main/122") && resolved(&c, "6BRI"));
        assert_eq!(names(&r), ["JournalUpdateIIR", "JournalUpdateIIR", "MainQuestUpdateIIR_c", "QuestFullUpdateIIR"]);
        assert_eq!(step_of(&c, 122), Some(2));
        // the plane parts' reward: the Pilot's task solved, the piece waits for its step (1699's buff)
        handle(Order::Solve("YPFX".into()), &mut c, player, "t", at);
        handle(Order::Progress { hash: "GBWD".into(), amount: 1, wait: true }, &mut c, player, "t", at);
        assert!(resolved(&c, "YPFX") && c.story.get("wait/GBWD") == Some(&1));
        // another piece before its step: lost (the Lua checks hasgoal)
        handle(Order::Progress { hash: "GBWD".into(), amount: 1, wait: false }, &mut c, player, "t", at);
        assert_eq!(c.story.get("wait/GBWD"), Some(&1));
        // Dino Tracker found: the pieces' step, the waiting one counted
        let r = handle(Order::Goal { hash: "I8D5".into(), requires: vec![] }, &mut c, player, "t", at);
        assert_eq!(names(&r), ["QuestGoalIIR", "QuestIIR_c", "JournalUpdateIIR", "QuestIIR_c", "QuestFullUpdateIIR", "QuestGoalIIR"]);
        assert_eq!(c.story.get("count/GBWD"), Some(&1));
        for _ in 0..2 {
            handle(Order::Progress { hash: "GBWD".into(), amount: 1, wait: false }, &mut c, player, "t", at);
        }
        assert!(!resolved(&c, "GBWD"));
        let r = handle(Order::Progress { hash: "GBWD".into(), amount: 1, wait: false }, &mut c, player, "t", at);
        assert!(resolved(&c, "GBWD"));
        assert_eq!(names(&r), ["QuestGoalIIR", "QuestIIR_c", "JournalUpdateIIR", "QuestIIR_c", "QuestFullUpdateIIR"]);
        // an earlier step's goal changes nothing
        assert!(handle(Order::Goal { hash: "6BRI".into(), requires: vec![] }, &mut c, player, "t", at).is_empty());
        // a goal that needs a tag first
        handle(Order::Goal { hash: "PM25".into(), requires: vec![316] }, &mut c, player, "t", Pf::main(1200));
        assert!(!resolved(&c, "PM25"));
        // the tutorial's spawn starts the Pirate World line at its first step
        let r = handle(Order::Spawn("S1M7".into()), &mut c, player, "t", Pf::main(1300));
        assert_eq!(names(&r), ["MainQuestUpdateIIR_c", "QuestFullUpdateIIR"]);
        assert_eq!(step_of(&c, 45), Some(0));
    }

    #[test]
    fn pirate_story_as_live() {
        let mut c = crate::game::new_character("t".into(), crate::instance::Mode::Progression, 1300);
        let player = Id32::new(50000, 0x010a_b3af);
        handle(Order::Spawn("S1M7".into()), &mut c, player, "t", Pf::main(4200));
        // the main quest as live listed it: the quests and their countdown 21..0, type 5
        let q = line(45).unwrap();
        let l = view(q, 45, Pf::main(4200));
        let r = crate::quests::main_quest_added(player.instance, &l, 0);
        let body = &r[13..];
        assert_eq!(&body[..12], &[0, 0, 0, 1, 0, 0, 0, 45, 0, 0, 1, 23]);
        let order: Vec<(u32, u8)> = q.steps.iter().enumerate().map(|(i, s)| (s.quest, (21 - i) as u8)).collect();
        assert_eq!(&order[..4], &[(279, 21), (78, 20), (186, 19), (79, 18)]);
        assert_eq!(&body[body.len() - 12..], &[0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0, 1]);
        // the tutorial's boss: "Follow the Video Game Guy" done, "Travel to the Pirate World" next
        let boss = crate::achievements::Event::Defeated { id: Id32::default(), name: "Captain Dreadleg".into(), smashable: false, boss: true, champion: false, level: 1, playfield: 1300 };
        let r = on_event(&mut c, player.instance, Pf::main(1300), &boss);
        assert_eq!(names(&r), ["QuestGoalIIR", "QuestIIR_c", "JournalUpdateIIR", "QuestIIR_c", "QuestFullUpdateIIR"]);
        assert_eq!(&r[0][13..], &[0, 0, 1, 0x17, 0, 0, 1, 0x4a, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0][..]);
        // the first step's journal entry as live sent it (LMO_Captures.cap: quest 279 at
        // distance 21, repeat 1; without the time, not kept): added to main 45, which the
        // journal at login has (the windows' quests) ...
        let hex = |r: &Bytes| r[13..].iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert!(hex(&r[2]).starts_with("00000040223e082d123a0897021000201528013"), "{}", hex(&r[2]));
        // ... else main 45 with its region names, as live
        let first = crate::quests::journal_record(player.instance, 45, 279, Some([4200, 4000]), 21, 1);
        assert!(hex(&first).starts_with("0000004e1a4c082d10a0960318e82020a0960328a01f323a0897021000201528013"), "{}", hex(&first));
        // the next step's QuestFullUpdateIIR as live sent it in Pirate World (quest 78, flags 2,
        // first 279, story, region (52000, 4600) (52000, 4000), the line's reward 462486 in the
        // second reward list, goal 116's marker with its district 1396) up to the goal's texts
        assert_eq!(names(&r[4..]), ["QuestFullUpdateIIR"]);
        const LIVE: &str = "00000001010ab3af000000010000004e00000001000000000000000100000002000000010000002d00000117000000cb20000011f80000cb2000000fa00000000100000000000000000000000000000000000000000000000000000000000000000000000000000001010ab3af0000004e000f44ac00070e964f1ea946840cb531cacbfb13fa6024e1000f44ac00000000b290805c29e627cab290805c29e627ca000000010000000100000000000000010000000100000009010ab3af0000004e000000740000000100000000000000010000000000000000000000000000000000000000000000000000000000000574000011f8427e00004178000040a00000428dcccd000000";
        let full = crate::quests::full_update(player.instance, &view(q, 45, Pf::main(4600)), 1, 0, false);
        assert_eq!(hex(&full)[..LIVE.len()], *LIVE);
        assert!(hex(&full).ends_with("0000000000"));
        // the cannon, then Oops: two steps in one dialogue text (211)
        handle(Order::Goal { hash: "OOPN".into(), requires: vec![] }, &mut c, player, "t", Pf::main(4600));
        handle(Order::Goal { hash: "X7OF".into(), requires: vec![] }, &mut c, player, "t", Pf::main(4600));
        handle(Order::Goal { hash: "0JSA".into(), requires: vec![] }, &mut c, player, "t", Pf::main(4600));
        assert_eq!(q.steps[step_of(&c, 45).unwrap()].quest, 185);
        // a much later goal jumps there; every step between counts as done
        let r = handle(Order::Goal { hash: "KSKL".into(), requires: vec![] }, &mut c, player, "t", Pf::main(4200));
        assert_eq!(q.steps[step_of(&c, 45).unwrap()].quest, 179);
        assert_eq!(names(&r).iter().filter(|n| *n == "JournalUpdateIIR").count(), 10);
        // the last: removed from the tracker
        handle(Order::SetStep { main: 45, step: 22 }, &mut c, player, "t", Pf::main(1200));
        let r = handle(Order::Advance(Some(45)), &mut c, player, "t", Pf::main(1200));
        assert_eq!(names(&r).last().map(String::as_str), Some("MainQuestUpdateIIR_c"));
        assert_eq!(step_of(&c, 45), None);
        assert!(show(&c, player.instance, Pf::main(1200)).is_empty());
    }

    #[test]
    fn the_journal_and_migration() {
        let mut c = crate::game::new_character("t".into(), crate::instance::Mode::Progression, 4200);
        // a character from before: the tutorial done, thrown into the jail (text 126's tag 308)
        c.tags.insert(crate::tutorial::DONE_TAG, 0);
        c.tags.insert(308, 0);
        migrate(&mut c);
        let q = line(45).unwrap();
        assert_eq!(q.steps[step_of(&c, 45).unwrap()].quest, 177);
        // once only
        c.tags.insert(309, 0);
        migrate(&mut c);
        assert_eq!(q.steps[step_of(&c, 45).unwrap()].quest, 177);
        // the journal: the steps done (with their distance), the windows' quests
        let j = journal(&c);
        let pirate = j.iter().find(|m| m.main == 45).unwrap();
        assert!(pirate.quests.contains(&(279, 21, 1)) && pirate.quests.contains(&(185, 17, 1)));
        assert!(pirate.quests.iter().any(|e| e.0 == 82) && !pirate.quests.iter().any(|e| e.0 == 177));
        // every window's quest (RDB 1000644) and the Lost Creations entry's
        let wanted: Vec<u32> = vec![75, 67, 148, 211, 173, 113, 82, 156, 25, 208, 212, 124, 116, 194, 196, 197, 81, 181, 281, 162, 149, 155, 213];
        for q in wanted {
            assert!(j.iter().any(|m| m.quests.iter().any(|e| e.0 == q)), "{q}");
        }
    }

    #[test]
    fn windows_locked() {
        let mut c = crate::game::new_character("t".into(), crate::instance::Mode::Progression, 4200);
        c.tags.insert(crate::tutorial::DONE_TAG, 0);
        c.tags.insert(308, 0);
        migrate(&mut c);
        let j = journal_with(&c, false);
        // only what the character did: the Pirate World steps up to the jail, no other line
        assert!(j.iter().all(|m| m.main == 45 || crate::quests::completed(&c).iter().any(|q| q.0 == m.main)), "{:?}", j.iter().map(|m| m.main).collect::<Vec<_>>());
        let pirate = j.iter().find(|m| m.main == 45).unwrap();
        assert!(pirate.quests.contains(&(279, 21, 1)) && !pirate.quests.iter().any(|e| e.0 == 82));
        assert!(journal_with(&c, true).iter().any(|m| m.quests.iter().any(|e| e.0 == 82)));
        // the server's setting over the content's (true)
        assert!(windows_unlocked());
    }

    #[test]
    fn a_players_reset() {
        let mut c = crate::game::new_character("t".into(), crate::instance::Mode::Progression, 5200);
        let player = Id32::new(crate::territory::CHARACTER_TYPE, 9);
        let at = Pf::main(5200);
        c.tags.insert(crate::tutorial::DONE_TAG, 0);
        // the Kingdom's story: the King's Court seen (its tag 425), "Go to the Kingdom" resolved
        c.tags.insert(425, 0);
        c.tags.insert(4285, 0);
        let achievement = *crate::achievements::player_counters(&c).first().map(|(s, _)| s).unwrap_or(&1613);
        c.tags.insert(achievement, 0);
        handle(Order::Goal { hash: "CPIJ".into(), requires: vec![] }, &mut c, player, "t", at);
        let medieval = find_line("medieval").unwrap();
        assert!(step_of(&c, medieval).is_some());
        let (records, rpcs) = player_reset(&mut c, player);
        assert_eq!(names(&records), ["MainQuestUpdateIIR_c"]);
        assert_eq!(rpcs.len(), 2);
        assert!(!c.tags.contains_key(&425) && !c.tags.contains_key(&4285));
        assert!(c.tags.contains_key(&crate::tutorial::DONE_TAG) && c.tags.contains_key(&achievement));
        assert!(!started(&c, medieval) && c.story.keys().all(|k| k == "chain"));
        // the talking heads' and cinematics' flags are story tags; achievements are not
        let tags = story_tags();
        assert!(tags.contains(&621) && tags.contains(&4285) && tags.contains(&425) && !tags.contains(&1613));
    }

    /// After a reset, arriving in a world through its window puts the line there: the arrival
    /// point lies in the intro cinematic's area, whose first conversation resolves a goal of the
    /// world's line (a later goal jumps the line there); Pirate World's first goal there,
    /// "Travel to the Pirate World", is its beach area (the live marker).
    #[test]
    fn arriving_puts_the_line_there() {
        let c = crate::gamedata::get();
        for (world, line_name) in [(5200, "medieval"), (3200, "space"), (2210, "mythology"), (6200, "dino")] {
            let main = find_line(line_name).unwrap();
            let arrival = crate::teleport::destination(world).unwrap().pos;
            let intro = c
                .triggers
                .values()
                .filter(|t| !t.removed && t.area.as_ref().and_then(|a| c.areas.get(a)).and_then(|a| a.shape.as_ref()).is_some_and(|s| s.contains(arrival, 1.0)))
                .find_map(|t| t.actions.iter().find_map(|a| match a {
                    lmoxide_content::Action::Cinematic { name } => Some(name.clone()),
                    _ => None,
                }));
            let intro = intro.unwrap_or_else(|| panic!("{world}: no cinematic at the arrival {arrival:?}"));
            let next = c.tunables.story.cinematics[&intro].then_dialogue.clone().unwrap();
            let d = crate::knubot::by_id(&next).unwrap();
            let goals = crate::knubot::goals_resolved(&d);
            assert!(goals.iter().any(|g| find_goal(g).is_some_and(|(m, ..)| m == main)), "{world} {intro} {next}: {goals:?}");
        }
        let beach = c.triggers.values().find(|t| t.id == "trigger/4600/story-story-reachbeachbackup1").unwrap();
        assert!(beach.actions.iter().any(|a| matches!(a, lmoxide_content::Action::Command { name, args } if name == "story_goal" && args["goal"] == "OOPN")));
        assert_eq!(find_goal("OOPN").map(|(m, ..)| m), Some(45));
    }

    #[test]
    fn story_targets() {
        let p = points();
        assert!(p.iter().any(|(l, pf, _)| *pf == 4600 && l.contains("BA_Cine_Arrival") && l.contains("pirate-intro")), "{p:?}");
        assert!(p.iter().any(|(l, _, _)| l.contains("tag ")));
    }
}
