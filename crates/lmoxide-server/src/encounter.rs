//! Encounters: the original server's Scry scripts on bounded areas (data/encounters.json,
//! tools/re/extract_encounters.py; research notes in research/lmoxide-notes/world-events.md).
//! Only the areas, the arena doors and the recorded tutorial survive; what runs here is
//! inferred (docs/TUNABLES.md "Encounters"):
//!
//! - Boss arenas: a player entering the start area engages the boss inside: the entrance closes
//!   (VisualOpen hidden, VisualClosed shown, as recorded in the tutorial) and the players
//!   inside hear the boss music. At 2/3 and 1/3 of its health the boss gets adds (regular
//!   enemies of the area). Its defeat opens the entrance and the exit and stops the music;
//!   when nobody is left near the arena it resets (full health, adds gone, doors open).
//! - Ambushes: entering an ambush area spawns two waves (as the recorded tutorial ambush: 3
//!   and 3) of the enemies around there, then the area rests a while.
//! - Champions: each side dungeon ("Defeat the champion" area quests) gets a champion named
//!   after one of its world's champions (their achievements), at "BA_SpawnChampion" or deep
//!   in the dungeon; one at a time, a new one a while after the last fell.
//!
//! Pocket worlds (dungeons; live gave each group its own instance) run each encounter once: a
//! won arena, a spent ambush and a fallen champion stay so until nobody is left there (a new
//! instance). In the open worlds they come back after their rest.
//!
//! Exploration: the "Achievement_*" / "BA_Explore_*" areas the exploration achievements name.
//!
//! What lies past a boss (farther from the playfield's entrance than its arena's start: later
//! ambushes, arenas, the champion) stays quiet until that boss is beaten.
//!
//! Locking: STATE is never held while calling into npc.rs or world.rs.

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use lmoxide_net::{iir, types::Id32};
use rand::seq::IndexedRandom;
use serde::Deserialize;
use tracing::info;

use crate::{npc::SpawnDef, world};

const TICK: Duration = Duration::from_secs(1);
/// An engaged arena nobody is near any more resets after this.
const RESET_AFTER: Duration = Duration::from_secs(20);
/// Boss health shares that bring adds.
const ADD_THRESHOLDS: [f32; 2] = [2.0 / 3.0, 1.0 / 3.0];
const ADDS: usize = 3;
const AMBUSH_WAVE: usize = 3;
const AMBUSH_REST: Duration = Duration::from_secs(5 * 60);
/// How far around an ambush area its enemies come from.
const AMBUSH_SOURCE: f32 = 80.0;
const CHAMPION_REST: Duration = Duration::from_secs(10 * 60);
/// A champion's health over a regular enemy's of its level (4x and more counts as a champion,
/// 8x as a boss: npc.rs).
const CHAMPION_HEALTH: u32 = 5;
const SPAWN_LIFETIME: Duration = Duration::from_secs(15 * 60);
const RING: f32 = 6.0;
/// How near an area's edge counts as in it (m).
const EDGE_SLACK: f32 = 3.0;

#[derive(Deserialize, Clone)]
struct Area {
    name: String,
    polygon: Vec<[f32; 2]>,
    heights: [f32; 2],
}

impl Area {
    fn centre(&self) -> [f32; 2] {
        let n = self.polygon.len() as f32;
        [self.polygon.iter().map(|p| p[0]).sum::<f32>() / n, self.polygon.iter().map(|p| p[1]).sum::<f32>() / n]
    }

    fn radius(&self) -> f32 {
        let c = self.centre();
        self.polygon.iter().map(|p| (p[0] - c[0]).hypot(p[1] - c[1])).fold(0.0, f32::max)
    }

    /// Point in polygon (x, z) or within EDGE_SLACK of it (start areas are often thin strips
    /// across an entrance, which a movement update can step over), within the heights.
    fn contains(&self, pos: [f32; 3]) -> bool {
        if pos[1] < self.heights[0] - 5.0 || pos[1] > self.heights[1] + 5.0 {
            return false;
        }
        let (x, z) = (pos[0], pos[2]);
        let mut inside = false;
        let n = self.polygon.len();
        for i in 0..n {
            let (a, b) = (self.polygon[i], self.polygon[(i + n - 1) % n]);
            if (a[1] > z) != (b[1] > z) && x < (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]) + a[0] {
                inside = !inside;
            }
            // distance to the edge a-b
            let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
            let t = (((x - a[0]) * dx + (z - a[1]) * dz) / (dx * dx + dz * dz).max(1e-6)).clamp(0.0, 1.0);
            if (x - a[0] - t * dx).hypot(z - a[1] - t * dz) <= EDGE_SLACK {
                return true;
            }
        }
        inside
    }
}

