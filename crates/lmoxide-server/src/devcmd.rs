//! Test commands typed into the normal ("Say") chat: a line starting with `/` or `!` is a
//! command for the server and isn't relayed to other players. Free-text chat needs a GM level
//! (`--gm-level`, set by tools/lan-server.sh). Answers come back as chat lines from "Server".
//!
//!   !help                     the commands
//!   !where                    current playfield and position
//!   !tp <playfield|name>      go to a playfield's arrival point (number or part of its name)
//!   !list <kind>              numbered list of a kind of test target (see KINDS)
//!   !goto <kind> [n]          go next to target n (default 1) of that kind
//!   !ach                      achievements earned, kill and smash counters
//!   !count <kills|smashes|builds> [n] [name]   count n (default 1) defeats/builds for achievements
//!   !defeat <boss name>       count a boss defeat (boss achievements, the active minifigure's)
//!   !xp [n]                   gain n XP (with --progression), show level and XP
//!   !stars <n>                gain n stars (minifigure XP for ranking up)
//!   !test [n|label]           list the manual test setups, or set one up (data/test_scenarios.json)
//!   !aura <0-4>               the active minifigure's aura tier (gives/takes its aura tags)
//!   !dialog [n]               play recorded NPC dialogue n (without n: list them)
//!   !mirrors <solve|reset>    turn this playfield's Medusa mirrors to the solution or the start
//!   !plates <solve|reset>     light every plate of the Gauntlet's plate room, or none
//!   !revive                   every minifigure (team and collection) back to full health
//!   !smashme                  smash the active minifigure (it rebuilds after the smash timer)
//!   !loot <kind>              drop a loot bag at your feet: potion, part, bag, chest, memberchest,
//!                             boss (a boss's roll: free and member chest), enemy, smashable
//!   !own [name]               obtain a minifigure (as from a bag); without a name: how many you own
//!   !collection <limit|all>   own only the team and the members' minifigures, or every one
//!                             (relog to see it in the Collection)
//!   !fx <id> [hook]           play effect package <id> on yourself (optionally from a code hook,
//!                             e.g. limbostart); !fx next|prev steps through every package
//!                             (data/effect_catalog.json), !fx stop ends the current one
//!   !skin <monsterdata|reset> look like any monster data (others see it too), reset: your minifigure
//!   !state <n>                set your BCCState (stat 20: 1 normal, 29 Limbo/smashed, 104 building, ...)
//!   !spawn [count] <name>     spawn count (default 1, at most 30) of an enemy or smashable around you
//!                             for 15 minutes, as !test does
//!
//! `COMMANDS` lists them for `!help` and for the control interface's catalog (`catalog`).
//!
//! New features add their test targets to `targets` so they can be reached the same way.

use lmoxide_net::types::Id32;

use crate::teleport;

/// What the game connection should do for a command.
pub enum Action {
    Reply(Vec<String>),
    /// teleport to (playfield, position, facing), with a reply
    Teleport(u32, [f32; 3], f32, String),
    /// show the achievement summary
    Achievements,
    /// smash the nearest smashable (or defeat the nearest enemy) as if the player hit it
    Smash,
    /// bring every minifigure of the player (team and collection) back from smashed, full health
    Revive,
    /// feed achievement events (n times)
    Progress(crate::achievements::Event, u32),
    /// gain XP
    Xp(u64),
    /// gain stars
    Stars(u64),
    /// set up a manual test (`!test`)
    Test(TestSetup),
    /// give the active minifigure's first n aura tags (0-4), taking the others away
    Aura(u32),
    /// play recorded dialogue n (0: list them)
    Dialog(usize),
    /// turn this playfield's mirrors to the solution (true) or the start
    Mirrors(bool),
    /// smash the active minifigure
    SmashMe,
    /// drop a loot bag of a kind (`!loot`)
    Loot(String),
    /// obtain a minifigure (`None`: tell how many are owned)
    Own(Option<u32>),
    /// limit the collection (true) or own every minifigure
    LimitCollection(bool),
    /// play / step through / stop effect packages (`!fx`)
    Fx(Fx),
    /// look like a monster data (`None`: the active minifigure again)
    Skin(Option<u32>),
    /// set the player's BCCState
    State(u32),
    /// spawn (name, count) enemies or smashables around the player (`!spawn`)
    Spawn(String, u32),
    /// open a teleport window by id, as a portal would (`!window`)
    Window(u32),
}

