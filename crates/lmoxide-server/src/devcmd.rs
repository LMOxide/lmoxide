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
//!   !areas [radius]           content areas and walls near you (role, category, triggers, inside?)
//!   !wall <open|close> <name> switch this playfield's walls of that name off or on
//!   !list <kind>              numbered list of a kind of test target (see KINDS)
//!   !goto <kind> [n]          go next to target n (default 1) of that kind
//!   !ach                      achievements earned, kill and smash counters
//!   !count <kills|smashes|builds> [n] [name]   count n (default 1) defeats/builds for achievements
//!   !defeat <boss name>       count a boss defeat (boss achievements, the active minifigure's)
//!   !lc                       the Lost Creations run you're in (level, map, phase, time left)
//!   !lc start [level] [map]   a run alone from level [level] (default 1) on [map] (a level id or
//!                             part of its name; default: as a run picks)
//!   !lc objective|finish      the level's objective done / across the end at once
//!   !lc time <s>              the timer set to <s> seconds
//!   !lc end                   out of the run (to the Treasure Chamber)
//!   !lc variant [id]          the next level copy runs that variant (if its level has it);
//!                             without an id: every level's variants
//!   !point <name>             save a named point (marker) here, facing as you do, in this
//!                             world's content (areas.json; tagged `review`)
//!   !goto point <name|n>      go onto a named point (`!goto point nw-1`), in your copy
//!   !lc <level>               complete Lost Creations level <level> with the active minifigure
//!                             (the achievements only)
//!   !cleared                  count this playfield as cleared (pocket adventure achievements)
//!   !xp [n]                   gain n XP (with --progression), show level and XP
//!   !stars <n>                gain n stars (minifigure XP for ranking up)
//!   !test [n|label]           list the manual test setups, or set one up (data/test_scenarios.json)
//!   !aura <0-4>               the active minifigure's aura tier (gives/takes its aura tags)
//!   !dialog [n|text]          play NPC dialogue n, or the one whose first text is `text` (the
//!                             rebuilt story conversations too; without n: list them)
//!   !cinematic [n|name|id]    play a narrated story cinematic (without an argument: list them)
//!   !story [reset]            the story (cinematics seen, story lines, goals); reset forgets it
//!   !story quest <line> <step>  put a story line (pirate, medieval, space, mythology, dino or
//!                             its main quest) at a step (1 = first; 0 = not started)
//!   !story advance [line]     complete the current step of a story line (default: the first)
//!   !story goal|beat|dialogue|seen <arg>   resolve a goal, run a beat, start a dialogue, or play
//!                             a cinematic as its trigger would
//!   !story progress <hash> [n]  count n toward a counted story goal (the Dino pieces: GBWD)
//!   !mirrors <solve|reset>    turn this playfield's Medusa mirrors to the solution or the start
//!   !keygate <open|reset>     open the Underworld's front gate as its pedestal does, or put it
//!                             (and the collapsing floor) back
//!   !plates <solve|reset>     light every plate of the Gauntlet's plate room, or none
//!   !revive                   every minifigure (team and collection) back to full health
//!   !smashme                  smash the active minifigure (it rebuilds after the smash timer)
//!   !loot <kind>              drop a loot bag at your feet: potion, part, bag, chest, memberchest,
//!                             boss (a boss's roll: free and member chest), enemy, smashable, or
//!                             any content loot table (`!loot loot/achievement-stars-small`);
//!                             rolled at the chest power here, or `!loot <kind> <power>`
//!   !power [table]            the loot power of chests, quest rewards and a level-N enemy here;
//!                             with a table or kind, five rolls of it at the chest power
//!   !stats                    the active minifigure's upgrade procs, zone element buffs, damage,
//!                             defence and build factors, smash timer, level cap, loot power
//!   !hurt <percent>           set the active minifigure's health to that share (low-health procs)
//!   !element <0-5|name>       play as if this zone had that element (Wrong Element / Elemental
//!                             Affinity; 0 none, 1 light ... 5 nature)
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
//!   !speed [factor]           your movement speed (1 normal, 0.5 half, 2 double; 0.1 to 10),
//!                             kept with the character; without a factor: the current one
//!   !bounce [bounce|spin|<kb> <launched> <dir>]  throw yourself 6 m as a bouncer / cannon does
//!   !aim [shots|lobs] [current|lead|homing]  how players' straight and lobbed shots are aimed
//!                             at their enemy (projectile.rs), for everyone; without a mode: the
//!                             current ones (start values: --shot-aim, --lob-aim)
//!
//! Every player (GM or not) has `/story reset [confirm]` (`player_command`): forget one's own
//! story, after a confirmation.
//!
//! `COMMANDS` lists them for `!help` and for the control interface's catalog (`catalog`).
//!
//! New features add their test targets to `targets` so they can be reached the same way.

use crate::instance::Pf;
use lmoxide_net::types::Id32;

use crate::teleport;

/// What the game connection should do for a command.
pub enum Action {
    Reply(Vec<String>),
    /// teleport to (playfield, position, facing; NaN: as the player faces now), with a reply
    Teleport(u32, [f32; 3], f32, String),
    /// as `Teleport`, into one running copy of a world (`!instance`)
    Enter(Pf, [f32; 3], f32, String),
    /// play this world as the world editor's working copy has it, in a copy of one's own
    /// (`!preview`; preview.rs); `false`: back from the preview
    Preview(bool),
    /// `!mode <mode>`: play the account's character of another game mode (territory::switch_mode)
    SwitchMode(crate::instance::Mode),