#[derive(Deserialize, Default, Clone)]
struct Doors {
    open: Vec<u32>,
    closed: Vec<u32>,
    exit: Vec<u32>,
}

#[derive(Deserialize, Clone)]
struct ArenaDef {
    #[serde(flatten)]
    area: Area,
    doors: Doors,
}

#[derive(Deserialize, Default)]
struct PlayfieldDef {
    #[serde(default)]
    arenas: Vec<ArenaDef>,
    #[serde(default)]
    ambushes: Vec<Area>,
    #[serde(default)]
    champion: Option<[f32; 2]>,
    #[serde(default)]
    explore: Vec<Area>,
}

/// Per playfield: for each ambush and arena, the arenas (indices) whose boss must be beaten
/// first, the arenas whose start is nearer the entrance.
struct Gates {
    ambushes: Vec<Vec<usize>>,
    arenas: Vec<Vec<usize>>,
    champion: Vec<usize>,
}

static GATES: LazyLock<HashMap<u32, Gates>> = LazyLock::new(|| {
    let dest: serde_json::Value = serde_json::from_str(include_str!("../../../data/destinations.json")).expect("destinations.json");
    DATA.iter()
        .filter_map(|(&pf, def)| {
            let p = dest["destinations"][pf.to_string()]["pos"].as_array()?;
            let entrance = [p[0].as_f64()? as f32, p[2].as_f64()? as f32];
            let far = |c: [f32; 2]| (c[0] - entrance[0]).hypot(c[1] - entrance[1]);
            let starts: Vec<f32> = def.arenas.iter().map(|a| far(a.area.centre())).collect();
            let before = |d: f32| starts.iter().enumerate().filter(|(_, s)| **s + 1.0 < d).map(|(i, _)| i).collect::<Vec<_>>();
            Some((
                pf,
                Gates {
                    ambushes: def.ambushes.iter().map(|a| before(far(a.centre()))).collect(),
                    arenas: starts.iter().map(|d| before(*d)).collect(),
                    champion: def.champion.map(|c| before(far(c))).unwrap_or_default(),
                },
            ))
        })
        .collect()
});

/// Whether every boss in front of a zone (`gates`) has been beaten (an arena without a boss
/// doesn't hold anything back).
fn unlocked(playfield: u32, gates: &[usize]) -> bool {
    let open: Vec<usize> = {
        let st = STATE.lock().unwrap();
        gates.iter().copied().filter(|i| !matches!(st.arenas.get(&(playfield, *i)), Some(Arena::Won { .. }))).collect()
    };
    open.into_iter().all(|i| {
        DATA.get(&playfield)
            .and_then(|d| d.arenas.get(i))
            .is_none_or(|a| crate::npc::boss_near(playfield, a.area.centre(), a.area.radius() + 60.0).is_none())
    })
}

static DATA: LazyLock<HashMap<u32, PlayfieldDef>> = LazyLock::new(|| {
    let doc: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/encounters.json")).expect("encounters.json");
    doc.into_iter()
        .filter(|(pf, _)| pf != &crate::tutorial::PLAYFIELD.to_string()) // tutorial.rs runs its own
        .filter_map(|(pf, v)| Some((pf.parse().ok()?, serde_json::from_value(v).expect("encounters.json playfield"))))
        .collect()
});

#[derive(Clone)]
enum Arena {
    Idle,
    Engaged { boss: Id32, players: HashSet<Id32>, adds: Vec<Id32>, waves: usize, empty_since: Option<Instant> },
    Won { boss: Id32 },
}

#[derive(Clone)]
enum Ambush {
    Idle,
    Wave { n: usize, ids: Vec<Id32>, around: [f32; 3] },
    Resting(Instant),
    /// pocket worlds: spent until the playfield empties
    Spent,
}