/// Every command with its usage and what it does (`!help`, and tools through `catalog`).
/// (`!help` first: anything unknown answers with the list.)
pub const COMMANDS: &[(&str, &str)] = &[
    ("!help", "the commands"),
    ("!where", "current playfield and position"),
    ("!tp <playfield|name>", "go to a playfield's arrival point (number or part of its name)"),
    ("!list <kind>", "numbered list of a kind of test target"),
    ("!goto <kind> [n]", "go next to target n (default 1) of that kind"),
    ("!test [n|label]", "list the manual test setups, or set one up (data/test_scenarios.json)"),
    ("!spawn [count] <name>", "spawn count (default 1, at most 30) of an enemy or smashable around you for 15 minutes"),
    ("!loot <kind>", "drop a loot bag at your feet"),
    ("!smash", "smash the nearest smashable (or defeat the nearest enemy) within 15 m"),
    ("!smashme", "smash the active minifigure (it rebuilds after the smash timer)"),
    ("!revive", "every minifigure (team and collection) back to full health"),
    ("!own [name]", "obtain a minifigure (as from a bag); without a name: how many you own"),
    ("!collection <limit|all>", "own only the team and the members' minifigures, or every one (relog to see it)"),
    ("!aura <0-4>", "the active minifigure's aura tier: none, Disciple, Master, Grandmaster, Superachiever"),
    ("!xp [n]", "gain n XP (with --progression), show level and XP"),
    ("!stars <n>", "gain n stars (minifigure XP for ranking up)"),
    ("!ach", "achievements earned, kill and smash counters"),
    ("!count <kills|smashes|builds> [n] [name]", "count n (default 1) defeats or builds for achievements"),
    ("!defeat <boss name>", "count a boss defeat (boss achievements, the active minifigure's)"),
    ("!dialog [n]", "play recorded NPC dialogue n (without n: list them)"),
    ("!mirrors <solve|reset>", "turn this playfield's Medusa mirrors to the solution or the start"),
    ("!plates <solve|reset>", "light every plate of the Gauntlet's plate room, or none"),
    ("!fx <id> [hook]", "play effect package <id> on yourself (optionally from a code hook)"),
    ("!fx next|prev|stop", "step through every effect package, or end the current one"),
    ("!skin <monster data|reset>", "look like any monster data (others see it too); reset: your minifigure"),
    ("!state <n>", "set your BCCState (1 normal, 29 Limbo/smashed, 104 building, ...)"),
    ("!window <id>", "open teleport window <id> (RDB 1000644), as a portal would"),
];

/// Most a `!spawn` brings at once.
const MAX_SPAWN: u32 = 30;

