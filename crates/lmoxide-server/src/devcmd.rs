//! Test commands typed into the normal ("Say") chat: a line starting with `/` or `!` is a
//! command for the server and isn't relayed to other players. Free-text chat needs a GM level
//! (`--gm-level`, set by tools/lan-server.sh). Answers come back as chat lines from "Server".
//!
//!   !help                     the commands
//!   !where                    current playfield and position
//!   !tp <playfield|name>      go to a playfield's arrival point (number or part of its name)
//!   !moveto <x> <y> <z>       go to a position in this playfield
//!   !move <dx> <dy> <dz>      move by that much from where you are (`!move 0 1000 0`: up out of
//!                             a test map's floor)
//!   !list <kind>              numbered list of a kind of test target (see KINDS)
//!   !goto <kind> [n]          go next to target n (default 1) of that kind
//!   !ach                      achievements earned, kill and smash counters
//!   !count <kills|smashes|builds> [n] [name]   count n (default 1) defeats/builds for achievements
//!   !defeat <boss name>       count a boss defeat (boss achievements, the active minifigure's)
//!   !lc <level>               complete Lost Creations level <level> with the active minifigure (stub)
//!   !cleared                  count this playfield as cleared (pocket adventure achievements)
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
//!   !fx <id> [hook] [@near|@ground]  play effect package <id> on yourself (optionally from a
//!                             code hook, e.g. onhit; @near: on the nearest enemy or smashable as
//!                             its target, @ground: at your feet); !fx next|prev steps through
//!                             every package (data/effect_catalog.json), !fx stop ends the current one
//!   !anim <attribute>         play an animation attribute (limbostart, chainattack, ...: the
//!                             package data/animations.json picks for it)
//!   !skin <monsterdata|reset> look like any monster data (others see it too), reset: your minifigure
//!   !state <n>                set your BCCState (stat 20: 1 normal, 29 Limbo/smashed, 104 building, ...)
//!   !spawn [count] <name>     spawn count (default 1, at most 30) of an enemy or smashable around you
//!                             for 15 minutes, as !test does
//!   !despawn [all]            remove the test spawns (!spawn, !test) within 30 m, or all of this playfield's
//!   !script [list]            this playfield's approved, active scripts (world editor) and their waves
//!   !script start <script> [wave]  start a script (name, part of it, id or number), at a wave
//!                             (name, id or number; default its first), as a trigger wall does
//!   !script stop [script]     end this playfield's running scripts (or that one); their spawns go
//!   !bounce [bounce|spin|<kb> <launched> <dir>]  throw yourself 6 m as a bouncer / cannon does
//!   !aim [shots|lobs] [current|lead|homing]  how players' straight and lobbed shots are aimed
//!                             at their enemy (projectile.rs), for everyone; without a mode: the
//!                             current ones (start values: --shot-aim, --lob-aim)
//!
//! `COMMANDS` lists them for `!help` and for the control interface's catalog (`catalog`).
//!
//! New features add their test targets to `targets` so they can be reached the same way.

use lmoxide_net::types::Id32;

use crate::teleport;

/// What the game connection should do for a command.
pub enum Action {
    Reply(Vec<String>),
    /// teleport to (playfield, position, facing; NaN: as the player faces now), with a reply
    Teleport(u32, [f32; 3], f32, String),
    /// show the achievement summary
    Achievements,
    /// show the quests (true: clear their progress and cooldowns first)
    Quests(bool),
    /// Take achievements away (`None`: all of them).
    RemoveAchievements(Option<Vec<u32>>),
    /// smash the nearest smashable (or defeat the nearest enemy) as if the player hit it
    Smash,
    /// bring every minifigure of the player (team and collection) back from smashed, full health
    Revive,
    /// team up with the online character of this name, without the invite (`!team`)
    Team(String),
    /// go to the server next to this one (`!server`, switch.rs); empty: tell which this is
    Server(String),
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
    /// remove test spawns: within 30 m (false) or on the whole playfield (true)
    Despawn(bool),
    /// open a teleport window by id, as a portal would (`!window`)
    Window(u32),
    /// throw the player 6 m ahead (`!bounce`)
    Bounce(crate::jumppad::Tumble),
}

