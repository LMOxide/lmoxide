//! Dashes (the Samurai's Hurricane Step, the Roman Commander's For Rome!, ...): spells with the
//! server-only ColumnDash game action. The client never moves by itself; the server moves the
//! caster along a straight path, hits the enemies on the way and around the end point.
//!
//! Records as the original server sent them for For Rome! (recorded session, 5 casts): after
//! the command's success, a `MoveTaskIIR_c` list task (a fast path to the end, then a 0.1 s
//! wait), BCC enter substate 200 ("Charging", blocks other commands), the dash's effect package
//! at its "charge" hook; at the end BCC exit substate 200, the package's "execute" hook and a
//! `StopIIR_c` at the end point. Spell data: data/dashes.json (tools/re/extract_dashes.py).

use std::{collections::HashMap, sync::LazyLock, time::Duration};

use bytes::Bytes;
use lmoxide_net::{iir, types::Id32};
use serde::Deserialize;
use tracing::info;

use crate::{effects, world};

#[derive(Debug, Clone, Deserialize)]
pub struct Dash {
    distance: f32,
    radius: f32,
    backwards: bool,
    effect: u32,
    path_mod: Option<f32>,
    arrival_mod: Option<f32>,
    arrival_radius: Option<f32>,
    /// What the enemies hit along the path / around the end show: (nano, EffectPackage,
    /// TimeExist cs: a buff) as the original server sent them (For Rome!: its hit 465746).
    #[serde(default)]
    path_visuals: Vec<(u32, u32, Option<u32>)>,
    #[serde(default)]
    arrival_visuals: Vec<(u32, u32, Option<u32>)>,
}

static DASHES: LazyLock<HashMap<String, Dash>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../data/dashes.json")).expect("dashes.json"));

/// Dash speed: the player's MovementChargeSpeed stat (1400), 37.5 in the recorded session (see
/// game::PLAYER_SPEEDS); the path's speed-up mode 2 makes the client use it.
const SPEED: f32 = 37.5;
/// "Charging" behaviour substate, held for the dash.
const SUBSTATE_CHARGING: u32 = 200;
const BCC_ENTER_SUBSTATE: u8 = 11;
const BCC_EXIT_SUBSTATE: u8 = 12;
/// The wait after the path, and when the end records came (recorded: ≈ T + 0.1 s).
const SETTLE: f32 = 0.1;

pub fn params(spell: u32) -> Option<&'static Dash> {
    DASHES.get(&spell.to_string())
}

impl Dash {
    /// A ColumnDash from a spell's action list (actions.rs): `distance` along the facing with the
    /// effect package `effect`, no hits.
    pub fn column(distance: f32, effect: u32) -> Self {
        Dash {
            distance,
            radius: 1.0,
            backwards: false,
            effect,
            path_mod: None,
            arrival_mod: None,
            arrival_radius: None,
            path_visuals: Vec::new(),
            arrival_visuals: Vec::new(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// `caster` dashes with `spell` towards `point` (the clicked point; `None` or on the caster:
/// along `facing`). `damage(ability_mod)` gives a hit's damage.
pub fn start(
    caster: Id32,
    playfield: u32,
    spell: u32,
    dash: Dash,
    pos: world::Position,
    point: Option<[f32; 3]>,
    facing: [f32; 2],
    damage: impl Fn(f32) -> u32 + Send + 'static,
) {
    let start = *pos.lock().unwrap();
    // direction and length: towards the point, whole metres, at most the spell's distance (as
    // in the recorded casts); with no point the full distance along the facing
    let towards = point.map(|p| [p[0] - start[0], p[2] - start[2]]).filter(|d| d[0].hypot(d[1]) > 0.5);
    let (dir, len) = match towards {
        Some(d) => {
            let l = d[0].hypot(d[1]);
            ([d[0] / l, d[1] / l], l.min(dash.distance).floor())
        }
        None => (facing, dash.distance),
    };
    let dir = if dash.backwards { [-dir[0], -dir[1]] } else { dir };
    let end = [start[0] + dir[0] * len, start[1], start[2] + dir[1] * len];
    let t = len / SPEED;
    let instance = effects::new_instance();
    info!(?caster, spell, len, ?end, "dash");

    let mut records = vec![
        path_record(caster, start, end, len),
        substate_record(caster, BCC_ENTER_SUBSTATE),
    ];
    if dash.effect != 0 {
        records.push(effects::effect_package_hook(caster, dash.effect, "charge", "", (t - 0.033).max(0.01), 1, instance));
    }
    world::send_playfield(caster, true, world::Out::Records(records));
    if let Some(m) = dash.path_mod {
        crate::npc::hit_column_with(caster, playfield, start, end, dash.radius, spell, damage(m), |t| show_hit(caster, t, &dash.path_visuals));
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs_f32(t + SETTLE)).await;
        *pos.lock().unwrap() = end;
        let mut records = vec![substate_record(caster, BCC_EXIT_SUBSTATE)];
        if dash.effect != 0 {
            records.push(effects::effect_package_hook(caster, dash.effect, "execute", "charge", 0.0, 1, instance));
        }
        records.push(stop_record(caster, end, dir));
        world::send_playfield(caster, true, world::Out::Records(records));
        if let (Some(m), Some(r)) = (dash.arrival_mod, dash.arrival_radius) {
            crate::npc::hit_area_with(caster, playfield, end, r, spell, damage(m), |t| show_hit(caster, t, &dash.arrival_visuals));
        }
    });
}

/// A Blink game action (server-only; the Skydiver's Skydive): `caster` goes to `point`, at most
/// `max` metres away (without a point, the full distance along `facing`), on the same server
/// path as a dash (the client plays the jump itself, from the spell's animation), and `arrive`
/// runs at the landing point. The path's records are the dash's (no recording of a Blink).
pub fn leap(caster: Id32, pos: world::Position, point: Option<[f32; 3]>, facing: [f32; 2], max: f32, arrive: impl FnOnce([f32; 3]) + Send + 'static) {
    let start = *pos.lock().unwrap();
    let towards = point.map(|p| [p[0] - start[0], p[2] - start[2]]).filter(|d| d[0].hypot(d[1]) > 0.5);
    let (dir, len) = match towards {
        Some(d) => {
            let l = d[0].hypot(d[1]);
            ([d[0] / l, d[1] / l], l.min(max))
        }
        None => (facing, max),
    };
    let end = [start[0] + dir[0] * len, point.map_or(start[1], |p| p[1]), start[2] + dir[1] * len];
    let t = len / SPEED;
    info!(?caster, len, ?end, "leap");
    world::send_playfield(caster, true, world::Out::Records(vec![path_record(caster, start, end, len), substate_record(caster, BCC_ENTER_SUBSTATE)]));
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs_f32(t + SETTLE)).await;
        *pos.lock().unwrap() = end;
        world::send_playfield(caster, true, world::Out::Records(vec![substate_record(caster, BCC_EXIT_SUBSTATE), stop_record(caster, end, dir)]));
        arrive(end);
    });
}