#[derive(Default)]
struct State {
    arenas: HashMap<(u32, usize), Arena>,
    ambushes: HashMap<(u32, usize), Ambush>,
    /// side dungeons: the champion and when the next may come
    champions: HashMap<u32, (Option<Id32>, Instant)>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

/// Starts the encounter clock.
pub fn start() {
    info!(
        arenas = DATA.values().map(|p| p.arenas.len()).sum::<usize>(),
        ambushes = DATA.values().map(|p| p.ambushes.len()).sum::<usize>(),
        "encounters"
    );
    tokio::spawn(async {
        let mut every = tokio::time::interval(TICK);
        loop {
            every.tick().await;
            tick();
        }
    });
}

/// The open worlds (and the hub); every other playfield is a pocket world.
const OPEN_WORLDS: [u32; 11] = [1200, 4200, 4210, 4600, 5200, 5210, 3200, 3210, 2210, 6200, 1300];

/// Whether `playfield` is a pocket world (instanced on live).
pub fn pocket(playfield: u32) -> bool {
    !OPEN_WORLDS.contains(&playfield)
}

/// The exploration areas the player at `pos` is in.
pub fn explored(playfield: u32, pos: [f32; 3]) -> Vec<&'static str> {
    let Some(def) = DATA.get(&playfield) else { return vec![] };
    def.explore.iter().filter(|a| a.contains(pos)).map(|a| a.name.as_str()).collect()
}

/// A player moved to `pos` on `playfield`.
pub fn moved(player: Id32, playfield: u32, pos: [f32; 3]) {
    let Some(def) = DATA.get(&playfield) else { return };
    let gates = GATES.get(&playfield);
    for (i, arena) in def.arenas.iter().enumerate() {
        if arena.area.contains(pos) && gates.is_none_or(|g| unlocked(playfield, &g.arenas[i])) {
            enter_arena(player, playfield, i, arena);
        }
    }
    for (i, area) in def.ambushes.iter().enumerate() {
        if area.contains(pos) && gates.is_none_or(|g| unlocked(playfield, &g.ambushes[i])) {
            enter_ambush(playfield, i, area, pos);
        }
    }
}

/// A player arrived on `playfield` at `pos`: a side dungeon's champion, if it's due.
/// `defeated` says which champions the player has beaten already (new ones are picked first).
pub fn entered(playfield: u32, pos: [f32; 3], defeated: &dyn Fn(&str) -> bool) {
    let names = crate::achievements::champions_for(playfield);
    if names.is_empty() {
        return;
    }
    let due = STATE.lock().unwrap().champions.get(&playfield).is_none_or(|(id, ready)| id.is_none() && Instant::now() >= *ready);
    if !due || GATES.get(&playfield).is_some_and(|g| !unlocked(playfield, &g.champion)) {
        return;
    }
    // where: the champion spot, else the regular enemy farthest from the entrance
    let spot = DATA.get(&playfield).and_then(|d| d.champion);
    let regulars = crate::npc::regulars_near(playfield, spot.unwrap_or([pos[0], pos[2]]), f32::MAX, usize::MAX);
    let base = match spot {
        Some(_) => regulars.first(),
        None => regulars.last(),
    };
    let Some(base) = base else { return };
    let fresh: Vec<&String> = names.iter().filter(|n| !defeated(n)).collect();
    let name = fresh.choose(&mut rand::rng()).copied().or_else(|| names.choose(&mut rand::rng())).cloned().unwrap_or_default();
    let at = spot.map_or([base.pos[0] + 2.0, base.pos[1], base.pos[2]], |s| [s[0], base.pos[1], s[1]]);
    let def = SpawnDef { name: name.clone(), health: (60 + 15 * base.level) * CHAMPION_HEALTH, pos: at, ..base.clone() };
    let id = crate::npc::spawn_temporary(def, playfield, SPAWN_LIFETIME);
    info!(playfield, %name, ?at, "champion");
    STATE.lock().unwrap().champions.insert(playfield, (Some(id), Instant::now()));
}

/// Whether a door (simple dynel instance) is hidden now: entrances are open (VisualOpen shown,
/// VisualClosed hidden) unless the arena is engaged; the exit opens when the boss is beaten.
pub fn door_hidden(guid: u32) -> Option<bool> {
    let (playfield, i, kind) = *DOORS.get(&guid)?;
    let state = STATE.lock().unwrap().arenas.get(&(playfield, i)).cloned().unwrap_or(Arena::Idle);
    Some(match (kind, state) {
        (Door::Open, Arena::Engaged { .. }) => true,
        (Door::Open, _) => false,
        (Door::Closed, Arena::Engaged { .. }) => false,
        (Door::Closed, _) => true,
        (Door::Exit, Arena::Won { .. }) => true,
        (Door::Exit, _) => false,
    })
}

