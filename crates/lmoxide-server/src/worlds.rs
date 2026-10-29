//! Running copies of worlds (instance.rs) are spawned when their first player arrives: the
//! residents (npc.rs), and the fixtures (buildies, jump pads, scenery, portal doors, the puzzles'
//! mirrors, beams and statues), each with its own state. A main copy stays (asleep while empty,
//! npc.rs); an extra copy is dropped once it has been empty for a while (`drop_copy`).

use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex},
    time::Instant,
};

use tracing::info;

use crate::instance::Pf;

static SPAWNED: LazyLock<Mutex<HashSet<Pf>>> = LazyLock::new(Default::default);

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
    crate::tiki::spawn_world(pf);
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
    crate::tiki::despawn_world(pf);
    crate::encounter::forget(pf);
    crate::scripts::playfield_emptied(pf);
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
