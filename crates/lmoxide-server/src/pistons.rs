//! The Gauntlet's piston room (6300, "Quest_DodgeRoom"): three rows of five wall traps
//! ("Piston <row>_<n>", look `evil_fort_wall_trap_4x4_medieval_interactable`) along three
//! corridors, each a step above the last. In turn they push out (the interactables' open motion,
//! mirrors.rs `TURN_PLUS`) and knock whoever stands in front of them ("BA_Piston<row>_<n>", 2.5 m
//! in front) off their ledge, down to the floor below, without damage (the user's memory of
//! live, 2026-10-25).
//!
//! Client data: the pistons and their areas; the piston "projectiles" XX_JumpPiston<row>_<n> run
//! the server script "JumpToPoint=AI_JumpPiston<row>_<n>;Height=1" (RDB 1000621 503086...),
//! whose points weren't shipped. The timing and the landing spots (the navmesh's floor below,
//! straight ahead) are inferred (docs/TUNABLES.md "Gauntlet").

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use lmoxide_net::types::Id32;
use serde::Deserialize;

use crate::{scenery::SIMPLE_DYNEL_TYPE, world};

pub const PLAYFIELD: u32 = 6300;
/// A piston pushes out for this long, every `PERIOD`; neighbours one `STAGGER` apart.
const PERIOD: Duration = Duration::from_secs(4);
const OUT: Duration = Duration::from_millis(1500);
const STAGGER: Duration = Duration::from_millis(700);
/// The push hits once the piston is out this far into its motion.
const HIT_AFTER: Duration = Duration::from_millis(300);
/// The hit area in front of a piston: half its width, from 0.5 to 4 m in front, 2.5 m up.
const HALF_WIDTH: f32 = 1.6;
const FRONT: (f32, f32) = (0.5, 4.0);
const HEIGHT: f32 = 2.5;
/// Where a hit lands, per row (the pistons push towards -z): z and height of the first lower
/// floor straight ahead (navmesh RDB 1000051: each corridor's walkway a metre below its ledge;
/// the user saw live drop you just one floor, 2026-10-25).
const LANDINGS: [(f32, f32); 3] = [(167.2, 46.8), (177.5, 51.8), (188.5, 56.8)];
const APEX: f32 = 1.0;

#[derive(Deserialize)]
struct Piece {
    name: String,
    pos: [f32; 3],
    guid: u32,
}

/// The pistons: (guid, row 1-3, column 1-5, position).
static PISTONS: LazyLock<Vec<(u32, usize, usize, [f32; 3])>> = LazyLock::new(|| {
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
    let pieces: Vec<Piece> = worlds
        .get(&PLAYFIELD.to_string())
        .and_then(|w| w.get("simple_dynels"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|d| serde_json::from_value(d.clone()).ok()).collect())
        .unwrap_or_default();
    pieces
        .iter()
        .filter_map(|p| {
            let (row, col) = p.name.strip_prefix("Piston ")?.split_once('_')?;
            Some((p.guid, row.parse().ok()?, col.parse().ok()?, p.pos))
        })
        .collect()
});

/// Pistons out now.
static OUT_NOW: LazyLock<Mutex<std::collections::HashSet<u32>>> = LazyLock::new(|| Mutex::new(Default::default()));

/// Whether scenery.rs spawns this piece (a piston).
pub fn is_piston(playfield: u32, name: &str) -> bool {
    playfield == PLAYFIELD && name.starts_with("Piston ")
}

/// A piston's pose for its spawn (out: the open motion).
pub fn pose_records(guid: u32) -> Vec<bytes::Bytes> {
    if OUT_NOW.lock().unwrap().contains(&guid) {
        vec![crate::effects::effect_package(Id32::new(SIMPLE_DYNEL_TYPE, guid), crate::mirrors::TURN_PLUS)]
    } else {
        Vec::new()
    }
}

/// Runs every piston's cycle (only while someone is in the Gauntlet).
pub fn start() {
    for &(guid, row, col, pos) in PISTONS.iter() {
        tokio::spawn(async move {
            // a wave along each row; the rows a little apart
            tokio::time::sleep(STAGGER * (col as u32 - 1) + STAGGER / 2 * (row as u32 - 1)).await;
            let mut tick = tokio::time::interval(PERIOD);
            loop {
                tick.tick().await;
                let players: Vec<(Id32, world::Position)> =
                    world::position_handles(PLAYFIELD).into_iter().filter(|(id, _)| crate::npc::is_player(*id)).collect();
                if players.is_empty() {
                    continue;
                }
                cycle(guid, row, pos, players).await;
            }
        });
    }
}

/// Out, knock whoever is in front off the corridor, back in.
async fn cycle(guid: u32, row: usize, pos: [f32; 3], players: Vec<(Id32, world::Position)>) {
    let id = Id32::new(SIMPLE_DYNEL_TYPE, guid);
    OUT_NOW.lock().unwrap().insert(guid);
    world::send_playfield(id, false, world::Out::Records(vec![crate::effects::effect_package(id, crate::mirrors::TURN_PLUS)]));
    tokio::time::sleep(HIT_AFTER).await;
    let (land_z, land_y) = LANDINGS[(row - 1).min(LANDINGS.len() - 1)];
    for (player, handle) in players {
        let at = *handle.lock().unwrap();
        let ahead = pos[2] - at[2];
        if (at[0] - pos[0]).abs() <= HALF_WIDTH && (FRONT.0..=FRONT.1).contains(&ahead) && (at[1] - pos[1]).abs() <= HEIGHT {
            tracing::info!(?player, piston = guid, "piston hit");
            let end = [at[0], land_y, land_z];
            tokio::spawn(async move { crate::jumppad::throw(player, &handle, end, APEX).await });
        }
    }
    tokio::time::sleep(OUT.saturating_sub(HIT_AFTER)).await;
    OUT_NOW.lock().unwrap().remove(&guid);
    world::send_playfield(id, false, world::Out::Records(vec![crate::effects::effect_package(id, crate::mirrors::TURN_MINUS)]));
}