/// Whether a simple dynel is an arena door (spawned by scenery.rs).
pub fn is_door(guid: u32) -> bool {
    DOORS.contains_key(&guid)
}

#[derive(Clone, Copy)]
enum Door {
    Open,
    Closed,
    Exit,
}

static DOORS: LazyLock<HashMap<u32, (u32, usize, Door)>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    for (&pf, def) in DATA.iter() {
        for (i, a) in def.arenas.iter().enumerate() {
            for (list, kind) in [(&a.doors.open, Door::Open), (&a.doors.closed, Door::Closed), (&a.doors.exit, Door::Exit)] {
                for &g in list {
                    m.insert(g, (pf, i, kind));
                }
            }
        }
    }
    m
});

/// Test command targets: arena starts, ambushes or exploration areas (centre, at the height of
/// the nearest enemy, else the area's floor).
pub fn points(kind: &str) -> Vec<(String, u32, [f32; 3])> {
    let mut v = Vec::new();
    for (&pf, def) in DATA.iter() {
        let areas: Vec<&Area> = match kind {
            "arena" => def.arenas.iter().map(|a| &a.area).collect(),
            "ambush" => def.ambushes.iter().collect(),
            _ => def.explore.iter().collect(),
        };
        for a in areas {
            let c = a.centre();
            let y = crate::npc::regulars_near(pf, c, f32::MAX, 1).first().map_or(a.heights[0], |d| d.pos[1]);
            v.push((a.name.clone(), pf, [c[0], y, c[1]]));
        }
    }
    v.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    v
}

fn enter_arena(player: Id32, playfield: u32, i: usize, arena: &ArenaDef) {
    let state = STATE.lock().unwrap().arenas.get(&(playfield, i)).cloned().unwrap_or(Arena::Idle);
    match state {
        Arena::Idle => {
            let Some(boss) = crate::npc::boss_near(playfield, arena.area.centre(), arena.area.radius() + 60.0) else { return };
            if crate::npc::health_of(boss).is_none() {
                return;
            }
            {
                let mut st = STATE.lock().unwrap();
                let entry = st.arenas.entry((playfield, i)).or_insert(Arena::Idle);
                if !matches!(entry, Arena::Idle) {
                    return;
                }
                *entry = Arena::Engaged { boss, players: [player].into(), adds: vec![], waves: 0, empty_since: None };
            }
            info!(playfield, arena = %arena.area.name, ?boss, ?player, "arena engaged");
            set_doors(arena);
            music(player, true);
        }
        Arena::Engaged { players, .. } if !players.contains(&player) => {
            if let Some(Arena::Engaged { players, .. }) = STATE.lock().unwrap().arenas.get_mut(&(playfield, i)) {
                players.insert(player);
            }
            music(player, true);
        }
        _ => {}
    }
}

fn enter_ambush(playfield: u32, i: usize, area: &Area, at: [f32; 3]) {
    {
        let mut st = STATE.lock().unwrap();
        let entry = st.ambushes.entry((playfield, i)).or_insert(Ambush::Idle);
        if !matches!(entry, Ambush::Idle) {
            return;
        }
        // claimed (the wave's ids follow)
        *entry = Ambush::Wave { n: 1, ids: vec![], around: at };
    }
    let ids = spawn_wave(playfield, area.centre(), at, AMBUSH_WAVE);
    info!(playfield, ambush = %area.name, spawned = ids.len(), "ambush");
    let next = match (ids.is_empty(), pocket(playfield)) {
        (true, true) => Ambush::Spent,
        (true, false) => Ambush::Resting(Instant::now() + AMBUSH_REST),
        _ => Ambush::Wave { n: 1, ids, around: at },
    };
    STATE.lock().unwrap().ambushes.insert((playfield, i), next);
}

/// `n` regular enemies from around `source` (x, z), in a ring around `at`.
fn spawn_wave(playfield: u32, source: [f32; 2], at: [f32; 3], n: usize) -> Vec<Id32> {
    let kinds = crate::npc::regulars_near(playfield, source, AMBUSH_SOURCE, n);
    if kinds.is_empty() {
        return vec![];
    }
    (0..n)
        .map(|k| {
            let angle = k as f32 / n as f32 * std::f32::consts::TAU;
            let pos = [at[0] + RING * angle.cos(), at[1], at[2] + RING * angle.sin()];
            crate::npc::spawn_temporary(SpawnDef { pos, ..kinds[k % kinds.len()].clone() }, playfield, SPAWN_LIFETIME)
        })
        .collect()
}

