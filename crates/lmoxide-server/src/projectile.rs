//! Projectiles: what many minifigure abilities shoot or send off (the Alien Avenger's lasers, the
//! Farmer's combine harvester, ...).
//!
//! The client's Projectile GameAction does nothing by itself: the server creates the projectile
//! dynel (type 51341) with UpdateMessage + `CoreProjectileFullUpdateIIR`, from the spell's
//! parameters (data/projectiles.json, tools/re/extract_projectiles.py). Every client then moves
//! it and plays its effects; the caster's client reports what it hit with `CoreProjectileIIR`
//! (u32 action, ID32 projectile, then 1 HitTarget: ID32 target + position; 2 HitPosition: two
//! positions; 6 DestroyOnPosition: position), and the server applies the damage and removes the
//! dynel (clients never remove it themselves).

use std::{
    collections::{HashMap, HashSet},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{ProtoGameCodeDynelIir, ProtoQuat, ProtoVec3},
    types::Id32,
};
use serde::Deserialize;
use tracing::info;

use crate::{
    game::{create_game_object, despawn_record},
    world,
};

const PROJECTILE_TYPE: u32 = 51341;
/// Range when the spell doesn't give one.
const DEFAULT_RANGE: f32 = 15.0;
/// Slow projectiles (vehicles like the combine harvester) plough through enemies; fast ones
/// (shots) stop at the first hit.
const PIERCING_BELOW_SPEED: f32 = 5.0;

#[derive(Debug, Clone, Deserialize)]
pub struct Params {
    pub item: u32,
    #[serde(rename = "type")]
    pub move_type: u32,
    pub speed: f32,
    pub attractor: u32,
    pub offset_z: f32,
    pub ricochets: u32,
    pub range: f32,
    /// Charge spells: the projectile item per charge level (hold longer, bigger shot).
    #[serde(default)]
    pub charge_items: Vec<u32>,
    /// Seconds per charge level.
    #[serde(default)]
    pub channel_interval: f32,
    /// Buff shown on the caster while charging (re-applied every interval).
    #[serde(default)]
    pub channel_buff: Option<u32>,
    /// Buff whose counter every channel tick raises by one (AddBuffCounter; the charge level).
    #[serde(default)]
    pub counter_buff: Option<u32>,
}

impl Params {
    /// The item for a charge held for `held` seconds.
    pub fn charged_item(&self, held: f32) -> u32 {
        let level = (held / self.channel_interval.max(0.05)) as usize;
        self.charge_items.get(level.min(self.charge_items.len().saturating_sub(1))).copied().unwrap_or(self.item)
    }
}

static PARAMS: LazyLock<HashMap<String, Params>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../data/projectiles.json")).expect("projectiles.json"));

struct Live {
    caster: Id32,
    spell: u32,
    damage: u32,
    piercing: bool,
    hit: HashSet<Id32>,
}

static LIVE: LazyLock<Mutex<HashMap<Id32, Live>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT: AtomicU32 = AtomicU32::new(1);

/// The projectile parameters of `spell`, if it shoots one.
pub fn params(spell: u32) -> Option<&'static Params> {
    PARAMS.get(&spell.to_string())
}

/// `caster` shoots `spell` from `from` towards `towards` (a target position, or a point ahead).
/// The projectile is shown to everyone on the caster's playfield; hits come back as
/// `CoreProjectileIIR` and deal `damage`.
#[allow(clippy::too_many_arguments)]
pub fn launch(
    caster: Id32,
    playfield: u32,
    spell: u32,
    item: Option<u32>,
    from: [f32; 3],
    towards: [f32; 3],
    target: Option<Id32>,
    damage: u32,
) {
    let Some(base) = params(spell) else { return };
    let p = &Params { item: item.unwrap_or(base.item), ..base.clone() };
    let id = Id32::new(PROJECTILE_TYPE, NEXT.fetch_add(1, Ordering::Relaxed));
    let range = if p.range > 0.0 { p.range } else { DEFAULT_RANGE };
    // aim at full range along the direction
    let (dx, dz) = (towards[0] - from[0], towards[2] - from[2]);
    let len = (dx * dx + dz * dz).sqrt().max(0.001);
    let end = [from[0] + dx / len * range, towards[1], from[2] + dz / len * range];
    let start = [from[0], from[1] + 1.0, from[2]];
    let records = vec![create_game_object(id, playfield, start), full_update(id, p, caster, target, spell, start, end, playfield)];
    LIVE.lock().unwrap().insert(id, Live { caster, spell, damage, piercing: p.speed < PIERCING_BELOW_SPEED, hit: HashSet::new() });
    info!(?caster, spell, item = p.item, "projectile");
    world::send_playfield(caster, true, world::Out::Records(records));
    // fallback removal once it has flown its range
    let lifetime = Duration::from_secs_f32(range / p.speed.max(0.1) + 2.0);
    tokio::spawn(async move {
        tokio::time::sleep(lifetime).await;
        remove(id, Duration::ZERO);
    });
}