/// What the commands accept, for the control interface (`GET /v1/test-commands`).
pub fn catalog() -> lmoxide_control::TestCatalog {
    let (spawnable, looks) = crate::npc::test_catalog();
    lmoxide_control::TestCatalog {
        commands: COMMANDS.iter().map(|(usage, about)| lmoxide_control::TestCommand { usage: usage.to_string(), about: about.to_string() }).collect(),
        kinds: KINDS
            .iter()
            .map(|(kind, about)| lmoxide_control::TargetKind {
                kind: kind.to_string(),
                about: about.to_string(),
                targets: targets(kind)
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(i, (label, playfield, pos))| lmoxide_control::Target { n: i as u32 + 1, label, playfield, pos })
                    .collect(),
            })
            .collect(),
        scenarios: SCENARIOS
            .scenarios
            .iter()
            .enumerate()
            .map(|(i, s)| lmoxide_control::TestScenario {
                n: i as u32 + 1,
                label: s.label.clone(),
                about: s.about.clone(),
                team: s.team.clone(),
                spawn: s.spawn.iter().map(|sp| lmoxide_control::SpawnCount { name: sp.name.clone(), count: sp.count }).collect(),
            })
            .collect(),
        dialogues: crate::knubot::dialogues()
            .iter()
            .enumerate()
            .map(|(i, d)| lmoxide_control::TestDialogue { n: i as u32 + 1, npc: d.npc.clone(), playfield: d.playfield })
            .collect(),
        loot_kinds: crate::loot::TEST_KINDS.iter().map(|k| k.to_string()).collect(),
        spawnable,
        looks,
    }
}

/// `!fx` arguments.
pub enum Fx {
    Play(u32, String),
    Step(i32),
    Stop,
}

/// Every effect package id (data/effect_package_ids.json, from tools/re/extract_effect_catalog.py).
pub fn effect_packages() -> &'static [u32] {
    static IDS: std::sync::LazyLock<Vec<u32>> =
        std::sync::LazyLock::new(|| serde_json::from_str(include_str!("../../../data/effect_package_ids.json")).expect("effect_package_ids.json"));
    &IDS
}

/// A resolved test scenario (data/test_scenarios.json).
pub struct TestSetup {
    pub label: String,
    /// where to go (playfield, position, facing); `None` = stay
    pub destination: Option<(u32, [f32; 3], f32)>,
    /// the new team's minifigure templates, the first active; empty = keep the team
    pub team: Vec<u32>,
    /// enemies to spawn around the player: (name, count)
    pub spawn: Vec<(String, u32)>,
}

#[derive(serde::Deserialize)]
struct Scenarios {
    scenarios: Vec<Scenario>,
}

#[derive(serde::Deserialize)]
struct Scenario {
    label: String,
    about: String,
    #[serde(default)]
    goto: Option<Goto>,
    #[serde(default)]
    team: Vec<String>,
    #[serde(default)]
    spawn: Vec<Spawn>,
}

#[derive(serde::Deserialize)]
struct Goto {
    kind: Option<String>,
    name: Option<String>,
    n: Option<usize>,
    playfield: Option<u32>,
    pos: Option<[f32; 3]>,
}

#[derive(serde::Deserialize)]
struct Spawn {
    name: String,
    count: u32,
}

static SCENARIOS: std::sync::LazyLock<Scenarios> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../data/test_scenarios.json")).expect("test_scenarios.json")
});