fn tick() {
    let arenas: Vec<((u32, usize), Arena)> = STATE.lock().unwrap().arenas.iter().map(|(k, v)| (*k, v.clone())).collect();
    for ((playfield, i), state) in arenas {
        let Some(def) = DATA.get(&playfield).and_then(|d| d.arenas.get(i)) else { continue };
        let next = match state {
            Arena::Engaged { boss, players, mut adds, mut waves, empty_since } => match crate::npc::health_of(boss) {
                None => {
                    info!(playfield, arena = %def.area.name, "arena won");
                    for p in &players {
                        music(*p, false);
                    }
                    Some(Arena::Won { boss })
                }
                Some((health, max)) => {
                    while waves < ADD_THRESHOLDS.len() && (health as f32) <= max as f32 * ADD_THRESHOLDS[waves] {
                        waves += 1;
                        if let Some(at) = crate::npc::position(boss) {
                            adds.extend(spawn_wave(playfield, def.area.centre(), at, ADDS));
                        }
                    }
                    let c = def.area.centre();
                    let near = players.iter().any(|p| {
                        world::whereabouts(*p).is_some_and(|(pf, pos)| pf == playfield && (pos[0] - c[0]).hypot(pos[2] - c[1]) <= def.area.radius() * 1.5 + 30.0)
                    });
                    let empty_since = if near { None } else { Some(empty_since.unwrap_or_else(Instant::now)) };
                    if empty_since.is_some_and(|t| t.elapsed() >= RESET_AFTER) {
                        info!(playfield, arena = %def.area.name, "arena reset");
                        crate::npc::restore(boss);
                        for a in adds {
                            crate::npc::despawn(a);
                        }
                        for p in &players {
                            music(*p, false);
                        }
                        Some(Arena::Idle)
                    } else {
                        Some(Arena::Engaged { boss, players, adds, waves, empty_since })
                    }
                }
            },
            // the boss is back: the arena is ready again
            Arena::Won { boss } if !pocket(playfield) && crate::npc::health_of(boss).is_some() => Some(Arena::Idle),
            _ => None,
        };
        if let Some(next) = next {
            let changed = std::mem::discriminant(&next) != std::mem::discriminant(&STATE.lock().unwrap().arenas[&(playfield, i)]);
            STATE.lock().unwrap().arenas.insert((playfield, i), next);
            if changed {
                set_doors(def);
            }
        }
    }

    let ambushes: Vec<((u32, usize), Ambush)> = STATE.lock().unwrap().ambushes.iter().map(|(k, v)| (*k, v.clone())).collect();
    for ((playfield, i), state) in ambushes {
        let next = match state {
            Ambush::Wave { n, ids, around } if !ids.is_empty() && ids.iter().all(|id| crate::npc::health_of(*id).is_none()) => {
                let area = &DATA[&playfield].ambushes[i];
                match n {
                    1 => {
                        let ids = spawn_wave(playfield, area.centre(), around, AMBUSH_WAVE);
                        Some(Ambush::Wave { n: 2, ids, around })
                    }
                    _ if pocket(playfield) => Some(Ambush::Spent),
                    _ => Some(Ambush::Resting(Instant::now() + AMBUSH_REST)),
                }
            }
            Ambush::Resting(until) if Instant::now() >= until => Some(Ambush::Idle),
            _ => None,
        };
        if let Some(next) = next {
            STATE.lock().unwrap().ambushes.insert((playfield, i), next);
        }
    }

    let champions: Vec<(u32, Id32)> = STATE.lock().unwrap().champions.iter().filter_map(|(pf, (id, _))| Some((*pf, (*id)?))).collect();
    for (playfield, id) in champions {
        if crate::npc::health_of(id).is_none() {
            // pocket worlds: not again in this instance (until it empties)
            let rest = if pocket(playfield) { Duration::from_secs(365 * 24 * 3600) } else { CHAMPION_REST };
            STATE.lock().unwrap().champions.insert(playfield, (None, Instant::now() + rest));
        }
    }

    // pocket worlds nobody is in any more: a new instance next time
    let pockets: HashSet<u32> = {
        let st = STATE.lock().unwrap();
        st.arenas.keys().map(|k| k.0).chain(st.ambushes.keys().map(|k| k.0)).chain(st.champions.keys().copied()).filter(|pf| pocket(*pf)).collect()
    };
    for playfield in pockets {
        if world::positions(playfield).iter().any(|(id, _)| crate::npc::is_player(*id)) {
            continue;
        }
        let won: Vec<usize> = {
            let mut st = STATE.lock().unwrap();
            st.ambushes.retain(|k, a| k.0 != playfield || matches!(a, Ambush::Wave { .. }));
            st.champions.retain(|pf, (id, _)| *pf != playfield || id.is_some());
            let won = st.arenas.iter().filter(|(k, a)| k.0 == playfield && matches!(a, Arena::Won { .. })).map(|(k, _)| k.1).collect();
            st.arenas.retain(|k, a| k.0 != playfield || !matches!(a, Arena::Won { .. }));
            won
        };
        for i in won {
            if let Some(def) = DATA.get(&playfield).and_then(|d| d.arenas.get(i)) {
                set_doors(def);
            }
        }
    }
}

