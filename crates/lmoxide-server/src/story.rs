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
//! - **Story goals**: dialogue texts' `resolvegoal` / `spawn`, the `story_goal` command (the
//!   Story_* locate areas), the area quests' reward scripts (`solvegoal`, QuestTriggerProgress).
//!   Kept per character (`cinematic/`, `goal/`, `count/`, `main/` keys); the story quests aren't
//!   shown in the tracker yet (the journal lists every story line as done: teleport.rs).
//!
//! Trigger commands (`register_command`): `cinematic {name}`, `dialogue {dialogue}`, `tag {tag}`,
//! `story_goal {goal, requires}`, `story_beat {text}`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

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
    /// `QuestTriggerProgress(hash, amount)`: only if the player has that goal.
    Progress { hash: String, amount: u32 },
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
}

/// Tell `player`'s connection to do `order`.
pub fn send(player: Id32, order: Order) {
    world::send_to(player, world::Out::Story(order));
}

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

/// A story goal by hash: (main quest, its goal id, how many count).
fn goal_of(hash: &str) -> Option<(u32, u32, u32)> {
    let c = crate::gamedata::get();
    c.quests.values().filter(|q| q.kind == lmoxide_content::QuestKind::Story).find_map(|q| {
        let g = q.steps.iter().flat_map(|s| &s.goals).find(|g| g.hash.as_deref() == Some(hash))?;
        Some((q.main?, g.goal, g.count.max(1)))
    })
}

/// The story line a quest hash (`spawn <hash>`) starts.
fn main_of_quest(hash: &str) -> Option<u32> {
    let c = crate::gamedata::get();
    c.quests.values().filter(|q| q.kind == lmoxide_content::QuestKind::Story).find_map(|q| q.steps.iter().any(|s| s.hash.as_deref() == Some(hash)).then_some(q.main?))
}

fn resolved(c: &Character, hash: &str) -> bool {
    c.story.contains_key(&format!("goal/{hash}"))
}

/// Whether the player "has" story goal `hash` (the client's Lua `player:hasgoal`): its story
/// line started and the goal not resolved yet. (Which step of the line they are on isn't kept:
/// the order is only known for the Pirate World.)
fn has_goal(c: &Character, hash: &str) -> bool {
    let Some((main, ..)) = goal_of(hash) else { return false };
    c.story.contains_key(&format!("main/{main}")) && !resolved(c, hash)
}

fn resolve(c: &mut Character, character: u32, hash: &str, why: &str) {
    let Some((main, goal, _)) = goal_of(hash) else {
        return info!(character, hash, why, "story: not a story goal");
    };
    let now = unix_now();
    c.story.entry(format!("main/{main}")).or_insert(now);
    if c.story.insert(format!("goal/{hash}"), now).is_none() {
        info!(character, main, goal, hash, why, "story goal resolved");
    }
}