    /// show the achievement summary
    Achievements,
    /// show the quests (true: clear their progress and cooldowns first)
    Quests(bool),
    /// Take achievements away (`None`: all of them).
    RemoveAchievements(Option<Vec<u32>>),
    /// rename a character (`None`: one's own) (`!rename`; control::rename)
    Rename(Option<u32>, String),
    /// smash the nearest smashable (or defeat the nearest enemy) as if the player hit it
    Smash,
    /// bring every minifigure of the player (team and collection) back from smashed, full health
    Revive,
    /// team up with the online character of this name, without the invite (`!team`)
    Team(String),
    /// feed achievement events (n times)
    Progress(crate::achievements::Event, u32),
    /// `!grant`: earn these achievements (lore ids) as if their rules held, rewards too
    Grant(Vec<u32>),
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
    /// open the Underworld's front gate (true) or put it back
    Keygate(bool),
    /// smash the active minifigure
    SmashMe,
    /// drop a loot bag of a kind (`!loot`), at this loot power (none: the chest power here)
    Loot(String, Option<u32>),
    /// the loot power of each source here, and a table rolled at it (`!power`)
    Power(Option<String>),
    /// the active minifigure's stats, procs and zone buffs (`!stats`)
    Stats,
    /// the active minifigure's health to this percent of its maximum (`!hurt`)
    /// Percent of the maximum; `true`: every equipped minifigure, not just the active one.
    Hurt(u32, bool),
    /// the zone's element for the player (`!element`)
    Element(u32),
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
    /// Show a content buff on yourself (`buff/...`), for this long or the buff's own time.
    Buff(String, Option<std::time::Duration>),
    /// open a teleport window by id, as a portal would (`!window`)
    Window(u32),
    /// throw the player 6 m ahead (`!bounce`)
    Bounce(crate::jumppad::Tumble),
    /// the Lost Creations (`!lc`)
    Lc(crate::lost_creations::Command),
    /// save a named point here, facing as the player does (`!point`)
    Point(String),
    /// the story: show it (`None`; true: clear it first), or do an order (`!cinematic`, `!story`)
    Story(Option<crate::story::Order>, bool),
    /// the character's movement speed factor (`!speed`; `None`: show it)
    Speed(Option<f32>),
}