/// What an enemy hit by a dash shows: its nanos' buffs and effect packages (see `Dash`).
fn show_hit(caster: Id32, target: Id32, visuals: &[(u32, u32, Option<u32>)]) {
    let mut records = Vec::new();
    for &(nano, package, time_exist) in visuals {
        match time_exist {
            Some(cs) => records.extend(effects::show_buff(target, nano, caster, Some(Duration::from_millis(10 * cs as u64)))),
            None => records.push(effects::effect_package(target, package)),
        }
    }
    if !records.is_empty() {
        world::send_playfield(target, true, world::Out::Records(records));
    }
}

/// `MoveTaskIIR_c`: u8 0, start; ListMoveTask (4): u8 wait flags 1, u8 current 0, u8 count 2;
/// PathMoveTask (1): u8 1, u16 path flags 4 (speed-up mode 2: MovementChargeSpeed), u32 2,
/// start, end, spline block (u8 0, f32 0, u8 0, f32 0, f32 len/35, fitted to the recorded
/// casts); WaitMoveTask (0): u8 0, f32 0.1, f32 0.1.
fn path_record(id: Id32, start: [f32; 3], end: [f32; 3], len: f32) -> Bytes {
    iir::record("MoveTaskIIR_c", id, false, |w| {
        let w = w.u8(0).f32(start[0]).f32(start[1]).f32(start[2]).u8(4).u8(1).u8(0).u8(2);
        let w = w.u8(1).u8(1).u16(4).u32(2);
        let w = [start, end].iter().flatten().fold(w, |w, v| w.f32(*v));
        let w = w.u8(0).f32(0.0).u8(0).f32(0.0).f32(len / 35.0);
        w.u8(0).u8(0).f32(SETTLE).f32(SETTLE)
    })
}

/// BehaviorControlCenterIIR_c action 11/12 (enter/exit substate) with the substate.
fn substate_record(id: Id32, action: u8) -> Bytes {
    iir::record("BehaviorControlCenterIIR_c", id, false, |w| w.u8(action).u32(SUBSTATE_CHARGING))
}

/// `StopIIR_c`: position, rotation (yaw towards `dir`), u8 1, u8 0 (as after the recorded dashes).
/// Also turns a standing player (actions.rs: towards their shots).
pub(crate) fn stop_record(id: Id32, at: [f32; 3], dir: [f32; 2]) -> Bytes {
    let half = dir[0].atan2(dir[1]) / 2.0;
    iir::record("StopIIR_c", id, false, |w| {
        w.f32(at[0]).f32(at[1]).f32(at[2]).f32(0.0).f32(half.sin()).f32(0.0).f32(half.cos()).u8(1).u8(0)
    })
}