/// `!test` without an argument lists the scenarios; with a number or label, resolves it.
fn test(arg: Option<&str>) -> Action {
    let list = &SCENARIOS.scenarios;
    let Some(arg) = arg else {
        let mut lines = vec!["!test <n|label>:".to_string()];
        lines.extend(list.iter().enumerate().map(|(i, s)| format!("{} {}: {}", i + 1, s.label, s.about)));
        return Action::Reply(lines);
    };
    let found = arg.parse::<usize>().ok().and_then(|n| list.get(n.wrapping_sub(1))).or_else(|| {
        let a = arg.to_ascii_lowercase();
        list.iter().find(|s| s.label.to_ascii_lowercase() == a)
    });
    let Some(s) = found else {
        return Action::Reply(vec![format!("no test \"{arg}\" (!test lists them)")]);
    };
    let destination = match &s.goto {
        None => None,
        Some(Goto { playfield: Some(pf), pos, .. }) => match (pos, teleport::destination(*pf)) {
            (Some(p), _) => Some((*pf, *p, 0.0)),
            (None, Some(d)) => Some((*pf, d.pos, d.rot)),
            (None, None) => return Action::Reply(vec![format!("test {}: playfield {pf} has no arrival point", s.label)]),
        },
        Some(Goto { kind: Some(kind), name, n, .. }) => {
            let mut t = targets(kind).unwrap_or_default();
            if let Some(name) = name {
                let name = name.to_ascii_lowercase();
                t.retain(|(label, _, _)| label.to_ascii_lowercase().starts_with(&name));
            }
            match t.get(n.unwrap_or(1).saturating_sub(1)) {
                Some((_, pf, p)) => Some((*pf, [p[0] + ARRIVE_OFFSET, p[1], p[2]], (-ARRIVE_OFFSET).atan2(0.0))),
                None => return Action::Reply(vec![format!("test {}: no such {kind} target", s.label)]),
            }
        }
        Some(_) => return Action::Reply(vec![format!("test {}: goto needs kind or playfield", s.label)]),
    };
    let mut team = Vec::new();
    for name in &s.team {
        match crate::collection::template_by_name(name) {
            Some(t) => team.push(t),
            None => return Action::Reply(vec![format!("test {}: no minifigure named \"{name}\"", s.label)]),
        }
    }
    let spawn = s.spawn.iter().map(|sp| (sp.name.clone(), sp.count.min(30))).collect();
    Action::Test(TestSetup { label: s.label.clone(), destination, team, spawn })
}

/// A command's target kinds (for `!list` / `!goto`) and what they are.
const KINDS: &[(&str, &str)] = &[
    ("buildie", "buildables (buildie.rs)"),
    ("pad", "jump pads"),
    ("lever", "levers (scenery)"),
    ("plate", "pressure plates (scenery)"),
    ("gate", "gates and portcullis (scenery)"),
    ("mirror", "Medusa mirrors (scenery)"),
    ("collectible", "collectibles (achievements)"),
    ("boombox", "boomboxes (achievements)"),
    ("arena", "boss arena starts (encounters)"),
    ("ambush", "ambush areas (encounters)"),
    ("explore", "exploration achievement areas"),
    ("portal", "portals (walk 8 m away after arriving, then step in)"),
    ("trigger", "tutorial triggers (1300; listed 6 m off, so !goto and the tour land inside)"),
    ("boss", "dungeon bosses"),
    ("champion", "champions"),
    ("enemy", "all enemies"),
    ("npc", "friendly NPCs"),
    ("fighter", "friendly NPCs fighting enemies (npc_fights.json)"),
    ("smashable", "smashables"),
];

/// How far from a target to arrive (so as not to stand inside it or on a pad).
const ARRIVE_OFFSET: f32 = 3.0;

/// Whether a chat line is a command.
pub fn is_command(message: &str) -> bool {
    message.starts_with('/') || message.starts_with('!')
}

/// The targets of a kind: (label, playfield, position).
fn targets(kind: &str) -> Option<Vec<(String, u32, [f32; 3])>> {
    Some(match kind {
        "buildie" => crate::buildie::points().into_iter().map(|(n, pf, p, state)| (format!("{n} [{state}]"), pf, p)).collect(),
        "pad" => crate::jumppad::points(),
        "lever" => crate::scenery::points("lever"),
        "plate" => crate::scenery::points("pressureplate"),
        "gate" => [crate::scenery::points("gate"), crate::scenery::points("portcullis")].concat(),
        "mirror" => crate::scenery::points("mirror"),
        "collectible" => crate::discovery::points(true),
        "boombox" => crate::discovery::points(false),
        "trigger" => crate::tutorial::points(),
        "portal" => teleport::points(),
        "arena" | "ambush" | "explore" => crate::encounter::points(kind),
        "boss" | "champion" | "enemy" | "npc" | "fighter" | "smashable" => crate::npc::points(kind),
        _ => return None,
    })
}

fn playfield_name(playfield: u32) -> String {
    teleport::destination(playfield).map_or_else(|| playfield.to_string(), |d| format!("{} ({playfield})", d.name))
}