/// A purely visual projectile (e.g. a pet's shot; the damage is dealt separately): flies from
/// `from` to `to` and is removed when it arrives.
pub fn launch_visual(caster: Id32, playfield: u32, item: u32, speed: f32, from: [f32; 3], to: [f32; 3]) {
    let p = Params {
        item,
        move_type: 1,
        speed,
        attractor: 0,
        offset_z: 0.0,
        ricochets: 0,
        range: 0.0,
        charge_items: vec![],
        channel_interval: 0.0,
        channel_buff: None,
        counter_buff: None,
    };
    let id = Id32::new(PROJECTILE_TYPE, NEXT.fetch_add(1, Ordering::Relaxed));
    let start = [from[0], from[1] + 1.0, from[2]];
    let records = vec![create_game_object(id, playfield, start), full_update(id, &p, caster, None, 0, start, to, playfield)];
    world::send_playfield(caster, true, world::Out::Records(records));
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    let flight = Duration::from_secs_f32((dx * dx + dz * dz).sqrt() / speed.max(0.1) + 0.5);
    tokio::spawn(async move {
        tokio::time::sleep(flight).await;
        world::send_playfield(caster, true, world::Out::Records(vec![despawn_record(id)]));
    });
}

/// `CoreProjectileFullUpdateIIR` (ReadSubClass 0x1244a60): header ProtoGameCodeDynelIIR, then
/// u32 item, u32 move type (1 straight, 2 ballistic, 3 orbiting), ID32 caster, ID32 target,
/// start, target position, f32 (5.0), f32 speed (m/s), u32 spell, start angle, local offset
/// (0, 0, forward), u32 attractor (0, or the caster attachment point it starts from),
/// u32 ricochets, u32 pierce count, i32 max distance (0: the spell's range).
#[allow(clippy::too_many_arguments)]
fn full_update(id: Id32, p: &Params, caster: Id32, target: Option<Id32>, spell: u32, start: [f32; 3], end: [f32; 3], playfield: u32) -> Bytes {
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(start[0]), y: Some(start[1]), z: Some(start[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield),
        running_effects: vec![],
    };
    iir::record("CoreProjectileFullUpdateIIR", id, false, |w| {
        let w = w.proto(&dynel).u32(p.item).u32(p.move_type);
        let w = target.unwrap_or_default().write(caster.write(w));
        let w = [start, end].iter().flatten().fold(w, |w, v| w.f32(*v));
        let w = w.f32(5.0).f32(p.speed).u32(spell);
        let w = [0.0, 0.0, 0.0, 0.0, 0.0, p.offset_z].iter().fold(w, |w, v| w.f32(*v));
        w.u32(p.attractor).u32(p.ricochets).u32(0).i32(0)
    })
}

/// A `CoreProjectileIIR` from the caster's client (`blob` is the whole record, `rest` its body).
/// Relayed to the others; hits deal damage, the end of the flight removes the projectile.
pub fn report(from: Id32, blob: &[u8], rest: &[u8]) {
    let u = |i: usize| rest.get(i..i + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let (Some(action), Some(kind), Some(instance)) = (u(0), u(4), u(8)) else { return };
    let id = Id32::new(kind, instance);
    world::send_playfield(from, false, world::Out::Records(vec![Bytes::copy_from_slice(blob)]));
    match action {
        // HitTarget: ID32 target, position
        1 => {
            let (Some(tk), Some(ti)) = (u(12), u(16)) else { return };
            let target = Id32::new(tk, ti);
            let hit = {
                let mut live = LIVE.lock().unwrap();
                let Some(l) = live.get_mut(&id) else { return };
                (l.caster == from && l.hit.insert(target)).then_some((l.spell, l.damage, l.piercing))
            };
            if let Some((spell, damage, piercing)) = hit {
                crate::npc::hit(from, target, spell, damage);
                if !piercing {
                    remove(id, Duration::from_millis(300));
                }
            }
        }
        // HitPosition / DestroyOnPosition: the flight is over
        2 | 6 => remove(id, Duration::from_secs(1)),
        _ => {}
    }
}

/// Removes a projectile for everyone (after `delay`, so its destroy effect can play).
fn remove(id: Id32, delay: Duration) {
    let Some(l) = LIVE.lock().unwrap().remove(&id) else { return };
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        world::send_playfield(l.caster, true, world::Out::Records(vec![despawn_record(id)]));
    });
}