/// Every command with its usage and what it does (`!help`, and tools through `catalog`).
/// (`!help` first: anything unknown answers with the list.)
pub const COMMANDS: &[(&str, &str)] = &[
    ("!help", "the commands"),
    ("!where", "current playfield and position"),
    ("!instance [salt]", "instance spike: re-enter this world under another playfield key (for everyone in this world)"),
    ("!tp <playfield|name>", "go to a playfield's arrival point (number or part of its name)"),
    ("!moveto <x> <y> <z>", "go to a position in this playfield"),
    ("!move <dx> <dy> <dz>", "move by that much from where you are (y is up: !move 0 1000 0 climbs out of a floor)"),
    ("!list <kind>", "numbered list of a kind of test target"),
    ("!goto <kind> [n]", "go next to target n (default 1) of that kind"),
    ("!goto player <name>", "go next to a player online (character or account name)"),
    ("!test [n|label]", "list the manual test setups, or set one up (data/test_scenarios.json)"),
    ("!spawn [count] <name>", "spawn count (default 1, at most 30) of an enemy or smashable around you for 15 minutes"),
    ("!despawn [all]", "remove the test spawns (!spawn, !test) within 30 m, or all of this playfield's"),
    ("!loot <kind>", "drop a loot bag at your feet"),
    ("!smash", "smash the nearest smashable (or defeat the nearest enemy) within 15 m"),
    ("!smashme", "smash the active minifigure (it rebuilds after the smash timer)"),
    ("!server [play|editor]", "which server this is; go to the play server or the world editor's (same account, the character there; answer Yes to \"relocate\")"),
    ("!revive", "every minifigure (team and collection) back to full health"),
    ("!own [name]", "obtain a minifigure (as from a bag); without a name: how many you own"),
    ("!collection <limit|all>", "own only the team and the members' minifigures, or every one (relog to see it)"),
    ("!aura <0-4>", "the active minifigure's aura tier: none, Disciple, Master, Grandmaster, Superachiever"),
    ("!xp [n]", "gain n XP (with --progression), show level and XP"),
    ("!stars <n>", "gain n stars (minifigure XP for ranking up)"),
    ("!ach", "achievements earned, kill and smash counters"),
    ("!quests [reset]", "area quests started, their progress and cooldowns; reset: clear them (start them again)"),
    ("!unach <id ...>|all", "take achievements away (and the \"all of\" ones above them; all: also the journal and counters), to earn them again; story and tutorial progress stays"),
    ("!count <kills|smashes|builds> [n] [name]", "count n (default 1) defeats or builds for achievements"),
    ("!defeat <boss name>", "count a boss defeat (boss achievements, the active minifigure's)"),
    ("!lc <level>", "complete Lost Creations level <level> with the active minifigure (achievements only; the Lost Creations don't run yet)"),
    ("!cleared", "count this playfield as cleared, as when its last enemy falls (pocket adventure achievements)"),
    ("!dialog [n]", "play recorded NPC dialogue n (without n: list them)"),
    ("!mirrors <solve|reset>", "turn this playfield's Medusa mirrors to the solution or the start"),
    ("!plates <solve|reset>", "light every plate of the Gauntlet's plate room, or none"),
    ("!fx <id> [hook] [@near|@ground]", "play effect package <id> on yourself, from a code hook (onhit, charge, ...), on the nearest enemy as target (@near) or at your feet (@ground)"),
    ("!anim <attribute>", "play an animation attribute (limbostart, chainattack, get_hit, ...)"),
    ("!fx next|prev|stop", "step through every effect package, or end the current one"),
    ("!skin <monster data|reset>", "look like any monster data (others see it too); reset: your minifigure"),
    ("!state <n>", "set your BCCState (1 normal, 29 Limbo/smashed, 104 building, ...)"),
    ("!script [list]", "this playfield's approved, active scripts (world editor) with their waves, triggers and what is running"),
    ("!script start <script> [wave]", "start a script (name, part of it, id or number from !script list) at a wave (name, id or number; default its first), as a trigger wall does"),
    ("!script stop [script]", "end this playfield's running scripts (or that one); their spawns go"),
    ("!window <id>", "open teleport window <id> (RDB 1000644), as a portal would"),
    ("!bounce [bounce|spin|<knockback 0/1> <launched 0/1> <direction>]", "throw yourself 6 m along +x as a bouncer (default) or a cannon/piston does, or with raw animation settings"),
    ("!aim [shots|lobs] [current|lead|homing]", "how players' straight shots and lobbed ones (both without shots|lobs) are aimed at their enemy, for everyone (without a mode: the current ones)"),
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
    /// package, code hook ("" for none), where
    Play(u32, String, FxAt),
    Step(i32),
    Stop,
}