/// Does `order` for the player (game.rs, with the character).
pub fn handle(order: Order, c: &mut Character, player: Id32, name: &str, place: Pf) {
    let character = player.instance;
    match order {
        Order::Cinematic(n) => cinematic(c, player, place, &n, false),
        Order::Replay(n) => cinematic(c, player, place, &n, true),
        Order::Goal { hash, requires } => {
            if requires.iter().all(|t| c.tags.contains_key(t)) {
                resolve(c, character, &hash, "trigger or dialogue");
            }
        }
        Order::Solve(hash) => {
            if has_goal(c, &hash) {
                resolve(c, character, &hash, "solvegoal");
            }
        }
        Order::Progress { hash, amount } => {
            if !has_goal(c, &hash) {
                return info!(character, hash, "story progress: the player doesn't have the goal");
            }
            let key = format!("count/{hash}");
            let n = c.story.get(&key).copied().unwrap_or(0) + i64::from(amount);
            c.story.insert(key, n);
            let need = goal_of(&hash).map_or(1, |g| g.2);
            info!(character, hash, n, need, "story goal progress");
            if n >= i64::from(need) {
                resolve(c, character, &hash, "counted");
            }
        }
        Order::Spawn(hash) => match main_of_quest(&hash) {
            Some(main) => {
                if c.story.insert(format!("main/{main}"), unix_now()).is_none() {
                    info!(character, main, hash, "story line started");
                }
            }
            None => info!(character, hash, "spawn: not a story quest (an NPC template?)"),
        },
        Order::Dialogue(id) => {
            let Some(first) = crate::gamedata::get().dialogues.get(&id).and_then(|d| d.texts.first().copied()) else {
                return info!(character, dialogue = id, "story: no such dialogue");
            };
            let Some(d) = crate::knubot::starting_with(first) else { return };
            let gives = crate::knubot::tags_given(&d);
            if !gives.is_empty() && gives.iter().all(|t| c.tags.contains_key(t)) {
                return;
            }
            let owner = crate::npc::find(&d.npc, place).unwrap_or(player);
            crate::knubot::start(player, name, owner, d);
        }
        Order::Beat { text, requires } => {
            let gives = crate::knubot::text_tags(text);
            if (!gives.is_empty() && gives.iter().all(|t| c.tags.contains_key(t))) || !requires.iter().all(|t| c.tags.contains_key(t)) {
                return;
            }
            crate::knubot::beat(player, text);
        }
        Order::TagsIfGoal { tags, goals } => {
            if goals.iter().any(|g| has_goal(c, g)) {
                for tag in tags {
                    world::send_to(player, world::Out::Progress(crate::achievements::Event::Tag(tag)));
                }
            }
        }
        Order::Pickup(template) => {
            let quests = &crate::gamedata::get().quests;
            let goals: Vec<String> = quests
                .values()
                .filter(|q| q.kind == lmoxide_content::QuestKind::Story)
                .flat_map(|q| q.steps.iter().flat_map(|s| &s.goals))
                .filter(|g| g.counts.as_deref() == Some("pickups") && g.pickups.contains(&template))
                .filter_map(|g| g.hash.clone())
                .collect();
            for hash in goals {
                handle(Order::Progress { hash, amount: 1 }, c, player, name, place);
            }
        }
    }
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
    lines.push(format!("story lines started: {}", keys("main/").iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")));
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

    #[test]
    fn story_goals() {
        let mut c = crate::game::new_character("t".into(), crate::instance::Mode::Progression, 6200);
        let player = Id32::new(50000, 7);
        // GBWD before the Dino story started: not counted
        handle(Order::Progress { hash: "GBWD".into(), amount: 1 }, &mut c, player, "t", Pf::main(6200));
        assert!(!c.story.contains_key("count/GBWD"));
        // "Take the plane to Dinosaur Rise" resolved: the line has started
        handle(Order::Goal { hash: "6BRI".into(), requires: vec![] }, &mut c, player, "t", Pf::main(6200));
        assert!(c.story.contains_key("main/122") && resolved(&c, "6BRI"));
        // the Pilot's task solved by the plane parts' reward, then the four pieces
        handle(Order::Solve("YPFX".into()), &mut c, player, "t", Pf::main(6200));
        assert!(resolved(&c, "YPFX"));
        for _ in 0..3 {
            handle(Order::Progress { hash: "GBWD".into(), amount: 1 }, &mut c, player, "t", Pf::main(6200));
        }
        assert!(!resolved(&c, "GBWD"));
        handle(Order::Progress { hash: "GBWD".into(), amount: 1 }, &mut c, player, "t", Pf::main(6200));
        assert!(resolved(&c, "GBWD"));
        // a goal that needs a tag first
        handle(Order::Goal { hash: "PM25".into(), requires: vec![316] }, &mut c, player, "t", Pf::main(1200));
        assert!(!resolved(&c, "PM25"));
        handle(Order::Spawn("WPZW".into()), &mut c, player, "t", Pf::main(4600));
        assert!(c.story.contains_key("main/45"));
    }

    #[test]
    fn story_targets() {
        let p = points();
        assert!(p.iter().any(|(l, pf, _)| *pf == 4600 && l.contains("BA_Cine_Arrival") && l.contains("pirate-intro")), "{p:?}");
        assert!(p.iter().any(|(l, _, _)| l.contains("tag ")));
    }
}
