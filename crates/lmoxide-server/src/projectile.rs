//! Projectiles: what many minifigure abilities shoot or send off (the Alien Avenger's lasers, the
//! Farmer's combine harvester, ...).
//!
//! The client's Projectile GameAction does nothing by itself: the server creates the projectile
//! dynel (type 51341) with UpdateMessage + `CoreProjectileFullUpdateIIR`, from the spell's
//! parameters (data/projectiles.json, tools/re/extract_projectiles.py). Every client then moves
//! it and plays its effects; the caster's client reports what it hit with `CoreProjectileIIR`
//! (u32 action, ID32 projectile, then 1 HitTarget: ID32 target + position; 2 HitPosition: two
//! positions; 6 DestroyOnPosition: position), and the server applies the damage (or runs the
//! projectile item's actions there, `launch_shot`) and removes the dynel (clients never remove it
//! themselves). Ballistic (lobbed) shots land on the point they were thrown at and act there once,
//! at the first report or, failing one, when their flight is over.

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

/// What a projectile does to what it hits: plain damage, or a callback with the target hit (if
/// any) and the position (the projectile item's actions, actions.rs).
pub type OnHit = std::sync::Arc<dyn Fn(Option<Id32>, [f32; 3]) + Send + Sync>;

enum Effect {
    Damage(u32),
    Run(OnHit),
}

struct Live {
    caster: Id32,
    spell: u32,
    effect: Effect,
    piercing: bool,
    /// Lobbed: acts once where it lands (`end`).
    ballistic: bool,
    end: [f32; 3],
    hit: HashSet<Id32>,
}

/// One shot of a Projectile game action (actions.rs): its item, ProjectileType (1 straight, 2
/// ballistic), Speed (m/s), AttractorID, MaxRicochets, AttackRange and MinimumAttackRange (m,
/// 0: none / the default range).
pub struct Shot {
    pub item: u32,
    pub move_type: u32,
    pub speed: f32,
    pub attractor: u32,
    pub ricochets: u32,
    pub range: f32,
    pub min_range: f32,
}

const BALLISTIC: u32 = 2;

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
    let p = Params { item: item.unwrap_or(base.item), ..base.clone() };
    send(caster, playfield, spell, p, 0.0, from, towards, target, Effect::Damage(damage));
}

/// `caster` shoots `shot` (a Projectile action of `spell`) from `from` towards `towards`, at
/// `target` when it has one; `on_hit` runs for what it hits (straight shots) or where it lands
/// (ballistic ones).
#[allow(clippy::too_many_arguments)]
pub fn launch_shot(caster: Id32, playfield: u32, spell: u32, shot: &Shot, from: [f32; 3], towards: [f32; 3], target: Option<Id32>, on_hit: OnHit) {
    let p = Params {
        item: shot.item,
        move_type: shot.move_type,
        speed: shot.speed,
        attractor: shot.attractor,
        offset_z: 0.0,
        ricochets: shot.ricochets,
        range: shot.range,
        charge_items: vec![],
        channel_interval: 0.0,
        channel_buff: None,
        counter_buff: None,
    };
    send(caster, playfield, spell, p, shot.min_range, from, towards, target, Effect::Run(on_hit));
}

#[allow(clippy::too_many_arguments)]
fn send(caster: Id32, playfield: u32, spell: u32, p: Params, min_range: f32, from: [f32; 3], towards: [f32; 3], target: Option<Id32>, effect: Effect) {
    let id = Id32::new(PROJECTILE_TYPE, NEXT.fetch_add(1, Ordering::Relaxed));
    let range = if p.range > 0.0 { p.range } else { DEFAULT_RANGE };
    let (dx, dz) = (towards[0] - from[0], towards[2] - from[2]);
    let len = (dx * dx + dz * dz).sqrt().max(0.001);
    let ballistic = p.move_type == BALLISTIC;
    // a straight shot flies its full range along the direction; a lobbed one lands on the point
    // it was thrown at (between its minimum range and its range)
    let reach = if ballistic { len.clamp(min_range.min(range), range) } else { range };
    let end = [from[0] + dx / len * reach, towards[1], from[2] + dz / len * reach];
    let start = [from[0], from[1] + 1.0, from[2]];
    let records = vec![create_game_object(id, playfield, start), full_update(id, &p, caster, target, spell, start, end, playfield)];
    let piercing = p.speed < PIERCING_BELOW_SPEED;
    LIVE.lock().unwrap().insert(id, Live { caster, spell, effect, piercing, ballistic, end, hit: HashSet::new() });
    info!(?caster, spell, item = p.item, ballistic, "projectile");
    world::send_playfield(caster, true, world::Out::Records(records));
    // fallback removal once it has flown its range (a lobbed shot lands then)
    let lifetime = if ballistic {
        Duration::from_secs_f32(reach / p.speed.max(0.1) + LANDING_GRACE)
    } else {
        Duration::from_secs_f32(range / p.speed.max(0.1) + 2.0)
    };
    tokio::spawn(async move {
        tokio::time::sleep(lifetime).await;
        land(id, None, None);
    });
}