/// Where `!fx` plays a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxAt {
    /// on the player (the record's identity and named target)
    Me,
    /// the player's, with the nearest enemy or smashable as its named target (`@near`)
    Near,
    /// at the player's feet (a ground effect: `@ground`)
    Ground,
}

/// The effect package `!anim <attribute>` plays for each animation attribute
/// (data/animations.json, tools/re/extract_animations.py).
pub fn animation_package(attribute: &str) -> Option<u32> {
    #[derive(serde::Deserialize)]
    struct File {
        attributes: Vec<Attribute>,
    }
    #[derive(serde::Deserialize)]
    struct Attribute {
        name: String,
        package: Option<u32>,
    }
    static PACKAGES: std::sync::LazyLock<std::collections::HashMap<String, u32>> = std::sync::LazyLock::new(|| {
        let f: File = serde_json::from_str(include_str!("../../../data/animations.json")).expect("animations.json");
        f.attributes.into_iter().filter_map(|a| Some((a.name.to_ascii_lowercase(), a.package?))).collect()
    });
    PACKAGES.get(&attribute.to_ascii_lowercase()).copied()
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
    ("dialogue", "the live capture's dialogue speakers (arriving in the jail, or on Isle of Yarr after it, starts its dialogue; !dialog plays any)"),
    ("boss", "dungeon bosses"),
    ("champion", "champions"),
    ("enemy", "all enemies"),
    ("npc", "friendly NPCs"),
    ("fighter", "friendly NPCs fighting enemies (npc_fights.json)"),
    ("patrol", "NPCs that walk a patrol route (the live capture's: Musketeers, Jungle Tricksters, Vespids, ...); arrives 14 m off, out of their notice"),
    ("smashable", "smashables"),
    ("script", "the world editor's approved, active scripts (at their area, else their first placement)"),
    ("quest", "area quests' areas (walking in starts the quest; quests.rs)"),
];

/// How far from a target to arrive (so as not to stand inside it or on a pad).
const ARRIVE_OFFSET: f32 = 3.0;
/// `!goto patrol`: how far from the patroller's home to arrive, so it keeps walking its route
/// (enemies notice players 8 m away; routes reach a few metres from home).
const PATROL_WATCH: f32 = 14.0;