/// Shows the arena's doors as its state has them.
fn set_doors(def: &ArenaDef) {
    for &guid in def.doors.open.iter().chain(&def.doors.closed).chain(&def.doors.exit) {
        if let Some(hidden) = door_hidden(guid) {
            let id = Id32::new(crate::scenery::SIMPLE_DYNEL_TYPE, guid);
            world::send_playfield(id, false, world::Out::Records(vec![door_record(id, hidden)]));
        }
    }
}

/// `UpdateSimpleDynelIIR_c` as recorded: u32 0, u8 hidden, u8 1, u8 0, u8 1.
pub fn door_record(id: Id32, hidden: bool) -> Bytes {
    iir::record("UpdateSimpleDynelIIR_c", id, false, |w| w.u32(0).u8(hidden as u8).u8(1).u8(0).u8(1))
}

fn music(player: Id32, on: bool) {
    let message = if on { "CombatMusic_Boss" } else { "CombatMusic_Stop" };
    world::send_to(player, world::Out::Records(vec![crate::tutorial::music_record(player, message)]));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_and_polygons() {
        let medusa = DATA[&2300].arenas.iter().find(|a| a.area.name == "BossEncounter_Start_Medusa").unwrap();
        let c = medusa.area.centre();
        assert!(medusa.area.contains([c[0], medusa.area.heights[0] + 1.0, c[1]]));
        assert!(!medusa.area.contains([c[0] + 500.0, medusa.area.heights[0] + 1.0, c[1]]));
        assert!(!DATA.contains_key(&crate::tutorial::PLAYFIELD));
        assert_eq!(door_hidden(medusa.doors.closed[0]), Some(true));
        assert_eq!(door_hidden(medusa.doors.open[0]), Some(false));
        assert!(DATA.values().map(|d| d.ambushes.len()).sum::<usize>() > 50);
        // Pirate Town (exploration) and which playfields are pocket worlds
        let town = DATA[&4200].explore.iter().find(|a| a.name == "Achievement_PirateTown").unwrap();
        let c = town.centre();
        assert_eq!(explored(4200, [c[0], town.heights[0] + 1.0, c[1]]), vec!["Achievement_PirateTown"]);
        assert!(pocket(2300) && pocket(1640) && !pocket(4200) && !pocket(1200));
        // past a boss: Medusa's Lair's ambushes lie before her arena; Underworld's last arenas
        // wait for the earlier ones
        for pf in [2300, 2320, 4310, 5500] {
            let (d, g) = (&DATA[&pf], &GATES[&pf]);
            let names = |v: &Vec<usize>| v.iter().map(|i| d.arenas[*i].area.name.clone()).collect::<Vec<_>>();
            for (a, gate) in d.ambushes.iter().zip(&g.ambushes) {
                println!("{pf} ambush {} <- {:?}", a.name, names(gate));
            }
            for (a, gate) in d.arenas.iter().zip(&g.arenas) {
                println!("{pf} arena {} <- {:?}", a.area.name, names(gate));
            }
        }
        assert!(GATES[&2300].ambushes.iter().all(|g| g.is_empty()));
    }
}
