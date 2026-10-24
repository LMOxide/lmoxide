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
//!
//! New features add their test targets to `targets` so they can be reached the same way.

use crate::teleport;

/// What the game connection should do for a command.
pub enum Action {
    Reply(Vec<String>),
    /// teleport to (playfield, position, facing), with a reply
    Teleport(u32, [f32; 3], f32, String),
    /// show the achievement summary
    Achievements,
    /// feed achievement events (n times)
    Progress(crate::achievements::Event, u32),
    /// gain XP
    Xp(u64),
    /// gain stars
    Stars(u64),
}

/// A command's target kinds (for `!list` / `!goto`) and what they are.
const KINDS: &[(&str, &str)] = &[
    ("buildie", "buildables (buildie.rs)"),
    ("pad", "jump pads"),
    ("lever", "levers (scenery)"),
    ("plate", "pressure plates (scenery)"),
    ("gate", "gates and portcullis (scenery)"),
    ("mirror", "Medusa mirrors (scenery)"),
    ("boss", "dungeon bosses"),
    ("champion", "champions"),
    ("enemy", "all enemies"),
    ("npc", "friendly NPCs"),
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
        "boss" | "champion" | "enemy" | "npc" | "smashable" => crate::npc::points(kind),
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
                Some("kills") => crate::achievements::Event::Defeated { name, smashable: false, boss: false, champion: false, level: 1, playfield },
                Some("smashes") => crate::achievements::Event::Defeated { name, smashable: true, boss: false, champion: false, level: 1, playfield },
                Some("builds") => crate::achievements::Event::Built { name },
                _ => return reply(vec!["usage: !count <kills|smashes|builds> [n] [name]".into()]),
            };
            Action::Progress(event, n)
        }
        "defeat" if !args.is_empty() => {
            Action::Progress(crate::achievements::Event::Defeated { name: args.join(" "), smashable: false, boss: true, champion: false, level: 1, playfield }, 1)
        }
        _ => reply(vec![
            "commands: !where, !tp <playfield|name>, !list <kind>, !goto <kind> [n], !ach, !count <kills|smashes|builds> [n] [name], !defeat <boss>, !xp [n], !stars <n>".into(),
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
    fn tp_by_name_and_number() {
        assert!(matches!(run("!tp isle of yarr", 1200, [0.0; 3]), Action::Teleport(4200, ..)));
        assert!(matches!(run("/tp 5200", 1200, [0.0; 3]), Action::Teleport(5200, ..)));
        assert!(matches!(run("!tp nowhere", 1200, [0.0; 3]), Action::Reply(_)));
    }

    #[test]
    fn commands_are_recognised() {
        assert!(is_command("!help") && is_command("/goto buildie") && !is_command("hello"));
        assert!(matches!(run("!list nothing", 1200, [0.0; 3]), Action::Reply(l) if l.len() == KINDS.len()));
    }
}