/// `!script list | start <script> [wave] | stop [script]`. The script's name may have spaces: the
/// whole rest first, else its last word is the wave.
fn script(args: &[&str], playfield: u32) -> Vec<String> {
    match args.first().map(|a| a.to_ascii_lowercase()).as_deref() {
        None | Some("list") => crate::scripts::list(playfield),
        Some("start") if args.len() > 1 => {
            let whole = args[1..].join(" ");
            let r = crate::scripts::start(playfield, &whole, None);
            let r = match r {
                // no script by the whole name: the last word may be the wave
                Err(e) if args.len() > 2 && e.starts_with("no script") => {
                    let (name, wave) = (args[1..args.len() - 1].join(" "), args[args.len() - 1]);
                    crate::scripts::start(playfield, &name, Some(wave)).or(Err(e))
                }
                r => r,
            };
            vec![r.unwrap_or_else(|e| e)]
        }
        Some("stop") => crate::scripts::stop(playfield, (args.len() > 1).then(|| args[1..].join(" ")).as_deref()),
        _ => vec!["usage: !script [list] | start <script> [wave] | stop [script]".into()],
    }
}

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
        "dialogue" => crate::knubot::points(),
        "portal" => teleport::points(),
        "arena" | "ambush" | "explore" => crate::encounter::points(kind),
        "boss" | "champion" | "enemy" | "npc" | "fighter" | "patrol" | "smashable" => crate::npc::points(kind),
        "script" => crate::scripts::points(),
        "quest" => crate::quests::points(),
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
        "moveto" | "move" => {
            let n: Vec<f32> = args.iter().filter_map(|a| a.trim_end_matches(',').parse::<f32>().ok()).filter(|v| v.is_finite()).collect();
            if n.len() != 3 || args.len() != 3 {
                return reply(vec![format!("usage: !{cmd} <{0}x> <{0}y> <{0}z> (y is up)", if cmd == "move" { "d" } else { "" })]);
            }
            let to = if cmd == "move" { [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]] } else { [n[0], n[1], n[2]] };
            Action::Teleport(playfield, to, f32::NAN, format!("going to {:.1}, {:.1}, {:.1}", to[0], to[1], to[2]))
        }
        "tp" => {
            let query = args.join(" ");
            if query.is_empty() {
                return reply(vec!["usage: !tp <playfield number or name>".into()]);
            }
            let found = query.parse::<u32>().ok().filter(|pf| teleport::destination(*pf).is_some()).or_else(|| {
                let q = query.to_ascii_lowercase();
                // an exact name first, else the lowest playfield whose name contains it
                teleport::destinations()
                    .filter(|(_, d)| d.name.to_ascii_lowercase().contains(&q))
                    .min_by_key(|(pf, d)| (d.name.to_ascii_lowercase() != q, *pf))
                    .map(|(pf, _)| pf)
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
        "goto" if args.first().is_some_and(|k| k.eq_ignore_ascii_case("player")) => {
            let name = args[1..].join(" ");
            let players = crate::sessions::players();
            let found = players.iter().find(|p| p.character_name.eq_ignore_ascii_case(&name) || p.username.eq_ignore_ascii_case(&name));
            let at = found.and_then(|p| crate::world::whereabouts(lmoxide_net::types::Id32::new(crate::territory::CHARACTER_TYPE, p.character_id)));
            match (found, at) {
                (Some(p), Some((pf, pos))) => {
                    let to = [pos[0] + ARRIVE_OFFSET, pos[1], pos[2]];
                    Action::Teleport(pf, to, (-ARRIVE_OFFSET).atan2(0.0), format!("going to {} in {}", p.character_name, playfield_name(pf)))
                }
                (Some(p), None) => reply(vec![format!("{} is online but not in a world yet", p.character_name)]),
                (None, _) if name.is_empty() => reply(vec!["usage: !goto player <name>".into()]),
                (None, _) => reply(vec![format!("nobody named {name} is online")]),
            }
        }
        "goto" => {
            let Some((kind, t)) = args.first().and_then(|k| targets(k).map(|t| (k, t))) else {
                return reply(kinds_help());
            };
            let n = args.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
            match t.get(n.wrapping_sub(1)) {
                Some((label, pf, p)) => {
                    let off = if *kind == "patrol" { PATROL_WATCH } else { ARRIVE_OFFSET };
                    let at = [p[0] + off, p[1], p[2]];
                    // face the target (yaw convention as in return points: atan2(dx, dz))
                    let rot = (-off).atan2(0.0);
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
        "quests" | "quest" => match args.first().copied() {
            None => Action::Quests(false),
            Some("reset") => Action::Quests(true),
            _ => reply(vec!["usage: !quests [reset]".into()]),
        },
        "unach" => match args.first().copied() {
            Some("all") => Action::RemoveAchievements(None),
            _ => match args.iter().map(|a| a.trim_matches(|c| c == '[' || c == ']').parse::<u32>()).collect::<Result<Vec<_>, _>>() {
                Ok(ids) if !ids.is_empty() => Action::RemoveAchievements(Some(ids)),
                _ => Action::Reply(vec!["!unach <achievement id ...> | all (the ids are in the web UI and the world editor: [1613])".into()]),
            },
        },
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
        "team" if !args.is_empty() => Action::Team(args.join(" ")),
        "team" => reply(vec!["usage: !team <character name> (online)".into()]),
        "server" => Action::Server(args.join(" ")),
        // instance spike: re-enter this world under another client key (for everyone in it)
        "instance" => match args.first().map(|a| a.parse::<u32>()) {
            None => reply(vec![format!("instance salt {} (key {:#x}); usage: !instance <0-127>", teleport::instance_salt(playfield), teleport::client_key(playfield))]),
            Some(Ok(salt)) if salt < 128 => {
                teleport::set_instance_salt(playfield, salt);
                Action::Teleport(playfield, pos, f32::NAN, format!("re-entering {} as key {:#x}", playfield_name(playfield), teleport::client_key(playfield)))
            }
            Some(_) => reply(vec!["usage: !instance <0-127>".into()]),
        },
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
            Some(n) if n.parse::<u32>().is_ok() => {
                let mut at = FxAt::Me;
                let mut hook = "";
                for a in &args[1..] {
                    match a.to_ascii_lowercase().as_str() {
                        "@near" | "@target" => at = FxAt::Near,
                        "@ground" => at = FxAt::Ground,
                        w if w.starts_with('@') => return reply(vec![format!("{a}? where: @near or @ground")]),
                        _ => hook = a,
                    }
                }
                Action::Fx(Fx::Play(n.parse().unwrap(), hook.to_string(), at))
            }
            _ => reply(vec!["usage: !fx <id> [code hook] [@near|@ground] | next | prev | stop".into()]),
        },
        "anim" => match args.first() {
            Some(a) => match animation_package(a) {
                Some(p) => Action::Fx(Fx::Play(p, String::new(), FxAt::Me)),
                None => reply(vec![format!("no effect package sets the animation attribute {a}")]),
            },
            None => reply(vec!["usage: !anim <animation attribute> (limbostart, chainattack, get_hit, ...)".into()]),
        },
        "despawn" => Action::Despawn(args.first().is_some_and(|a| a.eq_ignore_ascii_case("all"))),
        "state" => match args.first().and_then(|n| n.parse().ok()) {
            Some(n) => Action::State(n),
            None => reply(vec!["usage: !state <BCCState number> (1 normal, 29 Limbo)".into()]),
        },
        "bounce" => {
            use crate::jumppad::Tumble;
            let n = |i: usize| args.get(i).and_then(|a| a.parse::<u8>().ok()).unwrap_or(0);
            match args.first().copied() {
                None | Some("bounce") => Action::Bounce(Tumble::BOUNCE),
                Some("spin") => Action::Bounce(Tumble::SPIN),
                Some(_) => Action::Bounce(Tumble { knockback: n(0) != 0, launched: n(1) != 0, direction: n(2) }),
            }
        }
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
        "script" | "scripts" => reply(script(&args, playfield)),
        "plates" => match args.first().copied() {
            Some(how @ ("solve" | "reset")) => {
                crate::plates::set_all(how == "solve");
                reply(vec![format!("plates: {how}")])
            }
            _ => reply(vec!["usage: !plates <solve|reset>".into()]),
        },
        "aim" => {
            use crate::projectile::{Aim, Shots, player_aim, set_player_aim};
            let modes = Aim::ALL.map(Aim::name).join("|");
            let now = || format!("players aim: shots {}, lobs {}", player_aim(Shots::Straight).name(), player_aim(Shots::Lobbed).name());
            let (which, mode) = match args.as_slice() {
                [] => (None, None),
                ["shots", mode] => (Some(vec![Shots::Straight]), Some(*mode)),
                ["lobs", mode] => (Some(vec![Shots::Lobbed]), Some(*mode)),
                [mode] => (Some(vec![Shots::Straight, Shots::Lobbed]), Some(*mode)),
                _ => (Some(vec![]), None),
            };
            match (which, mode.map(|m| Aim::ALL.into_iter().find(|a| a.name() == m))) {
                (None, _) => reply(vec![format!("{} (!aim [shots|lobs] <{modes}>)", now())]),
                (Some(which), Some(Some(aim))) if !which.is_empty() => {
                    if which.into_iter().all(|shots| set_player_aim(shots, aim)) {
                        reply(vec![now()])
                    } else {
                        reply(vec![format!("aim {} isn't implemented yet; {}", aim.name(), now())])
                    }
                }
                _ => reply(vec![format!("usage: !aim [shots|lobs] <{modes}>")]),
            }
        }
        "mirrors" => match args.first().copied() {
            Some(how @ ("solve" | "reset")) => Action::Mirrors(how == "solve"),
            _ => reply(vec!["usage: !mirrors <solve|reset>".into()]),
        },
        "lc" => match args.first().and_then(|n| n.parse::<u32>().ok()) {
            Some(level) => Action::Progress(crate::achievements::Event::LostCreations { level }, 1),
            None => reply(vec!["usage: !lc <level>".into()]),
        },
        "cleared" => Action::Progress(crate::achievements::Event::Cleared { playfield }, 1),
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
        assert!(matches!(run("!moveto 1 2 3", 1200, [9.0; 3]), Action::Teleport(1200, [1.0, 2.0, 3.0], r, _) if r.is_nan()));
        assert!(matches!(run("!move 0 1000 -1", 1200, [5.0, -900.0, 5.0]), Action::Teleport(1200, [5.0, 100.0, 4.0], ..)));
        assert!(matches!(run("!move 0 1000", 1200, [0.0; 3]), Action::Reply(_)));
    }

    #[test]
    fn goto_the_live_dialogue_speakers() {
        // in capture order: the LEGO Center, the Monster Laboratory (2), the jail (2), ...
        assert!(matches!(run("!goto dialogue 1", 4200, [0.0; 3]), Action::Teleport(1200, ..)));
        assert!(matches!(run("!goto dialogue 4", 4200, [0.0; 3]), Action::Teleport(4610, ..)));
        assert!(matches!(run("!goto dialogue 6", 4610, [0.0; 3]), Action::Teleport(4200, ..)));
    }

    #[test]
    fn fx_takes_a_hook_and_a_place() {
        assert!(matches!(run("!fx 966", 1200, [0.0; 3]), Action::Fx(Fx::Play(966, h, FxAt::Me)) if h.is_empty()));
        assert!(matches!(run("!fx 966 onhit @near", 1200, [0.0; 3]), Action::Fx(Fx::Play(966, h, FxAt::Near)) if h == "onhit"));
        assert!(matches!(run("!fx 966 @ground", 1200, [0.0; 3]), Action::Fx(Fx::Play(966, h, FxAt::Ground)) if h.is_empty()));
        assert!(matches!(run("!fx 966 @moon", 1200, [0.0; 3]), Action::Reply(_)));
    }

    #[test]
    fn anim_plays_an_attribute_package() {
        assert!(matches!(run("!anim limbostart", 1200, [0.0; 3]), Action::Fx(Fx::Play(_, _, FxAt::Me))));
        assert!(matches!(run("!anim nothing_like_it", 1200, [0.0; 3]), Action::Reply(_)));
        assert!(matches!(run("!despawn all", 1200, [0.0; 3]), Action::Despawn(true)));
        assert!(matches!(run("!despawn", 1200, [0.0; 3]), Action::Despawn(false)));
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