/// Every command with its usage and what it does (`!help`, and tools through `catalog`).
/// (`!help` first: anything unknown answers with the list.)
pub const COMMANDS: &[(&str, &str)] = &[
    ("!help", "the commands"),
    ("!where", "current playfield and position"),
    ("!mode <progression|maxed|editor>", "play your character of another game mode: the game asks to relocate to a new server, Yes takes you there (no restart, no patcher)"),
    ("!preview [end]", "play this world as the world editor's working copy has it (unpublished changes too), enemies fighting, in a copy of your own; `!preview end` goes back"),
    ("!instance [copy]", "which running copy of this world you are in; go to another copy (0 = the main one; other copies are empty for now)"),
    ("!tp <playfield|name>", "go to a playfield's arrival point (number or part of its name)"),
    ("!moveto <x> <y> <z>", "go to a position in this playfield"),
    ("!move <dx> <dy> <dz>", "move by that much from where you are (y is up: !move 0 1000 0 climbs out of a floor)"),
    ("!areas [radius]", "the content areas within radius m (default 20) of you: role, category, triggers on them, and whether you stand inside; walls: whether they block"),
    ("!wall <open|close> <name>", "open (switch off) or close the walls of this playfield whose name or id contains <name> (in this copy, until it empties)"),
    ("!list <kind>", "numbered list of a kind of test target"),
    ("!goto <kind> [n]", "go next to target n (default 1) of that kind"),
    ("!goto player <name>", "go next to a player online (character or account name)"),
    ("!rename [<who> =] <new name>", "admins: rename your character, or someone's (who: a character name or id, or the account name of a player online); online characters are sent again where they stand so everyone sees it"),
    ("!test [n|label]", "list the manual test setups, or set one up (data/test_scenarios.json)"),
    ("!spawn [count] <name>", "spawn count (default 1, at most 30) of an enemy or smashable around you for 15 minutes"),
    ("!despawn [all]", "remove the test spawns (!spawn, !test) within 30 m, or all of this playfield's"),
    ("!loot <kind> [power]", "drop a loot bag at your feet (rolled at the chest power here, or this one)"),
    ("!power [table|kind]", "the loot power of chests, quest rewards and enemies here; with a table, five rolls of it at the chest power"),
    ("!smash", "smash the nearest smashable (or defeat the nearest enemy) within 15 m"),
    ("!smashme", "smash the active minifigure (it rebuilds after the smash timer)"),
    ("!stats", "the active minifigure's upgrade procs, zone element buffs (Wrong Element, Elemental Affinity), damage/defence/build factors, smash timer, level cap and loot power"),
    ("!hurt <percent> [all]", "set the active minifigure's (all: every equipped one's) health to that share of its maximum (Second Wind, True Grit, Escape Artist below 21%; a potion at 5% or less: Close Call!)"),
    ("!element <0-5|name>", "play as if this zone had that element: 0 none, 1 light, 2 shadow, 3 tech, 4 magic, 5 nature (Wrong Element / Elemental Affinity)"),
    ("!revive", "every minifigure (team and collection) back to full health"),
    ("!own [name]", "obtain a minifigure (as from a bag); without a name: how many you own"),
    ("!collection <limit|all>", "own only the team and the members' minifigures, or every one (relog to see it)"),
    ("!aura <0-4>", "the active minifigure's aura tier: none, Disciple, Master, Grandmaster, Superachiever"),
    ("!xp [n]", "gain n XP (with --progression), show level and XP"),
    ("!stars <n>", "gain n stars (minifigure XP for ranking up)"),
    ("!ach", "achievements earned, kill and smash counters"),
    ("!quests [reset]", "area quests started, their progress and cooldowns; reset: clear them (start them again)"),
    ("!grant <id ...>", "earn achievements now (their rewards too, and the \"all of\" ones they complete); !unach takes them away"),
    ("!unach <id ...>|all", "take achievements away (and the \"all of\" ones above them; all: also the journal and counters), to earn them again; story and tutorial progress stays"),
    ("!count <kills|smashes|builds> [n] [name]", "count n (default 1) defeats or builds for achievements"),
    ("!defeat <boss name>", "count a boss defeat (boss achievements, the active minifigure's)"),
    ("!lc", "the Lost Creations run you're in: level, map, phase, time left"),
    ("!lc start [level] [map]", "a Lost Creations run alone, from level [level] (default 1) on [map] (a level id or part of its name; default: as a run picks)"),
    ("!lc objective|finish", "the level's objective done, or across the end at once"),
    ("!lc time <s>", "the level's timer set to <s> seconds"),
    ("!lc end", "out of the Lost Creations run (to the Treasure Chamber)"),
    ("!lc variant [id]", "the next Lost Creations level copy runs variant <id> (if its level has one by that id); without an id: every level's variants"),
    ("!point <name>", "save a named point (a marker) where you stand, facing as you do, in this world's content (areas.json, tagged review); spawns stand on it with `at`"),
    ("!goto point <name|n>", "go onto a named point (marker) by name or slug (`!goto point nw-1`; the one in your world first), in the copy you're in"),
    ("!lc <level>", "complete Lost Creations level <level> with the active minifigure (the achievements only)"),
    ("!cleared", "count this playfield as cleared, as when its last enemy falls (pocket adventure achievements)"),
    ("!dialog [n|text]", "play NPC dialogue n or the one starting with dialogue text `text` (e.g. 283 the Jester; rebuilt story conversations too; without n: list them)"),
    ("!cinematic [n|name|package]", "play a narrated story cinematic (again; without an argument: list them)"),
    ("!story [reset]", "the story: cinematics seen, story lines started, goals resolved; reset: forget them (the lore flags stay)"),
    ("!story quest <line> <step>", "put a story line (pirate, medieval, space, mythology, dino or its main quest id) at step <step> (1: its first; 0: not started; past the last: done) and show it in the tracker"),
    ("!story advance [line]", "complete the current step of a story line (default: the first one running), as its goal resolving would"),
    ("!story progress <hash> [n]", "count n (default 1) toward a counted story goal the player is on, as a reward script does (the Dino pieces: !story progress GBWD)"),
    ("!story goal <hash>|beat <text>|dialogue <id>|seen <cinematic>", "resolve a story goal, run a story beat (a dialogue text's server side), start a dialogue (unless its tags are had), or play a cinematic as its trigger would (once)"),
    ("!mirrors <solve|reset>", "turn this playfield's Medusa mirrors to the solution or the start"),
    ("!keygate <open|reset>", "open the Underworld's front gate as its pedestal does (crystal, light, gate), or put it and the collapsing floor back"),
    ("!plates <solve|reset>", "light every plate of the Gauntlet's plate room, or none"),
    ("!fx <id> [hook] [@near|@ground]", "play effect package <id> on yourself, from a code hook (onhit, charge, ...), on the nearest enemy as target (@near) or at your feet (@ground)"),
    ("!speed [factor]", "your movement speed: 1 normal, 0.5 half, 2 double (0.1 to 10; kept with the character across logins); without a factor: the current one"),
    ("!buff <buff> [secs]", "put a content buff on yourself as a trigger's apply_buff does (`!buff speed-pad 60`: the speed pad's buff and its trail)"),
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

/// The commands alpha testers have (`--tester-gm-level`; the others need `--devcmd-gm-level`):
/// teleporting, test scenarios, minifigures, dialogues, achievements, animations, looks and
/// states, and the dev UI's Quick section.
pub const TESTER_COMMANDS: &[&str] = &[
    "help", "where", "tp", "moveto", "move", "list", "goto", "test", "own", "dialog", "ach", "count", "defeat", "anim", "skin",
    "state", "revive", "smash", "smashme", "xp", "stars", "mirrors", "plates",
];

/// The commands only admins have (`--admin-gm-level`).
pub const ADMIN_COMMANDS: &[&str] = &["rename"];

/// The GM level a command line needs.
pub fn gm_level_for(message: &str) -> u32 {
    let levels = crate::state::levels();
    let cmd = message.get(1..).unwrap_or("").split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if ADMIN_COMMANDS.contains(&cmd.as_str()) {
        levels.admin
    } else if TESTER_COMMANDS.contains(&cmd.as_str()) {
        levels.tester
    } else {
        levels.devcmd
    }
}

/// `!rename`'s `<who>`: `me`, a character id, a player online (character or account name) or a
/// saved character's name.
fn rename_target(who: &str) -> Result<Option<u32>, String> {
    if who.eq_ignore_ascii_case("me") {
        return Ok(None);
    }
    if let Ok(id) = who.parse::<u32>() {
        return Ok(Some(id));
    }
    let players = crate::sessions::players();
    if let Some(p) = players.iter().find(|p| p.character_name.eq_ignore_ascii_case(who)).or_else(|| players.iter().find(|p| p.username.eq_ignore_ascii_case(who))) {
        return Ok(Some(p.character_id));
    }
    let saved = crate::state::or_log(crate::state::db().search_characters(who, 50), "characters", Vec::new());
    let exact: Vec<_> = saved.iter().filter(|(c, _)| c.name.eq_ignore_ascii_case(who)).collect();
    match exact[..] {
        [] => Err(format!("no character named {who}")),
        [(c, _)] => Ok(Some(c.id)),
        _ => Err(format!(
            "{who} names several characters (one per game mode): use an id, {}",
            exact.iter().map(|(c, _)| format!("{} ({})", c.id, c.mode)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Most a `!spawn` brings at once.
const MAX_SPAWN: u32 = 30;

/// What the commands accept, for the control interface (`GET /v1/test-commands`).
pub fn catalog() -> lmoxide_control::TestCatalog {
    let (spawnable, looks) = crate::npc::test_catalog();
    lmoxide_control::TestCatalog {
        commands: COMMANDS
            .iter()
            .map(|(usage, about)| lmoxide_control::TestCommand { usage: usage.to_string(), about: about.to_string(), gm_level: gm_level_for(usage) })
            .collect(),
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
    /// enemies to spawn around the player: (name, count, the only spells they cast: none = all)
    pub spawn: Vec<(String, u32, Vec<u32>)>,
    /// level-ups to gain (XP up to the level that many above; with progression)
    pub levels: u32,
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
    #[serde(default)]
    levels: u32,
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
    /// cast only these of their spells (e.g. Captain Dreadleg's bomb)
    #[serde(default)]
    spells: Vec<u32>,
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
        lines.push("homestone: give the Homestone (the jail escape's), then cast it from its shortcut".into());
        return Action::Reply(lines);
    };
    if arg.eq_ignore_ascii_case("homestone") {
        // the jail escape's `spawn SLXS` (text 823): the item and the shortcut; cast it to go to
        // the LEGO Center (refused in a Lost Creations level)
        return Action::Story(Some(crate::story::Order::Spawn(crate::homestone::hash().to_string())), false);
    }
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
    let spawn = s.spawn.iter().map(|sp| (sp.name.clone(), sp.count.min(30), sp.spells.clone())).collect();
    Action::Test(TestSetup { label: s.label.clone(), destination, team, spawn, levels: s.levels.min(crate::progression::MAX_PLAYER_LEVEL) })
}

/// A command's target kinds (for `!list` / `!goto`) and what they are.
const KINDS: &[(&str, &str)] = &[
    ("buildie", "buildables (buildie.rs)"),
    ("pad", "jump pads"),
    ("lever", "levers (scenery)"),
    ("plate", "pressure plates (scenery)"),
    ("gate", "gates and portcullis (scenery)"),
    ("mirror", "Medusa mirrors (scenery)"),
    ("pedestal", "the Underworld front gate's pedestal (use it: the crystal opens the gate; !keygate)"),
    ("collectible", "collectibles (achievements)"),
    ("boombox", "boomboxes (achievements)"),
    ("arena", "boss arena starts (encounters)"),
    ("ambush", "ambush areas (encounters)"),
    ("explore", "exploration achievement areas"),
    ("portal", "portals (walk 8 m away after arriving, then step in)"),
    ("lc", "the Lost Creations door (Dinosaur Rise), each level's start, the Treasure Chamber's reward chest"),
    ("trigger", "tutorial triggers (1300; listed 6 m off, so !goto and the tour land inside)"),
    ("dialogue", "the live capture's dialogue speakers (arriving in the jail, or on Isle of Yarr after it, starts its dialogue; !dialog plays any)"),
    ("rebuilt", "where the rebuilt story conversations start (their area, or the boss they follow; tools/content/story_dialogues.py)"),
    ("boss", "dungeon bosses"),
    ("champion", "champions"),
    ("enemy", "all enemies"),
    ("npc", "friendly NPCs"),
    ("fighter", "friendly NPCs fighting enemies (npc_fights.json)"),
    ("patrol", "NPCs that walk a patrol route (the live capture's: Musketeers, Jungle Tricksters, Vespids, ...); arrives 14 m off, out of their notice"),
    ("smashable", "smashables"),
    ("script", "the world editor's approved, active scripts (at their area, else their first placement)"),
    ("point", "named points (markers): enemy spawn points (Jungle Arena's NW_1 ...), the Gatekeeper's spot, `!point` ones"),
    ("quest", "area quests' areas (walking in starts the quest; quests.rs), and the areas that complete a goal (\"(completes: ...)\": the Tar Pit nest, the Gauntlet room exits)"),
    ("story", "story trigger areas: cinematics, story beats, talking heads (tags), story goals (story.rs)"),
    ("area", "server trigger, story and territory areas (RDB 1000053 / 1000050; triggers.rs), with role and category"),
    ("wall", "invisible walls (client RDB 1040010 and server 1000053 collision; enemies' paths stop at them, server ones push players back)"),
    ("fallzone", "falling-out-of-the-world volumes: arrives above one (under the world), falls in, comes back at the nearest respawn point"),
    ("speedpad", "speed pads (BA_Speedbuff): build the sign there, walk onto it for the speed buff"),
];

/// How far from a target to arrive (so as not to stand inside it or on a pad).
const ARRIVE_OFFSET: f32 = 3.0;
/// `!goto patrol`: how far from the patroller's home to arrive, so it keeps walking its route
/// (enemies notice players 8 m away; routes reach a few metres from home).
const PATROL_WATCH: f32 = 14.0;

/// `!script list | start <script> [wave] | stop [script]`. The script's name may have spaces: the
/// whole rest first, else its last word is the wave.
fn script(args: &[&str], playfield: Pf) -> Vec<String> {
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

/// `!areas`: the content areas and walls within `radius` m of `pos`.
fn areas_near(playfield: Pf, pos: [f32; 3], radius: f32) -> Vec<String> {
    let near = |c: [f32; 3]| ((c[0] - pos[0]).powi(2) + (c[2] - pos[2]).powi(2)).sqrt();
    let mut lines: Vec<(f32, String)> = crate::triggers::areas_of(playfield.model())
        .into_iter()
        .filter(|a| near(a.centre()) <= radius && a.role != lmoxide_content::AreaRole::Wall)
        .map(|a| {
            let triggers = crate::triggers::triggers_on(&a.id);
            let inside = if a.shape.contains(pos, 1.0) { " (you're inside)" } else { "" };
            let what = format!("{:?}{}", a.role, a.category.as_deref().map(|c| format!(" {c}")).unwrap_or_default());
            let t = if triggers.is_empty() { String::new() } else { format!(", triggers: {}", triggers.join(" ")) };
            (near(a.centre()), format!("{:.0} m {} [{what}]{t}{inside}", near(a.centre()), a.name))
        })
        .collect();
    lines.extend(crate::triggers::walls_of(playfield).into_iter().filter(|(w, _)| near(w.centre()) <= radius).map(|(w, on)| {
        let state = if on { "blocks" } else { "open" };
        (near(w.centre()), format!("{:.0} m wall {} ({:?}, {}{}{state})", near(w.centre()), w.name, w.by, if w.movement { "movement " } else { "" }, if w.pathing { "paths " } else { "" }))
    }));
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    if lines.is_empty() {
        return vec![format!("no areas within {radius:.0} m")];
    }
    lines.into_iter().take(25).map(|(_, l)| l).collect()
}

/// Whether a chat line is a command.
pub fn is_command(message: &str) -> bool {
    message.starts_with('/') || message.starts_with('!')
}

/// The targets of a kind: (label, playfield, position).
fn targets(kind: &str) -> Option<Vec<(String, u32, [f32; 3])>> {
    Some(match kind {
        "buildie" => crate::buildie::points().into_iter().map(|(n, pf, p, state)| (format!("{n} [{state}]"), pf.model(), p)).collect(),
        "pad" => crate::jumppad::points(),
        "lever" => crate::scenery::points("lever"),
        "plate" => crate::scenery::points("pressureplate"),
        "gate" => [crate::scenery::points("gate"), crate::scenery::points("portcullis")].concat(),
        "mirror" => crate::scenery::points("mirror"),
        "pedestal" => crate::keygate::points(),
        "collectible" => crate::discovery::points(true),
        "boombox" => crate::discovery::points(false),
        "trigger" => crate::tutorial::points(),
        "dialogue" => crate::knubot::points(),
        "rebuilt" => crate::knubot::rebuilt_points(),
        "portal" => teleport::points(),
        "arena" | "ambush" | "explore" => crate::encounter::points(kind),
        "boss" | "champion" | "enemy" | "npc" | "fighter" | "patrol" | "smashable" => crate::npc::points(kind),
        "script" => crate::scripts::points(),
        "point" => crate::content::points(),
        "quest" => crate::quests::points(),
        "story" => crate::story::points(),
        "lc" => crate::lost_creations::points(),
        "area" | "wall" | "fallzone" | "speedpad" => crate::triggers::points(kind),
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

/// A command every player may type, GM or not (the others are test commands).
#[derive(Debug, PartialEq)]
pub enum PlayerCommand {
    /// `/story reset`: forget one's own story (asks first; `confirm`, or the same line again
    /// within 30 s, does it).
    StoryReset { confirm: bool },
}

/// The player commands: `/story reset [confirm]` for everyone; `!story reset [confirm]` too for
/// players without the test commands (`gm`: they have them, and `!story reset` is the test one).
pub fn player_command(message: &str, gm: bool) -> Option<PlayerCommand> {
    let rest = message.strip_prefix('/').or_else(|| message.strip_prefix('!').filter(|_| !gm))?;
    let words: Vec<String> = rest.split_whitespace().map(str::to_ascii_lowercase).collect();
    match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["story", "reset"] => Some(PlayerCommand::StoryReset { confirm: false }),
        ["story", "reset", "confirm"] => Some(PlayerCommand::StoryReset { confirm: true }),
        _ => None,
    }
}

/// Runs a command typed by a player standing at `pos` on `playfield`.
pub fn run(message: &str, playfield: Pf, pos: [f32; 3]) -> Action {
    let mut words = message[1..].split_whitespace();
    let cmd = words.next().unwrap_or("").to_ascii_lowercase();
    let args: Vec<&str> = words.collect();
    let reply = |lines: Vec<String>| Action::Reply(lines);
    match cmd.as_str() {
        "where" => {
            let copy = if playfield.is_main() { String::new() } else { format!(" copy {}", playfield.copy()) };
            reply(vec![format!("{}{copy}: {:.1} {:.1} {:.1}", playfield_name(playfield.model()), pos[0], pos[1], pos[2])])
        }

        "areas" => reply(areas_near(playfield, pos, args.first().and_then(|r| r.parse().ok()).unwrap_or(20.0))),
        "wall" => match (args.first().copied(), args.get(1..).map(|a| a.join(" ").to_ascii_lowercase()).filter(|n| !n.is_empty())) {
            (Some(how @ ("open" | "close")), Some(name)) => {
                let walls: Vec<_> = crate::triggers::walls_of(playfield)
                    .into_iter()
                    .filter(|(w, _)| w.id.to_ascii_lowercase().contains(&name) || w.name.to_ascii_lowercase().contains(&name))
                    .collect();
                for (w, _) in &walls {
                    crate::triggers::set_wall(playfield, &w.id, how == "open");
                }
                reply(vec![format!("{} {} wall(s): {}", if how == "open" { "opened" } else { "closed" }, walls.len(), walls.iter().map(|(w, _)| w.name.as_str()).collect::<Vec<_>>().join(", "))])
            }
            _ => reply(vec!["usage: !wall <open|close> <name>".into()]),
        },
        "moveto" | "move" => {
            let n: Vec<f32> = args.iter().filter_map(|a| a.trim_end_matches(',').parse::<f32>().ok()).filter(|v| v.is_finite()).collect();
            if n.len() != 3 || args.len() != 3 {
                return reply(vec![format!("usage: !{cmd} <{0}x> <{0}y> <{0}z> (y is up)", if cmd == "move" { "d" } else { "" })]);
            }
            let to = if cmd == "move" { [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]] } else { [n[0], n[1], n[2]] };
            Action::Enter(playfield, to, f32::NAN, format!("going to {:.1}, {:.1}, {:.1}", to[0], to[1], to[2]))
        }
        "tp" => {
            let query = args.join(" ");
            if query.is_empty() {
                return reply(vec!["usage: !tp <playfield number or name>".into()]);
            }
            // in a Lost Creations level: the next Gatekeeper's level, in the same run
            if query.eq_ignore_ascii_case("gatekeeper") && crate::lost_creations::is_level(playfield.model()) {
                return Action::Lc(crate::lost_creations::Command::NextGatekeeper);
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
                    Action::Enter(pf, to, (-ARRIVE_OFFSET).atan2(0.0), format!("going to {} in {}", p.character_name, playfield_name(pf.model())))
                }
                (Some(p), None) => reply(vec![format!("{} is online but not in a world yet", p.character_name)]),
                (None, _) if name.is_empty() => reply(vec!["usage: !goto player <name>".into()]),
                (None, _) => reply(vec![format!("nobody named {name} is online")]),
            }
        }
        "rename" => {
            let rest = args.join(" ");
            let (who, name) = rest.split_once('=').map_or(("me", rest.as_str()), |(w, n)| (w.trim(), n.trim()));
            if name.is_empty() {
                reply(vec!["usage: !rename [<who> =] <new name>".into()])
            } else {
                match rename_target(who) {
                    Ok(target) => Action::Rename(target, name.to_string()),
                    Err(e) => reply(vec![e]),
                }
            }
        }
        "point" if !args.is_empty() => Action::Point(args.join(" ")),
        "point" => reply(vec!["usage: !point <name>".into()]),
        "goto" if args.first().is_some_and(|k| k.eq_ignore_ascii_case("point")) && args.get(1).is_some_and(|n| n.parse::<usize>().is_err()) => {
            let name = args[1..].join(" ").to_ascii_lowercase();
            let all = crate::content::points();
            // the label is "NAME (area/<pf>/<slug>)": by name or slug, this world's first
            let fits = |l: &str| {
                let l = l.to_ascii_lowercase();
                l.split(' ').next() == Some(name.as_str()) || l.trim_end_matches(')').rsplit('/').next() == Some(name.as_str())
            };
            let found = all.iter().filter(|t| fits(&t.0)).min_by_key(|t| t.1 != playfield.model());
            match found {
                Some((label, pf, p)) if *pf == playfield.model() => Action::Enter(playfield, *p, f32::NAN, format!("going to point {label}")),
                Some((label, pf, p)) => Action::Teleport(*pf, *p, f32::NAN, format!("going to point {label} in {}", playfield_name(*pf))),
                None => reply(vec![format!("no point named {name} (!list point)")]),
            }
        }
        "goto" => {
            let Some((kind, t)) = args.first().and_then(|k| targets(k).map(|t| (k, t))) else {
                return reply(kinds_help());
            };
            let n = args.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
            match t.get(n.wrapping_sub(1)) {
                Some((label, pf, p)) => {
                    // (the rebuilt conversations: right inside the area that starts them)
                    let off = match *kind {
                        "patrol" => PATROL_WATCH,
                        "rebuilt" => 0.0,
                        _ => ARRIVE_OFFSET,
                    };
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
        "cinematic" | "cine" => match args.first() {
            None => {
                let mut lines = vec!["!cinematic <n|name|package>:".to_string()];
                lines.extend(crate::story::cinematics().into_iter().enumerate().map(|(i, (n, p, about))| format!("{} {n} ({p}): {about}", i + 1)));
                Action::Reply(lines)
            }
            Some(a) => match crate::story::find_cinematic(a) {
                Some(name) => Action::Story(Some(crate::story::Order::Replay(name)), false),
                None => reply(vec![format!("no cinematic \"{a}\" (!cinematic lists them)")]),
            },
        },
        "story" => {
            use crate::story::Order;
            match (args.first().copied(), args.get(1).copied()) {
                (None, _) => Action::Story(None, false),
                (Some("reset"), _) => Action::Story(None, true),
                (Some("goal"), Some(h)) => Action::Story(Some(Order::Goal { hash: h.to_ascii_uppercase(), requires: Vec::new() }), false),
                (Some("quest"), Some(l)) => match (crate::story::find_line(l), args.get(2).and_then(|n| n.parse::<usize>().ok())) {
                    (Some(main), Some(step)) => Action::Story(Some(Order::SetStep { main, step }), false),
                    (None, _) => reply(vec![format!("no story line \"{l}\" (pirate, medieval, space, mythology, dino)")]),
                    _ => reply(vec!["usage: !story quest <line> <step>".into()]),
                },
                (Some("advance"), l) => match l.map(crate::story::find_line) {
                    Some(None) => reply(vec![format!("no story line \"{}\"", l.unwrap_or_default())]),
                    found => Action::Story(Some(Order::Advance(found.flatten())), false),
                },
                (Some("spawn"), Some(h)) => Action::Story(Some(Order::Spawn(h.to_ascii_uppercase())), false),
                (Some("progress"), Some(h)) => {
                    let amount = args.get(2).and_then(|n| n.parse::<u32>().ok()).unwrap_or(1).min(1000);
                    Action::Story(Some(Order::Progress { hash: h.to_ascii_uppercase(), amount, wait: false }), false)
                }
                (Some("beat"), Some(t)) if t.parse::<u32>().is_ok() => Action::Story(Some(Order::Beat { text: t.parse().unwrap_or(0), requires: Vec::new() }), false),
                (Some("dialogue"), Some(d)) => {
                    let id = if d.starts_with("dialogue/") { d.to_string() } else { format!("dialogue/{d}") };
                    Action::Story(Some(Order::Dialogue(id)), false)
                }
                (Some("seen"), Some(c)) => match crate::story::find_cinematic(c) {
                    Some(name) => Action::Story(Some(Order::Cinematic(name)), false),
                    None => reply(vec![format!("no cinematic \"{c}\"")]),
                },
                _ => reply(vec!["usage: !story [reset] | quest <line> <step> | advance [line] | goal <hash> | progress <hash> [n] | spawn <hash> | beat <text> | dialogue <id> | seen <cinematic>".into()]),
            }
        }
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
        "grant" => match args.iter().map(|a| a.trim_matches(|c| c == '[' || c == ']').parse::<u32>()).collect::<Result<Vec<_>, _>>() {
            Ok(ids) if !ids.is_empty() => Action::Grant(ids),
            _ => reply(vec!["usage: !grant <achievement id ...> (the ids are in the dev UI's Achievements and the world editor: [1613])".into()]),
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
                Some("kills") => crate::achievements::Event::Defeated { id: Id32::new(0, 0), name, smashable: false, boss: false, champion: false, level: 1, playfield: playfield.model() },
                Some("smashes") => crate::achievements::Event::Defeated { id: Id32::new(0, 0), name, smashable: true, boss: false, champion: false, level: 1, playfield: playfield.model() },
                Some("builds") => crate::achievements::Event::Built { name },
                _ => return reply(vec!["usage: !count <kills|smashes|builds> [n] [name]".into()]),
            };
            Action::Progress(event, n)
        }
        "smash" => Action::Smash,
        "revive" => Action::Revive,
        "team" if !args.is_empty() => Action::Team(args.join(" ")),
        "team" => reply(vec!["usage: !team <character name> (online)".into()]),
        "mode" => match args.first().map(|a| a.to_ascii_lowercase()).as_deref() {
            Some("progression" | "play" | "game") => Action::SwitchMode(crate::instance::Mode::Progression),
            Some("maxed" | "maxxed") => Action::SwitchMode(crate::instance::Mode::Maxed),
            Some("editor" | "world-editor" | "worldeditor" | "survey") => Action::SwitchMode(crate::instance::Mode::WorldEditor),
            _ => reply(vec![format!(
                "this is the {} mode; usage: !mode <progression|maxed|editor> (you come back as your character there)",
                crate::instance::mode_of(playfield).name()
            )]),
        },
        "preview" => match args.first().copied() {
            None => Action::Preview(true),
            Some("end" | "off" | "back") => Action::Preview(false),
            Some(_) => reply(vec!["usage: !preview [end]".into()]),
        },
        // go to another running copy of this world (instance.rs), at the same spot
        "instance" => match args.first().map(|a| a.parse::<u32>()) {
            None => reply(vec![format!(
                "{} copy {} (key {:#x}); usage: !instance <copy> (0 = the main one)",
                playfield_name(playfield.model()),
                playfield.copy(),
                playfield.key()
            )]),
            Some(Ok(copy)) if copy <= crate::instance::MAX_COPY => {
                let to = Pf::copy_of(playfield.model(), copy);
                Action::Enter(to, pos, f32::NAN, format!("going to {} copy {copy} (key {:#x})", playfield_name(to.model()), to.key()))
            }
            Some(_) => reply(vec![format!("usage: !instance <0-{}>", crate::instance::MAX_COPY)]),
        },
        "smashme" => Action::SmashMe,
        "loot" => match args.first().copied() {
            Some(kind) if (crate::loot::TEST_KINDS.contains(&kind) || crate::gamedata::get().loot.contains_key(kind)) && args.get(1).is_none_or(|p| p.parse::<u32>().is_ok()) => {
                Action::Loot(kind.to_string(), args.get(1).and_then(|p| p.parse().ok()))
            }
            _ => reply(vec![format!("usage: !loot <{}|loot/<table>> [power]", crate::loot::TEST_KINDS.join("|"))]),
        },
        "power" => match args.first().copied() {
            None => Action::Power(None),
            Some(kind) if crate::loot::TEST_KINDS.contains(&kind) || crate::gamedata::get().loot.contains_key(kind) => Action::Power(Some(kind.to_string())),
            Some(_) => reply(vec![format!("usage: !power [{}|loot/<table>]", crate::loot::TEST_KINDS.join("|"))]),
        },
        "stats" => Action::Stats,
        "hurt" => match args.first().and_then(|n| n.trim_end_matches('%').parse::<u32>().ok()).filter(|n| (1..=100).contains(n)) {
            Some(n) => Action::Hurt(n, args.get(1).is_some_and(|a| a.eq_ignore_ascii_case("all"))),
            None => reply(vec!["usage: !hurt <1-100> [all] (percent of the maximum health)".into()]),
        },
        "element" => {
            let names = ["none", "light", "shadow", "tech", "magic", "nature"];
            match args.first().and_then(|a| a.parse::<u32>().ok().filter(|n| *n <= 5).or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(a)).map(|i| i as u32))) {
                Some(e) => Action::Element(e),
                None => reply(vec!["usage: !element <0-5|none|light|shadow|tech|magic|nature>".into()]),
            }
        }
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
        "buff" => match (args.first(), args.get(1).map(|s| s.parse::<f32>())) {
            (Some(b), None | Some(Ok(_))) => {
                let id = if b.starts_with("buff/") { b.to_string() } else { format!("buff/{b}") };
                Action::Buff(id, args.get(1).and_then(|s| s.parse::<f32>().ok()).map(|s| std::time::Duration::from_secs_f32(s.max(0.1))))
            }
            _ => reply(vec!["usage: !buff <buff id> [secs] (!buff speed-pad)".into()]),
        },
        "speed" => match args.first() {
            None => Action::Speed(None),
            Some(a) => match crate::speed::parse(a) {
                Ok(f) => Action::Speed(Some(f)),
                Err(why) => reply(vec![why, "usage: !speed [factor] (1 normal, 0.5 half, 2 double)".into()]),
            },
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
                crate::plates::set_all(crate::instance::towards(crate::instance::mode_of(playfield), playfield, crate::plates::PLAYFIELD, 0), how == "solve");
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
        "keygate" => match args.first().copied() {
            Some(how @ ("open" | "reset")) => Action::Keygate(how == "open"),
            _ => reply(vec!["usage: !keygate <open|reset>".into()]),
        },
        "mirrors" => match args.first().copied() {
            Some(how @ ("solve" | "reset")) => Action::Mirrors(how == "solve"),
            _ => reply(vec!["usage: !mirrors <solve|reset>".into()]),
        },
        "lc" => {
            use crate::lost_creations::Command;
            let usage = || reply(vec!["usage: !lc [start [level] [map] | objective | finish | time <s> | end | variant [id] | <level>]".into()]);
            match args.first().copied() {
                None => Action::Lc(Command::Status),
                Some("start") => {
                    let level = args.get(1).and_then(|n| n.parse::<u32>().ok());
                    let map = args.get(if level.is_some() { 2 } else { 1 }..).map(|m| m.join(" ")).filter(|m| !m.is_empty());
                    Action::Lc(Command::Start { level: level.unwrap_or(1), map })
                }
                Some("objective") => Action::Lc(Command::Objective),
                Some("finish") => Action::Lc(Command::Finish),
                Some("end") => Action::Lc(Command::End),
                Some("variant") => Action::Lc(Command::Variant(args.get(1).map(|v| v.to_string()))),
                Some("time") => match args.get(1).and_then(|n| n.parse::<u32>().ok()) {
                    Some(s) => Action::Lc(Command::Time(s)),
                    None => usage(),
                },
                Some(n) => match n.parse::<u32>() {
                    Ok(level) => Action::Progress(crate::achievements::Event::LostCreations { level }, 1),
                    Err(_) => usage(),
                },
            }
        }
        "cleared" => Action::Progress(crate::achievements::Event::Cleared { playfield: playfield.model() }, 1),
        "defeat" if !args.is_empty() => {
            Action::Progress(crate::achievements::Event::Defeated { id: Id32::new(0, 0), name: args.join(" "), smashable: false, boss: true, champion: false, level: 1, playfield: playfield.model() }, 1)
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
    fn testers_have_their_commands_only() {
        // the defaults: testers 1, every command 2
        assert_eq!(gm_level_for("!goto boss 1"), 1);
        assert_eq!(gm_level_for("/TP 4200"), 1);
        assert_eq!(gm_level_for("!own Pirate Captain"), 1);
        assert_eq!(gm_level_for("!spawn 3 skeleton"), 2);
        assert_eq!(gm_level_for("!fx 163540"), 2);
        assert_eq!(gm_level_for("!"), 2);
        assert_eq!(gm_level_for("!rename me = Grumpy Axe"), 4);
        assert_eq!(gm_level_for("!RENAME x"), 4);
        // every tester command exists
        for cmd in TESTER_COMMANDS {
            assert!(COMMANDS.iter().any(|(usage, _)| usage[1..].split_whitespace().next() == Some(cmd)), "{cmd}");
        }
    }

    #[test]
    fn test_scenarios_resolve() {
        assert!(matches!(run("!test", Pf::main(1200), [0.0; 3]), Action::Reply(l) if l.len() == SCENARIOS.scenarios.len() + 2));
        assert!(matches!(run("!test Homestone", Pf::main(1200), [0.0; 3]), Action::Story(Some(crate::story::Order::Spawn(h)), false) if h == "SLXS"));
        for (i, s) in SCENARIOS.scenarios.iter().enumerate() {
            // every scenario's minifigures and playfields exist (target kinds need the NPCs)
            if s.goto.as_ref().is_some_and(|g| g.kind.is_some()) {
                continue;
            }
            match run(&format!("!test {}", i + 1), Pf::main(1200), [0.0; 3]) {

                Action::Test(setup) => assert_eq!(setup.label, s.label),
                Action::Reply(l) => panic!("{}: {l:?}", s.label),
                _ => panic!("{}", s.label),
            }
        }
    }

    #[test]
    fn tp_by_name_and_number() {
        assert!(matches!(run("!tp isle of yarr", Pf::main(1200), [0.0; 3]), Action::Teleport(4200, ..)));
        assert!(matches!(run("/tp 5200", Pf::main(1200), [0.0; 3]), Action::Teleport(5200, ..)));
        assert!(matches!(run("!tp nowhere", Pf::main(1200), [0.0; 3]), Action::Reply(_)));
        assert!(matches!(run("!moveto 1 2 3", Pf::main(1200), [9.0; 3]), Action::Enter(pf, [1.0, 2.0, 3.0], r, _) if r.is_nan() && pf == Pf::main(1200)));
        assert!(matches!(run("!move 0 1000 -1", Pf::main(1200), [5.0, -900.0, 5.0]), Action::Enter(pf, [5.0, 100.0, 4.0], ..) if pf == Pf::main(1200)));
        assert!(matches!(run("!move 0 1000", Pf::main(1200), [0.0; 3]), Action::Reply(_)));
    }

    #[test]
    fn goto_the_live_dialogue_speakers() {
        // in capture order: the LEGO Center, the Monster Laboratory (2), the jail (2), ...
        assert!(matches!(run("!goto dialogue 1", Pf::main(4200), [0.0; 3]), Action::Teleport(1200, ..)));
        assert!(matches!(run("!goto dialogue 4", Pf::main(4200), [0.0; 3]), Action::Teleport(4610, ..)));
        assert!(matches!(run("!goto dialogue 6", Pf::main(4610), [0.0; 3]), Action::Teleport(4200, ..)));
        // the rebuilt story conversations' starts, in story order: Pirate World first
        assert!(matches!(run("!goto rebuilt 1", Pf::main(4200), [0.0; 3]), Action::Teleport(1632, ..)));
    }

    #[test]
    fn fx_takes_a_hook_and_a_place() {
        assert!(matches!(run("!fx 966", Pf::main(1200), [0.0; 3]), Action::Fx(Fx::Play(966, h, FxAt::Me)) if h.is_empty()));
        assert!(matches!(run("!fx 966 onhit @near", Pf::main(1200), [0.0; 3]), Action::Fx(Fx::Play(966, h, FxAt::Near)) if h == "onhit"));
        assert!(matches!(run("!fx 966 @ground", Pf::main(1200), [0.0; 3]), Action::Fx(Fx::Play(966, h, FxAt::Ground)) if h.is_empty()));
        assert!(matches!(run("!buff speed-pad", Pf::main(1200), [0.0; 3]), Action::Buff(b, None) if b == "buff/speed-pad"));
        assert!(matches!(run("!buff buff/speed-pad 60", Pf::main(1200), [0.0; 3]), Action::Buff(_, Some(d)) if d.as_secs() == 60));
        assert!(matches!(run("!buff speed-pad x", Pf::main(1200), [0.0; 3]), Action::Reply(_)));
        assert!(matches!(run("!fx 966 @moon", Pf::main(1200), [0.0; 3]), Action::Reply(_)));
    }

    #[test]
    fn anim_plays_an_attribute_package() {
        assert!(matches!(run("!anim limbostart", Pf::main(1200), [0.0; 3]), Action::Fx(Fx::Play(_, _, FxAt::Me))));
        assert!(matches!(run("!anim nothing_like_it", Pf::main(1200), [0.0; 3]), Action::Reply(_)));
        assert!(matches!(run("!speed", Pf::main(1200), [0.0; 3]), Action::Speed(None)));
        assert!(matches!(run("!speed 0.5", Pf::main(1200), [0.0; 3]), Action::Speed(Some(f)) if f == 0.5));
        assert!(matches!(run("!speed 20", Pf::main(1200), [0.0; 3]), Action::Reply(_)));
        assert!(matches!(run("!despawn all", Pf::main(1200), [0.0; 3]), Action::Despawn(true)));
        assert!(matches!(run("!despawn", Pf::main(1200), [0.0; 3]), Action::Despawn(false)));
    }

    #[test]
    fn spawn_takes_an_optional_count() {
        assert!(matches!(run("!spawn 3 Crate", Pf::main(1200), [0.0; 3]), Action::Spawn(n, 3) if n == "Crate"));
        assert!(matches!(run("!spawn Captain Dreadleg", Pf::main(1200), [0.0; 3]), Action::Spawn(n, 1) if n == "Captain Dreadleg"));
        assert!(matches!(run("!spawn 500 Crate", Pf::main(1200), [0.0; 3]), Action::Spawn(_, MAX_SPAWN)));
        assert!(matches!(run("!spawn", Pf::main(1200), [0.0; 3]), Action::Reply(_)));
        assert!(matches!(run("!spawn 0 Crate", Pf::main(1200), [0.0; 3]), Action::Reply(_)));
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
            assert!(!matches!(run(&line, Pf::main(1200), [0.0; 3]), Action::Reply(l) if l[0].starts_with("commands:")), "{word}");
        }
    }

    #[test]
    fn commands_are_recognised() {
        assert_eq!(player_command("/story reset", true), Some(PlayerCommand::StoryReset { confirm: false }));
        assert_eq!(player_command("/Story Reset confirm", true), Some(PlayerCommand::StoryReset { confirm: true }));
        assert_eq!(player_command("!story reset", false), Some(PlayerCommand::StoryReset { confirm: false }));
        // a GM's !story reset is the test command
        assert_eq!(player_command("!story reset", true), None);
        assert_eq!(player_command("/story", false), None);
        assert_eq!(player_command("story reset", false), None);
        assert!(is_command("!help") && is_command("/goto buildie") && !is_command("hello"));
        assert!(matches!(run("!list nothing", Pf::main(1200), [0.0; 3]), Action::Reply(l) if l.len() == KINDS.len()));
    }
}