/// The targets of a kind, for the development tour (`LMOXIDE_DEV_TOUR`).
/// With LMOXIDE_DEV_TOUR_PER_PLAYFIELD=1, only the first target of each playfield.
pub fn tour_targets(kind: &str) -> Vec<(String, u32, [f32; 3])> {
    let mut targets = targets(kind).unwrap_or_default();
    if std::env::var("LMOXIDE_DEV_TOUR_PER_PLAYFIELD").is_ok_and(|v| v == "1") {
        let mut seen = std::collections::HashSet::new();
        targets.retain(|t| seen.insert(t.1));
    }
    targets
}

/// Runs a command typed by a player standing at `pos` on `playfield`.
pub fn run(message: &str, playfield: u32, pos: [f32; 3]) -> Action {
    let mut words = message[1..].split_whitespace();
    let cmd = words.next().unwrap_or("").to_ascii_lowercase();
    let args: Vec<&str> = words.collect();
    let reply = |lines: Vec<String>| Action::Reply(lines);
    match cmd.as_str() {
        "where" => reply(vec![format!("{}: {:.1} {:.1} {:.1}", playfield_name(playfield), pos[0], pos[1], pos[2])]),
        "tp" => {
            let query = args.join(" ");
            if query.is_empty() {
                return reply(vec!["usage: !tp <playfield number or name>".into()]);
            }
            let found = query.parse::<u32>().ok().filter(|pf| teleport::destination(*pf).is_some()).or_else(|| {
                let q = query.to_ascii_lowercase();
                teleport::destinations().find(|(_, d)| d.name.to_ascii_lowercase().contains(&q)).map(|(pf, _)| pf)
            });
            match found.and_then(|pf| teleport::destination(pf).map(|d| (pf, d))) {
                Some((pf, d)) => Action::Teleport(pf, d.pos, d.rot, format!("going to {}", playfield_name(pf))),
                None => reply(vec![format!("no playfield matches \"{query}\"")]),
            }
        }
        "list" => match args.first().and_then(|k| targets(k).map(|t| (k, t))) {
            Some((kind, t)) if !t.is_empty() => reply(
                t.iter()
                    .enumerate()
                    .map(|(i, (label, pf, _))| format!("{kind} {}: {label} in {}", i + 1, playfield_name(*pf)))
                    .collect(),
            ),
            Some((kind, _)) => reply(vec![format!("no {kind} targets")]),
            None => reply(kinds_help()),
        },
        "goto" => {
            let Some((kind, t)) = args.first().and_then(|k| targets(k).map(|t| (k, t))) else {
                return reply(kinds_help());
            };
            let n = args.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
            match t.get(n.wrapping_sub(1)) {
                Some((label, pf, p)) => {
                    let at = [p[0] + ARRIVE_OFFSET, p[1], p[2]];
                    // face the target (yaw convention as in return points: atan2(dx, dz))
                    let rot = (-ARRIVE_OFFSET).atan2(0.0);
                    Action::Teleport(*pf, at, rot, format!("going to {kind} {n}: {label} in {}", playfield_name(*pf)))
                }
                None => reply(vec![format!("{kind} {n} doesn't exist ({} known)", t.len())]),
            }
        }
        "test" => test(args.first().copied()),
        "dialog" | "dialogue" => Action::Dialog(args.first().and_then(|n| n.parse().ok()).unwrap_or(0)),
        "aura" => match args.first().and_then(|n| n.parse::<u32>().ok()).filter(|n| *n <= 4) {
            Some(n) => Action::Aura(n),
            None => reply(vec!["usage: !aura <0-4> (none, Disciple, Master, Grandmaster, Superachiever)".into()]),
        },
        "ach" | "achievements" => Action::Achievements,
        "xp" => Action::Xp(args.first().and_then(|n| n.parse().ok()).unwrap_or(0).min(10_000_000)),
        "stars" => match args.first().and_then(|n| n.parse::<u64>().ok()) {
            Some(n) => Action::Stars(n.min(10_000_000)),
            None => reply(vec!["usage: !stars <n>".into()]),
        },
        "count" => {
            let n = args.get(1).and_then(|n| n.parse::<u32>().ok()).unwrap_or(1).min(100_000);
            let name = args.get(2..).map(|w| w.join(" ")).filter(|n| !n.is_empty()).unwrap_or_else(|| "Test Dummy".into());
            let event = match args.first().copied() {
                Some("kills") => crate::achievements::Event::Defeated { id: Id32::new(0, 0), name, smashable: false, boss: false, champion: false, level: 1, playfield },
                Some("smashes") => crate::achievements::Event::Defeated { id: Id32::new(0, 0), name, smashable: true, boss: false, champion: false, level: 1, playfield },
                Some("builds") => crate::achievements::Event::Built { name },
                _ => return reply(vec!["usage: !count <kills|smashes|builds> [n] [name]".into()]),
            };
            Action::Progress(event, n)
        }
        "smash" => Action::Smash,
        "revive" => Action::Revive,
        "smashme" => Action::SmashMe,
        "loot" => match args.first().copied() {
            Some(kind) if crate::loot::TEST_KINDS.contains(&kind) => Action::Loot(kind.to_string()),
            _ => reply(vec![format!("usage: !loot <{}>", crate::loot::TEST_KINDS.join("|"))]),
        },
        "own" if args.is_empty() => Action::Own(None),
        "own" => match crate::collection::template_by_name(&args.join(" ")) {
            Some(t) => Action::Own(Some(t)),
            None => reply(vec![format!("no minifigure named {}", args.join(" "))]),
        },
        "fx" => match args.first().copied() {
            Some("next") => Action::Fx(Fx::Step(1)),
            Some("prev") => Action::Fx(Fx::Step(-1)),
            Some("stop") => Action::Fx(Fx::Stop),
            Some(n) if n.parse::<u32>().is_ok() => Action::Fx(Fx::Play(n.parse().unwrap(), args.get(1).copied().unwrap_or("").to_string())),
            _ => reply(vec!["usage: !fx <id> [code hook] | next | prev | stop".into()]),
        },
        "state" => match args.first().and_then(|n| n.parse().ok()) {
            Some(n) => Action::State(n),
            None => reply(vec!["usage: !state <BCCState number> (1 normal, 29 Limbo)".into()]),
        },
        "window" => match args.first().and_then(|n| n.parse().ok()) {
            Some(n) => Action::Window(n),
            None => reply(vec!["usage: !window <teleport window id>".into()]),
        },
        "spawn" => {
            // `!spawn 3 Crate` or `!spawn Captain Dreadleg`
            let (count, name) = match args.first().and_then(|n| n.parse::<u32>().ok()) {
                Some(n) => (n, args[1..].join(" ")),
                None => (1, args.join(" ")),
            };
            if name.is_empty() || count == 0 {
                return reply(vec!["usage: !spawn [count] <enemy or smashable name>".into()]);
            }
            Action::Spawn(name, count.min(MAX_SPAWN))
        }
        "skin" => match args.first().copied() {
            Some("reset") => Action::Skin(None),
            Some(n) if n.parse::<u32>().is_ok() => Action::Skin(n.parse().ok()),
            _ => reply(vec!["usage: !skin <monster data id> | reset".into()]),
        },
        "collection" => match args.first().copied() {
            Some(how @ ("limit" | "all")) => Action::LimitCollection(how == "limit"),
            _ => reply(vec!["usage: !collection <limit|all>".into()]),
        },
        "plates" => match args.first().copied() {
            Some(how @ ("solve" | "reset")) => {
                crate::plates::set_all(how == "solve");
                reply(vec![format!("plates: {how}")])
            }
            _ => reply(vec!["usage: !plates <solve|reset>".into()]),
        },
        "mirrors" => match args.first().copied() {
            Some(how @ ("solve" | "reset")) => Action::Mirrors(how == "solve"),
            _ => reply(vec!["usage: !mirrors <solve|reset>".into()]),
        },
        "defeat" if !args.is_empty() => {
            Action::Progress(crate::achievements::Event::Defeated { id: Id32::new(0, 0), name: args.join(" "), smashable: false, boss: true, champion: false, level: 1, playfield }, 1)
        }
        _ => reply(vec![
            format!("commands: {}", COMMANDS.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")),
            format!("kinds: {}", KINDS.iter().map(|k| k.0).collect::<Vec<_>>().join(", ")),
        ]),
    }
}

fn kinds_help() -> Vec<String> {
    KINDS.iter().map(|(k, what)| format!("{k}: {what}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scenarios_resolve() {
        assert!(matches!(run("!test", 1200, [0.0; 3]), Action::Reply(l) if l.len() == SCENARIOS.scenarios.len() + 1));
        for (i, s) in SCENARIOS.scenarios.iter().enumerate() {
            // every scenario's minifigures and playfields exist (target kinds need the NPCs)
            if s.goto.as_ref().is_some_and(|g| g.kind.is_some()) {
                continue;
            }
            match run(&format!("!test {}", i + 1), 1200, [0.0; 3]) {
                Action::Test(setup) => assert_eq!(setup.label, s.label),
                Action::Reply(l) => panic!("{}: {l:?}", s.label),
                _ => panic!("{}", s.label),
            }
        }
    }

    #[test]
    fn tp_by_name_and_number() {
        assert!(matches!(run("!tp isle of yarr", 1200, [0.0; 3]), Action::Teleport(4200, ..)));
        assert!(matches!(run("/tp 5200", 1200, [0.0; 3]), Action::Teleport(5200, ..)));
        assert!(matches!(run("!tp nowhere", 1200, [0.0; 3]), Action::Reply(_)));
    }

    #[test]
    fn spawn_takes_an_optional_count() {
        assert!(matches!(run("!spawn 3 Crate", 1200, [0.0; 3]), Action::Spawn(n, 3) if n == "Crate"));
        assert!(matches!(run("!spawn Captain Dreadleg", 1200, [0.0; 3]), Action::Spawn(n, 1) if n == "Captain Dreadleg"));
        assert!(matches!(run("!spawn 500 Crate", 1200, [0.0; 3]), Action::Spawn(_, MAX_SPAWN)));
        assert!(matches!(run("!spawn", 1200, [0.0; 3]), Action::Reply(_)));
        assert!(matches!(run("!spawn 0 Crate", 1200, [0.0; 3]), Action::Reply(_)));
    }

    #[test]
    fn catalog_lists_the_static_choices() {
        let c = catalog();
        assert_eq!(c.scenarios.len(), SCENARIOS.scenarios.len());
        assert_eq!(c.kinds.len(), KINDS.len());
        assert_eq!(c.commands.len(), COMMANDS.len());
        assert!(c.loot_kinds.iter().any(|k| k == "potion"));
        assert_eq!(c.scenarios[0].n, 1);
        // every command `run` knows is in COMMANDS (the help and the catalog)
        for c in &COMMANDS[1..] {
            let word = c.0.split_whitespace().next().unwrap();
            let line = format!("{word} x"); // (!defeat needs a name)
            assert!(!matches!(run(&line, 1200, [0.0; 3]), Action::Reply(l) if l[0].starts_with("commands:")), "{word}");
        }
    }

    #[test]
    fn commands_are_recognised() {
        assert!(is_command("!help") && is_command("/goto buildie") && !is_command("hello"));
        assert!(matches!(run("!list nothing", 1200, [0.0; 3]), Action::Reply(l) if l.len() == KINDS.len()));
    }
}
