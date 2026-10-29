//! Trigger walls on play servers: the world editor's walls (and radii) that are approved and
//! active here (content.rs), checked against the players' moves like the survey server does.
//! Crossing one does what it says: discovers a location (and the achievement it counts toward),
//! opens a recorded dialogue or a portal, or shows a message. Here they are invisible, as live's
//! trigger areas were. A script wall starts the script (or the wave it names; scripts.rs).

use crate::instance::Pf;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use lmoxide_net::types::Id32;
use lmoxide_survey::{TriggerWall, WallAction};
use tracing::info;

use crate::{achievements::Event, world};

/// How often players' moves are checked (as the survey server's walls).
const TICK: Duration = Duration::from_millis(200);
/// A move longer than this between two checks is a teleport, not a walk through a wall.
const JUMP: f32 = 15.0;

#[derive(Default)]
struct State {
    /// The walls by playfield.
    walls: BTreeMap<u32, Vec<TriggerWall>>,
    /// Where each player was at the last check (playfield, position).
    last: HashMap<Id32, (Pf, [f32; 3])>,
    /// "Only once" walls each player crossed already (wall id, player), until a restart.
    fired: HashSet<(String, Id32)>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);
static WATCHING: AtomicBool = AtomicBool::new(false);

/// The content's walls by playfield (content.rs) are the ones crossing does something; starts
/// watching the first time.
pub fn set(walls: BTreeMap<u32, Vec<TriggerWall>>) {
    let n: usize = walls.values().map(Vec::len).sum();
    let mut st = STATE.lock().unwrap();
    if st.walls != walls {
        info!(walls = n, playfields = walls.len(), "world content: trigger walls");
    }
    st.walls = walls;
    drop(st);
    if n > 0 && !WATCHING.swap(true, Ordering::SeqCst) {
        tokio::spawn(watch());
    }
}

async fn watch() {
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        // where the players are (outside the lock)
        let players: Vec<(Id32, String, Pf, [f32; 3])> = crate::sessions::players()
            .into_iter()
            .filter_map(|p| {
                let id = Id32::new(crate::territory::CHARACTER_TYPE, p.character_id);
                // (the world editor's copies run no play content)
                let (pf, _) = world::whereabouts(id).filter(|(pf, _)| !crate::instance::rules(*pf).editor)?;
                Some((id, p.character_name, pf, world::live_position(id)?))
            })
            .collect();
        let mut crossed = Vec::new();
        {
            let mut st = STATE.lock().unwrap();
            let State { walls, last, fired } = &mut *st;
            last.retain(|id, _| players.iter().any(|p| p.0 == *id));
            for (player, name, pf, pos) in &players {
                let Some((last_pf, was)) = last.insert(*player, (*pf, *pos)) else { continue };
                if last_pf != *pf || was == *pos || dist(was, *pos) > JUMP {
                    continue;
                }
                // a preview's walls are the working copy's (preview.rs)
                let preview = crate::preview::of(*pf);
                let list = match &preview {
                    Some(p) => p.walls.as_slice(),
                    None => walls.get(&pf.model()).map_or(&[][..], Vec::as_slice),
                };
                for w in list {
                    if w.crossed(was, *pos) && !(w.once && fired.contains(&(w.id.clone(), *player))) {
                        if w.once {
                            fired.insert((w.id.clone(), *player));
                        }
                        crossed.push((w.clone(), *player, name.clone(), *pf));
                    }
                }
            }
        }
        for (w, player, name, pf) in crossed {
            fire(&w, player, &name, pf);
        }
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// What crossing wall `w` on `pf` does for `player`.
fn fire(w: &TriggerWall, player: Id32, player_name: &str, pf: Pf) {
    info!(wall = %w.id, name = %w.name, player = player.instance, action = ?w.action, "trigger wall crossed");
    match &w.action {
        WallAction::Discover { name, achievement } => {
            for event in discoveries(pf.model(), name.as_deref(), achievement.as_deref()) {
                world::send_to(player, world::Out::Progress(event));
            }
        }
        WallAction::Dialog { dialog } => match dialog.as_deref().and_then(|d| d.parse().ok()).and_then(crate::knubot::starting_with) {
            Some(d) => {
                // the speaker: that NPC if it's here, else the player
                let owner = crate::npc::find(&d.npc, pf).unwrap_or(player);
                crate::knubot::start(player, player_name, owner, d);
            }
            None => info!(wall = %w.id, ?dialog, "trigger wall: no recorded dialogue starts with that text"),
        },
        WallAction::Portal { window: Some(window), .. } => world::send_to(player, world::Out::Window(*window)),
        WallAction::Portal { window: None, to: Some(to) } => match crate::teleport::destination(*to) {
            Some(d) => world::send_to(player, world::Out::Teleport(Some(*to), d.pos, d.rot)),
            None => info!(wall = %w.id, to, "trigger wall: no arrival point for that playfield"),
        },
        WallAction::Message { text } if !text.is_empty() => crate::chat::system(player.instance, text),
        WallAction::Script { script: Some(script), wave } => match crate::scripts::start(pf, script, wave.as_deref()) {
            Ok(what) => info!(wall = %w.id, "trigger wall: {what}"),
            Err(why) => info!(wall = %w.id, "trigger wall: script not started: {why}"),
        },
        WallAction::Script { script: None, .. } => info!(wall = %w.id, "trigger wall: names no script"),
        WallAction::Collect { drop } => crate::achievements::wall_collect(player, drop.as_deref(), &w.id),
        WallAction::Portal { .. } | WallAction::Message { .. } | WallAction::Other => {}
    }
}

/// What a "discovers a location" wall counts: the location found (an exploration area's or
/// world object's name, on this playfield), and for an achievement that needs one location, that
/// location (so it is granted even if the wall names another or none); an achievement that needs
/// several counts only the ones found. An achievement the trigger targets don't know (no
/// location) is granted as its tag.
fn discoveries(pf: u32, name: Option<&str>, achievement: Option<&str>) -> Vec<Event> {
    let mut out: Vec<Event> = name.filter(|n| !n.is_empty()).map(|n| Event::Found { playfield: pf, name: n.to_string() }).into_iter().collect();
    let Some(a) = achievement.filter(|a| !a.is_empty()) else { return out };
    match crate::survey::achievement_locations(a) {
        Some(locations) if locations.len() == 1 => {
            let (playfield, name) = locations.into_iter().next().expect("one");
            let found = Event::Found { playfield, name };
            if !out.contains(&found) {
                out.push(found);
            }
        }
        Some(_) => {}
        None => {
            if let Ok(id) = a.parse() {
                out.push(Event::Tag(id));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovering() {
        let found = |pf: u32, n: &str|
 Event::Found { playfield: pf, name: n.into() };
        // Fiery Explorer: its own location, whatever the wall names
        assert_eq!(discoveries(6200, Some("Achievement_Volcano"), Some("1613")), [found(6200, "Achievement_Volcano")]);
        assert_eq!(discoveries(6200, None, Some("1613")), [found(6200, "Achievement_Volcano")]);
        assert_eq!(discoveries(6200, Some("Elsewhere"), Some("1613")), [found(6200, "Elsewhere"), found(6200, "Achievement_Volcano")]);
        // Paleontologist needs all of Dinosaur Rise's: only what was found counts
        assert_eq!(discoveries(6200, Some("Achievement_Swamp"), Some("1539")), [found(6200, "Achievement_Swamp")]);
        assert_eq!(discoveries(6200, None, None), []);
    }
}
