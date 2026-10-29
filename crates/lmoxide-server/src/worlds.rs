//! Running copies of worlds (instance.rs) are spawned when their first player arrives: the
//! residents (npc.rs), and the fixtures (buildies, jump pads, scenery, portal doors, the puzzles'
//! mirrors, beams and statues), each with its own state. A main copy stays (asleep while empty,
//! npc.rs); an extra copy is dropped once it has been empty for a while (`drop_copy`).

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use tracing::info;

use crate::instance::Pf;

static SPAWNED: LazyLock<Mutex<HashSet<Pf>>> = LazyLock::new(Default::default);

/// An extra copy is dropped once nobody has been in it this long (a player who lost their
/// connection has this long to come back; docs/TUNABLES.md).
const EMPTY_DROP: Duration = Duration::from_secs(180);
const DROP_CHECK: Duration = Duration::from_secs(30);

/// Drops extra copies nobody has been in for `EMPTY_DROP` (main copies stay, asleep).
pub fn start() {
    tokio::spawn(async {
        let mut tick = tokio::time::interval(DROP_CHECK);
        let mut empty_since: HashMap<Pf, Instant> = HashMap::new();
        loop {
            tick.tick().await;
            let extras: Vec<Pf> = spawned().into_iter().filter(|pf| pf.copy() >= crate::instance::FIRST_EXTRA_COPY).collect();
            empty_since.retain(|pf, _| extras.contains(pf));
            for pf in extras {
                let occupied = crate::world::positions(pf).iter().any(|(id, _)| crate::npc::is_player(*id));
                if occupied {
                    empty_since.remove(&pf);
                } else if empty_since.entry(pf).or_insert_with(Instant::now).elapsed() >= EMPTY_DROP {
                    empty_since.remove(&pf);
                    drop_copy(pf);
                }
            }
        }
    });
}

/// Spawns copy `pf` unless it is already there.
pub fn ensure(pf: Pf) {
    if !SPAWNED.lock().unwrap().insert(pf) {
        return;
    }
    let started = Instant::now();
    crate::npc::spawn_world(pf);
    crate::buildie::spawn_world(pf);
    crate::jumppad::spawn_world(pf);
    crate::scenery::spawn_world(pf);
    crate::teleport::spawn_world(pf);
    crate::gates::spawn_world(pf);
    crate::mirrors::spawn_world(pf);
    crate::keygate::spawn_world(pf);
    crate::tiki::spawn_world(pf);
    crate::lost_creations::spawn_world(pf);
    crate::lost_creations::spawn_entry(pf);
    info!(playfield = ?pf, mode = crate::instance::mode_of(pf).id(), ms = started.elapsed().as_millis() as u64, "world copy spawned");
}

/// Removes copy `pf` and everything in it (its players have left).
pub fn drop_copy(pf: Pf) {
    if !SPAWNED.lock().unwrap().remove(&pf) {
        return;
    }
    crate::npc::despawn_world(pf);
    crate::buildie::despawn_world(pf);
    crate::jumppad::despawn_world(pf);
    crate::scenery::despawn_world(pf);
    crate::teleport::despawn_world(pf);
    crate::gates::despawn_world(pf);
    crate::mirrors::despawn_world(pf);
    crate::keygate::despawn_world(pf);
    crate::tiki::despawn_world(pf);
    crate::lost_creations::despawn_world(pf);
    crate::lost_creations::despawn_entry(pf);
    crate::encounter::forget(pf);
    crate::scripts::playfield_emptied(pf);
    crate::preview::end(pf);
    crate::instance::forget_copy(pf);
    info!(playfield = ?pf, "world copy dropped");
}

/// Whether copy `pf` is spawned.
pub fn is_spawned(pf: Pf) -> bool {
    SPAWNED.lock().unwrap().contains(&pf)
}

/// Every spawned copy.
pub fn spawned() -> Vec<Pf> {
    let mut v: Vec<Pf> = SPAWNED.lock().unwrap().iter().copied().collect();
    v.sort();
    v
}

/// The spawned copies of playfield `model` (puzzles tick in each).
pub fn copies_of(model: u32) -> Vec<Pf> {
    let mut v: Vec<Pf> = SPAWNED.lock().unwrap().iter().copied().filter(|pf| pf.model() == model).collect();
    v.sort();
    v
}

/// The element of copy `pf` (stat ElementalType 3000109 and PlayfieldProxyData `elemental_type`:
/// the minimap's ring): a Lost Creations level's run element, else its world's (content
/// `element`; 0 none).
pub fn element(pf: Pf) -> u32 {
    crate::lost_creations::element(pf).unwrap_or_else(|| element_of(pf.model()))
}

/// The element of playfield `model` (content `element`; 0 none).
pub fn element_of(model: u32) -> u32 {
    crate::gamedata::get().world_by_playfield(model).and_then(|w| w.element).map_or(0, |e| e.id())
}