/// A lobbed shot that no report ended by then lands at its end point.
const LANDING_GRACE: f32 = 0.3;

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

/// An enemy's shot (npc_spells.rs): projectile `item` of `spell` flying from `from` to `to` at
/// `speed` (move type 1 straight, 2 ballistic), at `target` when it has one, as the original
/// server sent the recorded Scallywags' and Musketeers' shots (UpdateMessage +
/// `CoreProjectileFullUpdateIIR` with the enemy as caster and the player as target). The server
/// decides the hit itself (`enemy_impact`). Returns the projectile and its flight time.
#[allow(clippy::too_many_arguments)]
pub fn launch_enemy(caster: Id32, playfield: u32, spell: u32, item: u32, move_type: u32, speed: f32, attractor: u32, from: [f32; 3], to: [f32; 3], target: Option<Id32>) -> (Id32, Duration) {
    let p = Params {
        item,
        move_type,
        speed,
        attractor,
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
    let end = [to[0], to[1] + 1.0, to[2]];
    let records = vec![create_game_object(id, playfield, start), full_update(id, &p, caster, target, spell, start, end, playfield)];
    world::send_playfield(caster, true, world::Out::Records(records));
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    (id, Duration::from_secs_f32((dx * dx + dz * dz).sqrt() / speed.max(0.1)))
}

/// An enemy's projectile `id` arrived: `CoreProjectileIIR` HitTarget (u32 1, ID32 projectile,
/// ID32 target, position; identity the caster, as recorded) when it hit someone, then it is
/// removed.
pub fn enemy_impact(caster: Id32, id: Id32, hit: Option<(Id32, [f32; 3])>) {
    if let Some((target, at)) = hit {
        let record = iir::record("CoreProjectileIIR", caster, false, |w| {
            let w = target.write(id.write(w.u32(1)));
            w.f32(at[0]).f32(at[1] + 1.0).f32(at[2])
        });
        world::send_playfield(caster, true, world::Out::Records(vec![record]));
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
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
            let at = [20, 24, 28].map(|i| rest.get(i..i + 4).map_or(0.0, |b| f32::from_be_bytes(b.try_into().unwrap())));
            let hit = {
                let mut live = LIVE.lock().unwrap();
                let Some(l) = live.get_mut(&id) else { return };
                if l.caster != from {
                    return;
                }
                if l.ballistic {
                    None
                } else {
                    l.hit.insert(target).then(|| (l.spell, l.piercing, match &l.effect {
                        Effect::Damage(d) => Effect::Damage(*d),
                        Effect::Run(f) => Effect::Run(f.clone()),
                    }))
                }
            };
            match hit {
                Some((spell, piercing, effect)) => {
                    match effect {
                        Effect::Damage(damage) => {
                            crate::npc::hit(from, target, spell, damage);
                        }
                        Effect::Run(f) => f(Some(target), crate::npc::position(target).unwrap_or(at)),
                    }
                    if !piercing {
                        remove(id, Duration::from_millis(300));
                    }
                }
                // a lobbed shot that came down on someone
                None => land(id, Some(target), Some(crate::npc::position(target).unwrap_or(at))),
            }
        }
        // HitPosition / DestroyOnPosition: the flight is over (a lobbed shot acts there)
        2 | 6 => {
            let at = [12, 16, 20].map(|i| rest.get(i..i + 4).map(|b| f32::from_be_bytes(b.try_into().unwrap())));
            let at = (at.iter().all(Option::is_some)).then(|| at.map(Option::unwrap_or_default));
            land(id, None, at);
        }
        _ => {}
    }
}

/// The flight of `id` is over: a lobbed shot with actions runs them where it came down (`at`, or
/// its end point), on `target` if it hit one; then it is removed.
fn land(id: Id32, target: Option<Id32>, at: Option<[f32; 3]>) {
    let Some(l) = LIVE.lock().unwrap().remove(&id) else { return };
    if l.ballistic
        && let Effect::Run(f) = &l.effect
    {
        f(target, at.unwrap_or(l.end));
    }
    let delay = if l.ballistic || at.is_some() { Duration::from_secs(1) } else { Duration::ZERO };
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        world::send_playfield(l.caster, true, world::Out::Records(vec![despawn_record(id)]));
    });
}

/// Removes a projectile for everyone (after `delay`, so its destroy effect can play).
fn remove(id: Id32, delay: Duration) {
    let Some(l) = LIVE.lock().unwrap().remove(&id) else { return };
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        world::send_playfield(l.caster, true, world::Out::Records(vec![despawn_record(id)]));
    });
}
