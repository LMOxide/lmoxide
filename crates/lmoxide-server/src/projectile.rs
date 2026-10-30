//! Projectiles: what many minifigure abilities shoot or send off (the Alien Avenger's lasers, the
//! Farmer's combine harvester, ...).
//!
//! The client's Projectile GameAction does nothing by itself: the server creates the projectile
//! dynel (type 51341) with UpdateMessage + `CoreProjectileFullUpdateIIR`, from the spell's
//! parameters (content/skills `projectile`, skills.rs). Every client then moves
//! it and plays its effects; the caster's client reports what it hit with `CoreProjectileIIR`
//! (u32 action, ID32 projectile, then 1 HitTarget: ID32 target + position; 2 HitPosition: two
//! positions; 6 DestroyOnPosition: position), and clients never remove it themselves.
//!
//! Straight shots, the players' and the enemies', fly on the server too (`fly`), as the client
//! moves them (ProjectileStraightLineMoveTask: from the start towards the end point at the
//! speed, until the maximum distance; the target is only used when there is no end point): the
//! first enemy (players' shots) or player (enemies' shots) whose centre comes within the shot's
//! radius (content/items `radius`) plus the character's is hit (TryHitDynels). So a shot
//! aimed where its target was misses if the target stepped sideways out of its line, or was out
//! of range, but not if it ran towards it or away along it. The caster's reports only end a
//! player's shot early (it hit a wall); the server's hit is the one that counts.
//!
//! Ballistic (lobbed) shots land on the point they were thrown at and act there once, at the
//! first report or, failing one, when their flight is over.
//!
//! Orbiting shots (ProjectileType 3: the Fortune Teller's cards, the Sad Clown's clouds) circle
//! the full update's target dynel, wherever it goes, as the client moves them
//! (ProjectileOrbitingMoveTask: the offset, plus the target's and the projectile's radii ahead,
//! turned around the vertical by the start angle plus Speed × 100 degrees a second). The server
//! moves them the same way and hits what they touch (`circle`); they last until a hit or for
//! their lifetime, and the server removes them.

use crate::instance::Pf;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU8, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{ProtoGameCodeDynelIir, ProtoQuat, ProtoVec3},
    types::Id32,
};
use serde::Deserialize;
use tracing::{debug, info};

use crate::{
    game::{create_game_object, despawn_record},
    world,
};

const PROJECTILE_TYPE: u32 = 51341;
/// Range when neither the action nor the spell gives one.
pub const DEFAULT_RANGE: f32 = 15.0;
/// Slow projectiles (vehicles like the combine harvester) plough through enemies; fast ones
/// (shots) stop at the first hit.
const PIERCING_BELOW_SPEED: f32 = 5.0;
/// Hit radius of a projectile item without one (CoreProjectile::InitFromItem's minimum).
const MIN_RADIUS: f32 = 0.5;
/// Hit radius of a character: CharRadius 50 (game::spawn_records) / 100.
const TARGET_RADIUS: f32 = 0.5;
/// How often the server moves its straight shots and looks for what they hit.
const STEP: Duration = Duration::from_millis(50);
/// A shot that hit something is removed after this (its destroy effect plays).
const HIT_GRACE: Duration = Duration::from_millis(300);
/// A shot that flew its range is removed after this (the clients started it a little later).
const END_GRACE: Duration = Duration::from_millis(300);
/// A HitPosition this close to the last bounce point (m) is the bounce's own.
const BOUNCE_SAME_POINT: f32 = 1.0;
/// A straight shot's client-reported hit counts when the target is this close to where the
/// client saw it (m): the client draws moving enemies a little off from where the server has
/// them, and its shot hits what it draws.
const CLIENT_HIT_SLACK: f32 = 3.0;
/// How long a shot with ricochets left waits after a hit with nothing to chain to, for the
/// caster's client to throw it on (`CoreProjectile::RicochetTarget` turns it a random 15-75°
/// left or right then, and its later hits are reported against it).
const REDIRECT_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Deserialize)]
pub struct Params {
    pub item: u32,
    #[serde(rename = "type")]
    pub move_type: u32,
    pub speed: f32,
    pub attractor: u32,
    pub offset_z: f32,
    pub ricochets: u32,
    /// Enemies it flies on to after hitting one (MaxChains: the Hunting Axe).
    #[serde(default)]
    pub chains: u32,
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

static PARAMS: LazyLock<HashMap<u32, Params>> =
    LazyLock::new(|| crate::skills::projectiles().into_iter().map(|(k, v)| (k, serde_json::from_value(v).expect("projectile"))).collect());

/// Hit radius (m) by projectile item.
static RADII: LazyLock<HashMap<u32, f32>> = LazyLock::new(crate::skills::projectile_radii);

/// The hit radius of projectile `item` (m).
pub fn radius_of(item: u32) -> f32 {
    RADII.get(&item).copied().unwrap_or(MIN_RADIUS)
}

/// How far a shot of `spell` flies: the Projectile action's AttackRange (`action`, 0: none),
/// else the skill's range (its AttackRange), else `DEFAULT_RANGE`.
fn range_of(spell: u32, action: f32) -> f32 {
    if action > 0.0 {
        return action;
    }
    crate::skills::range(spell).unwrap_or(DEFAULT_RANGE)
}

/// How players' shots are aimed at the enemy they target: straight shots and lobbed ones each
/// have their own (`--shot-aim`, `--lob-aim`, `!aim`). Enemies always shoot at where the player is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Aim {
    /// At where the enemy is when the shot leaves: an enemy that moves on sideways (a straight
    /// shot), or at all (a lobbed one, which lands where it was thrown), may escape it.
    Current,
    /// At where the enemy will be when the shot gets there, if it keeps on along its path.
    Lead,
    /// The shot follows the enemy (the server's flight; the client still draws it straight).
    Homing,
}

impl Aim {
    pub const ALL: [Aim; 3] = [Aim::Current, Aim::Lead, Aim::Homing];

    pub fn name(self) -> &'static str {
        match self {
            Aim::Current => "current",
            Aim::Lead => "lead",
            Aim::Homing => "homing",
        }
    }

    /// `Homing` isn't implemented yet.
    pub fn implemented(self) -> bool {
        self != Aim::Homing
    }
}

/// Which shots an aim applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shots {
    Straight,
    Lobbed,
}

static PLAYER_AIM: AtomicU8 = AtomicU8::new(0);
static LOB_AIM: AtomicU8 = AtomicU8::new(1);

fn aim_setting(shots: Shots) -> &'static AtomicU8 {
    match shots {
        Shots::Straight => &PLAYER_AIM,
        Shots::Lobbed => &LOB_AIM,
    }
}

/// How players' `shots` are aimed now.
pub fn player_aim(shots: Shots) -> Aim {
    Aim::ALL[aim_setting(shots).load(Ordering::Relaxed) as usize]
}

/// Changes how players' `shots` are aimed; false (and no change) if `aim` isn't implemented yet.
pub fn set_player_aim(shots: Shots, aim: Aim) -> bool {
    if aim.implemented() {
        aim_setting(shots).store(Aim::ALL.iter().position(|a| *a == aim).unwrap_or(0) as u8, Ordering::Relaxed);
    }
    aim.implemented()
}

/// Where a player's shot from `from` at `speed` (m/s) at the enemy `target` is aimed
/// (`player_aim`), if it is alive: where it is, or (`Lead`) where it will be when the shot
/// gets there, following its path (`npc::position_after`; one that stands still stays put).
pub fn aim_at(target: Id32, from: [f32; 3], speed: f32, shots: Shots) -> Option<[f32; 3]> {
    let now = crate::npc::position(target)?;
    match player_aim(shots) {
        // (Homing is refused by `set_player_aim`)
        Aim::Current | Aim::Homing => Some(now),
        Aim::Lead => {
            // the flight time to where it will be then, a few times over (it converges fast
            // unless the enemy is about as fast as the shot)
            let mut at = now;
            for _ in 0..LEAD_ROUNDS {
                let t = (horizontal(from, at) / speed.max(0.1)).min(MAX_LEAD);
                at = crate::npc::position_after(target, Duration::from_secs_f32(t))?;
            }
            Some(at)
        }
    }
}

/// `aim_at` for a shot of `spell` with its own projectile parameters (content/skills `projectile`).
pub fn aim_for(spell: u32, target: Id32, from: [f32; 3]) -> Option<[f32; 3]> {
    let Some(p) = params(spell) else { return crate::npc::position(target) };
    aim_at(target, from, p.speed, if p.move_type == BALLISTIC { Shots::Lobbed } else { Shots::Straight })
}

/// Lead aim: how often the flight time is refined, and the most it looks ahead (s).
const LEAD_ROUNDS: usize = 4;
const MAX_LEAD: f32 = 3.0;

fn horizontal(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// What a projectile does to what it hits: plain damage, or a callback with the target hit (if
/// any) and the position (the projectile item's actions, actions.rs).
pub type OnHit = std::sync::Arc<dyn Fn(Option<Id32>, [f32; 3]) + Send + Sync>;

#[derive(Clone)]
enum Effect {
    Damage(u32),
    Run(OnHit),
}

/// Who a straight shot can hit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    /// a player's shot: enemies and smashables
    Npcs,
    /// an enemy's shot: players
    Players,
}

/// A straight shot as the server flies it: from `start` along `dir` (unit) at `speed` (m/s),
/// for `range` m, or up to `stop_at` (where the caster's client said it ended); it hits what
/// comes within `reach` of its centre.
#[derive(Clone, Copy)]
struct Flight {
    start: [f32; 3],
    dir: [f32; 3],
    speed: f32,
    range: f32,
    reach: f32,
    side: Side,
    stop_at: Option<f32>,
    /// When this leg started; a bounce or a chain starts a new one (`turn`).
    launched: Instant,
    leg: u32,
    /// Stopped at a hit until then, for the caster's client to throw it on (`hold`).
    waiting: Option<Instant>,
}

impl Flight {
    fn new(start: [f32; 3], end: [f32; 3], speed: f32, range: f32, reach: f32, side: Side) -> Self {
        let d = [0, 1, 2].map(|i| end[i] - start[i]);
        let len = d.iter().map(|v| v * v).sum::<f32>().sqrt().max(0.001);
        Flight { start, dir: d.map(|v| v / len), speed, range, reach, side, stop_at: None, launched: Instant::now(), leg: 0, waiting: None }
    }

    /// A new leg from `start` along `dir` (a ricochet off a wall, or a chain to the next enemy):
    /// it flies the full range again, as the client's projectile does.
    fn turn(&mut self, start: [f32; 3], dir: [f32; 3]) {
        let len = dir.iter().map(|v| v * v).sum::<f32>().sqrt();
        if len < 1e-3 {
            return;
        }
        *self = Flight { start, dir: dir.map(|v| v / len), stop_at: None, launched: Instant::now(), leg: self.leg + 1, waiting: None, ..*self };
    }

    /// The point `d` m along the flight.
    fn at(&self, d: f32) -> [f32; 3] {
        [0, 1, 2].map(|i| self.start[i] + self.dir[i] * d)
    }

    fn limit(&self) -> f32 {
        self.stop_at.map_or(self.range, |s| s.min(self.range))
    }
}

/// Whether the shot's centre, moving from `a` to `b`, comes within `reach` of a character at
/// `p` (horizontally; and within 2 × `reach` + 1 m of its height, as TryHitDynels allows):
/// how far along (0..1) it is closest.
fn crossing(a: [f32; 3], b: [f32; 3], p: [f32; 3], reach: f32) -> Option<f32> {
    let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
    let len2 = dx * dx + dz * dz;
    let t = if len2 < 1e-6 { 0.0 } else { (((p[0] - a[0]) * dx + (p[2] - a[2]) * dz) / len2).clamp(0.0, 1.0) };
    let c = [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t);
    let off = (p[0] - c[0]).hypot(p[2] - c[2]);
    (off < reach && (c[1] - p[1]).abs() < 2.0 * reach + 1.0).then_some(t)
}

/// An orbiting shot as the client moves it: around `centre` (followed), `offset` (m, with the
/// radii) turned `angle` + `rate` × t radians around the vertical; it hits what comes within
/// `reach`, until `until`.
#[derive(Clone, Copy)]
struct Orbit {
    centre: Id32,
    offset: [f32; 3],
    angle: f32,
    rate: f32,
    reach: f32,
    side: Side,
    launched: Instant,
    until: Instant,
}

impl Orbit {
    /// Where it is `t` s after the launch with the centre at `c` (the client's quaternion turn
    /// around +Y: x' = x cos a + z sin a, z' = z cos a - x sin a).
    fn at(&self, c: [f32; 3], t: f32) -> [f32; 3] {
        let (s, k) = (self.angle + self.rate * t).sin_cos();
        let [x, y, z] = self.offset;
        [c[0] + x * k + z * s, c[1] + y, c[2] + z * k - x * s]
    }
}

/// An orbiting shot's turn rate (rad/s) from its speed (m/s on the wire): the client's
/// OnStart takes speed × π / 1.8, i.e. the action's Speed in degrees a second.
fn orbit_rate(speed: f32) -> f32 {
    speed * std::f32::consts::PI / 1.8
}

struct Live {
    caster: Id32,
    spell: u32,
    effect: Effect,
    piercing: bool,
    /// Lobbed: acts once where it lands (`end`).
    ballistic: bool,
    end: [f32; 3],
    /// A straight shot's flight (`fly`).
    flight: Option<Flight>,
    /// An orbiting shot's circle (`circle`).
    orbit: Option<Orbit>,
    hit: HashSet<Id32>,
    /// Enemies it still flies on to after a hit (`strike`).
    chains: u32,
    /// Turns the caster's client still gives it (a wall bounce, a chain or a random throw after
    /// a hit: `CoreProjectile` +0x2ac).
    ricochets: u32,
    /// Where it last bounced off a wall (the caster's client's ricochet, `report`).
    bounced_at: Option<[f32; 3]>,
}

/// One shot of a Projectile game action (actions.rs): its item, ProjectileType (1 straight, 2
/// ballistic), Speed (m/s), AttractorID, MaxRicochets, MaxChains, AttackRange and MinimumAttackRange (m,
/// 0: none / the spell's range); `width` (m) replaces the item's hit radius plus the target's,
/// and `piercing` makes it hit everything on its way.
pub struct Shot {
    pub item: u32,
    pub move_type: u32,
    pub speed: f32,
    pub attractor: u32,
    pub ricochets: u32,
    pub chains: u32,
    pub range: f32,
    pub min_range: f32,
    pub width: Option<f32>,
    pub piercing: bool,
}

pub const BALLISTIC: u32 = 2;
pub const ORBITING: u32 = 3;
/// How long an orbiting shot circles when its spell has no TimeExist: live's Sad Clown clouds
/// (buffs of 500 cs on their anchor, despawned 5.2–5.3 s after they appeared).
pub const ORBIT_LIFETIME: Duration = Duration::from_secs(5);

/// One orbiting shot of a Projectile action (ProjectileType 3): `item` circling `centre` (a
/// character, followed) at `speed` (m/s on the wire: Speed / 100), starting `angle` degrees
/// around (RotY) at `offset` (m: DisplaceMentX/Y/Z; the client adds the centre's and the item's
/// radii ahead), for `lifetime`. `hits_players`: an enemy's (it hits players), else a player's.
pub struct Circle {
    pub item: u32,
    pub speed: f32,
    pub centre: Id32,
    pub angle: f32,
    pub offset: [f32; 3],
    pub lifetime: Duration,
    pub hits_players: bool,
}

/// `caster` sends an orbiting shot of `spell` (`circle`) on `playfield`; `on_hit` runs for what
/// it touches (it bursts there), and it is gone after its lifetime. Nothing is sent when the
/// centre isn't there.
pub fn launch_orbit(caster: Id32, playfield: Pf, spell: u32, circle: &Circle, on_hit: OnHit) {
    let Some(c) = centre_position(circle.centre, playfield) else { return };
    let p = Params {
        item: circle.item,
        move_type: ORBITING,
        speed: circle.speed,
        attractor: 0,
        offset_z: 0.0,
        ricochets: 0,
        chains: 0,
        range: 0.0,
        charge_items: vec![],
        channel_interval: 0.0,
        channel_buff: None,
        counter_buff: None,
    };
    let id = Id32::new(PROJECTILE_TYPE, NEXT.fetch_add(1, Ordering::Relaxed));
    let angle = circle.angle.to_radians();
    let ahead = TARGET_RADIUS + radius_of(circle.item);
    let launched = Instant::now();
    let orbit = Orbit {
        centre: circle.centre,
        offset: [circle.offset[0], circle.offset[1], circle.offset[2] + ahead],
        angle,
        rate: orbit_rate(circle.speed),
        reach: radius_of(circle.item) + TARGET_RADIUS,
        side: if circle.hits_players { Side::Players } else { Side::Npcs },
        launched,
        until: launched + circle.lifetime,
    };
    let start = orbit.at(c, 0.0);
    let records = vec![
        create_game_object(id, playfield, start),
        // (no end point: live sent zeros, the clients circle the target)
        full_update_turned(id, &p, caster, Some(circle.centre), spell, start, [0.0; 3], playfield, 0, [0.0, angle, 0.0], circle.offset),
    ];
    let live = Live {
        caster,
        spell,
        effect: Effect::Run(on_hit),
        piercing: false,
        ballistic: false,
        end: c,
        flight: None,
        orbit: Some(orbit),
        hit: HashSet::new(),
        chains: 0,
        ricochets: 0,
        bounced_at: None,
    };
    LIVE.lock().unwrap().insert(id, live);
    debug!(?caster, spell, item = circle.item, centre = ?circle.centre, "orbiting projectile");
    world::send_playfield(caster, true, world::Out::Records(records));
    circle_on(id, playfield);
}

/// Where the centre of an orbit is now, if it is still on `playfield`: an NPC along its walk, a
/// player where it is now.
fn centre_position(id: Id32, playfield: Pf) -> Option<[f32; 3]> {
    if crate::npc::is_npc(id) {
        return crate::npc::position(id);
    }
    world::whereabouts(id).filter(|(pf, _)| *pf == playfield).and_then(|_| world::live_position(id))
}

/// Moves orbiting shot `id` on the server as the clients do, every `STEP`, and hits what it
/// passed (`strike`: it bursts on the first), until its time is up or its centre is gone.
fn circle_on(id: Id32, playfield: Pf) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(STEP);
        let mut last: Option<[f32; 3]> = None;
        loop {
            tick.tick().await;
            let Some((o, caster, hit)) = LIVE.lock().unwrap().get(&id).and_then(|l| Some((l.orbit?, l.caster, l.hit.clone()))) else { return };
            let now = Instant::now();
            let Some(c) = centre_position(o.centre, playfield).filter(|_| now < o.until) else {
                remove(id, Duration::ZERO);
                return;
            };
            let here = o.at(c, now.duration_since(o.launched).as_secs_f32());
            let from = last.unwrap_or(here);
            last = Some(here);
            let first = candidates(o.side, playfield)
                .into_iter()
                .filter(|(t, _)| *t != caster && *t != o.centre && !hit.contains(t))
                .filter_map(|(t, p)| crossing(from, here, p, o.reach).map(|s| (s, t, p)))
                .min_by(|x, y| x.0.total_cmp(&y.0));
            if let Some((_, target, at)) = first
                && !strike(id, target, at)
            {
                return;
            }
        }
    });
}

static LIVE: LazyLock<Mutex<HashMap<Id32, Live>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT: AtomicU32 = AtomicU32::new(1);

/// The projectile parameters of `spell`, if it shoots one.
pub fn params(spell: u32) -> Option<&'static Params> {
    PARAMS.get(&spell)
}

/// `caster` shoots `spell` from `from` towards `towards` (a target position, or a point ahead).
/// The projectile is shown to everyone on the caster's playfield; what it hits takes `damage`.
#[allow(clippy::too_many_arguments)]
pub fn launch(
    caster: Id32,
    playfield: Pf,
    spell: u32,
    item: Option<u32>,
    from: [f32; 3],
    towards: [f32; 3],
    target: Option<Id32>,
    damage: u32,
) {
    let Some(base) = params(spell) else { return };
    let p = Params { item: item.unwrap_or(base.item), ..base.clone() };
    send(caster, playfield, spell, p, 0.0, None, false, from, towards, target, Effect::Damage(damage));
}

/// `caster` shoots `shot` (a Projectile action of `spell`) from `from` towards `towards`, at
/// `target` when it has one; `on_hit` runs for what it hits (straight shots) or where it lands
/// (ballistic ones).
#[allow(clippy::too_many_arguments)]
pub fn launch_shot(caster: Id32, playfield: Pf, spell: u32, shot: &Shot, from: [f32; 3], towards: [f32; 3], target: Option<Id32>, on_hit: OnHit) {
    let p = Params {
        item: shot.item,
        move_type: shot.move_type,
        speed: shot.speed,
        attractor: shot.attractor,
        offset_z: 0.0,
        ricochets: shot.ricochets,
        chains: shot.chains,
        range: shot.range,
        charge_items: vec![],
        channel_interval: 0.0,
        channel_buff: None,
        counter_buff: None,
    };
    send(caster, playfield, spell, p, shot.min_range, shot.width, shot.piercing, from, towards, target, Effect::Run(on_hit));
}

#[allow(clippy::too_many_arguments)]
fn send(
    caster: Id32,
    playfield: Pf,
    spell: u32,
    p: Params,
    min_range: f32,
    width: Option<f32>,
    piercing: bool,
    from: [f32; 3],
    towards: [f32; 3],
    target: Option<Id32>,
    effect: Effect,
) {
    let id = Id32::new(PROJECTILE_TYPE, NEXT.fetch_add(1, Ordering::Relaxed));
    let ballistic = p.move_type == BALLISTIC;
    // the clients fly a straight shot whole metres (the full update's maximum distance)
    let range = if ballistic { range_of(spell, p.range) } else { range_of(spell, p.range).ceil() };
    let (dx, dz) = (towards[0] - from[0], towards[2] - from[2]);
    let len = (dx * dx + dz * dz).sqrt().max(0.001);
    // a straight shot flies its full range along the direction; a lobbed one lands on the point
    // it was thrown at (between its minimum range and its range)
    let reach = if ballistic { len.clamp(min_range.min(range), range) } else { range };
    let end = [from[0] + dx / len * reach, towards[1], from[2] + dz / len * reach];
    let start = [from[0], from[1] + 1.0, from[2]];
    let max_distance = if ballistic { 0 } else { range as i32 };
    let records = vec![create_game_object(id, playfield, start), full_update(id, &p, caster, target, spell, start, end, playfield, max_distance)];
    let piercing = piercing || p.speed < PIERCING_BELOW_SPEED;
    let flight = (!ballistic).then(|| Flight::new(start, end, p.speed, range, width.unwrap_or(radius_of(p.item) + TARGET_RADIUS), Side::Npcs));
    let (chains, ricochets) = (p.chains, p.ricochets);
    LIVE.lock().unwrap().insert(id, Live { caster, spell, effect, piercing, ballistic, end, flight, orbit: None, hit: HashSet::new(), chains, ricochets, bounced_at: None });
    info!(?caster, spell, item = p.item, ballistic, range, "projectile");
    world::send_playfield(caster, true, world::Out::Records(records));
    if !ballistic {
        fly(id, playfield);
        return;
    }
    // a lobbed shot that no report ended lands then
    let lifetime = Duration::from_secs_f32(reach / p.speed.max(0.1) + LANDING_GRACE);
    tokio::spawn(async move {
        tokio::time::sleep(lifetime).await;
        land(id, None, None);
    });
}

/// A lobbed shot that no report ended by then lands at its end point.
const LANDING_GRACE: f32 = 0.3;

/// Flies straight shot `id` on the server: every `STEP` it moves on as far as its speed has
/// taken it since the launch, and hits what it passed (`strike`), until it stopped at a hit, or
/// flew its range.
fn fly(id: Id32, playfield: Pf) {
    tokio::spawn(async move {
        let mut leg = 0;
        let mut done = 0.0f32;
        let mut tick = tokio::time::interval(STEP);
        'step: loop {
            tick.tick().await;
            let Some((f, caster, hit)) = LIVE.lock().unwrap().get(&id).and_then(|l| Some((l.flight?, l.caster, l.hit.clone()))) else { return };
            if f.leg != leg {
                // it bounced or chained: the new leg from its start
                (leg, done) = (f.leg, 0.0);
            }
            let to = (f.speed * f.launched.elapsed().as_secs_f32()).min(f.limit());
            let (a, b) = (f.at(done), f.at(to.max(done)));
            let mut hits: Vec<(f32, Id32, [f32; 3])> = candidates(f.side, playfield)
                .into_iter()
                .filter(|(t, _)| *t != caster && !hit.contains(t))
                .filter_map(|(t, p)| crossing(a, b, p, f.reach).map(|s| (s, t, p)))
                .collect();
            hits.sort_by(|x, y| x.0.total_cmp(&y.0));
            for (_, target, at) in hits {
                if !strike(id, target, at) {
                    return;
                }
                if LIVE.lock().unwrap().get(&id).and_then(|l| l.flight).is_some_and(|f| f.leg != leg) {
                    continue 'step;
                }
            }
            done = done.max(to);
            if done >= f.limit() && !f.waiting.is_some_and(|t| Instant::now() < t) {
                remove(id, END_GRACE);
                return;
            }
        }
    });
}

/// What a shot from `side` can hit on `playfield`, where each is now.
fn candidates(side: Side, playfield: Pf) -> Vec<(Id32, [f32; 3])> {
    match side {
        Side::Npcs => crate::npc::targets_where(playfield, |_| true),
        Side::Players => world::live_positions(playfield).into_iter().filter(|(id, _)| crate::npc::is_player(*id)).collect(),
    }
}

/// Straight shot `id` hit `target` (at `at`): shown to the clients (a player's own client
/// already showed its shot's hit), then its effect. Returns whether the shot flies on.
fn strike(id: Id32, target: Id32, at: [f32; 3]) -> bool {
    let (caster, spell, effect, piercing, side) = {
        let mut live = LIVE.lock().unwrap();
        let Some(l) = live.get_mut(&id) else { return false };
        if !l.hit.insert(target) {
            return true;
        }
        let side = l.flight.map(|f| f.side).or(l.orbit.map(|o| o.side)).unwrap_or(Side::Npcs);
        (l.caster, l.spell, l.effect.clone(), l.piercing, side)
    };
    info!(?caster, ?target, spell, "projectile hit");
    world::send_playfield(caster, side == Side::Players, world::Out::Records(vec![hit_record(caster, id, target, at)]));
    match effect {
        Effect::Damage(damage) => {
            crate::npc::hit(caster, target, spell, damage);
        }
        Effect::Run(f) => f(Some(target), at),
    }
    if piercing || chain(id, at) || hold(id, at) {
        return true;
    }
    remove(id, HIT_GRACE);
    false
}

/// A player's shot with ricochets left that found nothing to chain to stops at `at` for a
/// while: the caster's client throws it on in a random direction (`report` turns the flight
/// then) and what it hits after counts. Returns whether it does.
fn hold(id: Id32, at: [f32; 3]) -> bool {
    let mut live = LIVE.lock().unwrap();
    let Some(l) = live.get_mut(&id).filter(|l| l.ricochets > 0) else { return false };
    let Some(f) = l.flight.as_mut().filter(|f| f.side == Side::Npcs) else { return false };
    let d = (0..3).map(|i| (at[i] - f.start[i]).powi(2)).sum::<f32>().sqrt();
    f.stop_at = Some(d.min(f.stop_at.unwrap_or(f32::MAX)));
    f.waiting = Some(Instant::now() + REDIRECT_WAIT);
    debug!(?id, "projectile waits for its ricochet");
    true
}

/// A shot with chains left (MaxChains: the Hunting Axe) flies on from `at` to an enemy or
/// smashable it hasn't hit within its range of the caster, as the client's
/// `CoreProjectile::RicochetTarget` does (the first such in its vicinity list; here the one
/// nearest `at`, and the client's own pick turns the flight when it reports it). Returns
/// whether it did.
fn chain(id: Id32, at: [f32; 3]) -> bool {
    let Some((caster, hit, f)) = LIVE.lock().unwrap().get(&id).filter(|l| l.chains > 0 && l.ricochets > 0).and_then(|l| Some((l.caster, l.hit.clone(), l.flight?))) else {
        return false;
    };
    let Some((playfield, from)) = world::whereabouts(caster) else { return false };
    let next = candidates(f.side, playfield)
        .into_iter()
        .filter(|(t, _)| *t != caster && !hit.contains(t) && (crate::npc::is_enemy(*t) || crate::npc::is_smashable(*t)))
        .filter(|(_, p)| horizontal(from, *p) <= f.range)
        .map(|(t, p)| (horizontal(at, p), t, p))
        .min_by(|x, y| x.0.total_cmp(&y.0));
    let Some((_, target, p)) = next else { return false };
    let mut live = LIVE.lock().unwrap();
    let Some(l) = live.get_mut(&id) else { return false };
    let Some(f) = l.flight.as_mut() else { return false };
    l.chains -= 1;
    l.ricochets -= 1;
    let start = [at[0], f.at(0.0)[1], at[2]];
    f.turn(start, [p[0] - at[0], 0.0, p[2] - at[2]]);
    debug!(?id, ?target, left = l.chains, "projectile chains");
    true
}

/// A purely visual projectile (e.g. a pet's shot; the damage is dealt separately): flies from
/// `from` to `to` and is removed when it arrives.
pub fn launch_visual(caster: Id32, playfield: Pf, item: u32, speed: f32, from: [f32; 3], to: [f32; 3]) {
    let p = Params {
        item,
        move_type: 1,
        speed,
        attractor: 0,
        offset_z: 0.0,
        ricochets: 0,
        chains: 0,
        range: 0.0,
        charge_items: vec![],
        channel_interval: 0.0,
        channel_buff: None,
        counter_buff: None,
    };
    let id = Id32::new(PROJECTILE_TYPE, NEXT.fetch_add(1, Ordering::Relaxed));
    let start = [from[0], from[1] + 1.0, from[2]];
    let records = vec![create_game_object(id, playfield, start), full_update(id, &p, caster, None, 0, start, to, playfield, 0)];
    world::send_playfield(caster, true, world::Out::Records(records));
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    let flight = Duration::from_secs_f32((dx * dx + dz * dz).sqrt() / speed.max(0.1) + 0.5);
    tokio::spawn(async move {
        tokio::time::sleep(flight).await;
        world::send_playfield(caster, true, world::Out::Records(vec![despawn_record(id)]));
    });
}

/// An enemy's straight shot (npc_spells.rs, tiki.rs): projectile `item` of `spell` (0: none)
/// flies from `from` towards `towards` for `range` m at `speed` (m/s); the first player it
/// touches is hit, shown as the original server showed the recorded Scallywags' and Musketeers'
/// hits (`CoreProjectileIIR` HitTarget from the enemy to everyone), and `on_hit` runs with the
/// player and where they were. The full update names `target` (the player aimed at, if any) as
/// the recorded shots did; the shot still flies straight.
#[allow(clippy::too_many_arguments)]
pub fn launch_at_players(
    caster: Id32,
    playfield: Pf,
    spell: u32,
    item: u32,
    speed: f32,
    attractor: u32,
    from: [f32; 3],
    towards: [f32; 3],
    range: f32,
    target: Option<Id32>,
    on_hit: OnHit,
) {
    let p = Params {
        item,
        move_type: 1,
        speed,
        attractor,
        offset_z: 0.0,
        ricochets: 0,
        chains: 0,
        range: 0.0,
        charge_items: vec![],
        channel_interval: 0.0,
        channel_buff: None,
        counter_buff: None,
    };
    let id = Id32::new(PROJECTILE_TYPE, NEXT.fetch_add(1, Ordering::Relaxed));
    let range = range.ceil();
    let (dx, dz) = (towards[0] - from[0], towards[2] - from[2]);
    let len = dx.hypot(dz);
    let (ux, uz) = if len > 0.001 { (dx / len, dz / len) } else { (0.0, 1.0) };
    let start = [from[0], from[1] + 1.0, from[2]];
    let end = [from[0] + ux * range, towards[1] + 1.0, from[2] + uz * range];
    let records = vec![create_game_object(id, playfield, start), full_update(id, &p, caster, target, spell, start, end, playfield, range as i32)];
    let flight = Flight::new(start, end, speed, range, radius_of(item) + TARGET_RADIUS, Side::Players);
    let live = Live {
        caster,
        spell,
        effect: Effect::Run(on_hit),
        piercing: speed < PIERCING_BELOW_SPEED,
        ballistic: false,
        end,
        flight: Some(flight),
        orbit: None,
        hit: HashSet::new(),
        chains: 0,
        ricochets: 0,
        bounced_at: None,
    };
    LIVE.lock().unwrap().insert(id, live);
    world::send_playfield(caster, true, world::Out::Records(records));
    fly(id, playfield);
}

/// An enemy's shot that the caller times itself (ballistic shots, npc_spells.rs; the ballista):
/// projectile `item` of `spell` flying from `from` to `to` at `speed` (move type 1 straight, 2
/// ballistic), at `target` when it has one. The caller decides the hit (`enemy_impact`).
/// Returns the projectile and its flight time.
#[allow(clippy::too_many_arguments)]
pub fn launch_enemy(caster: Id32, playfield: Pf, spell: u32, item: u32, move_type: u32, speed: f32, attractor: u32, from: [f32; 3], to: [f32; 3], target: Option<Id32>) -> (Id32, Duration) {
    let p = Params {
        item,
        move_type,
        speed,
        attractor,
        offset_z: 0.0,
        ricochets: 0,
        chains: 0,
        range: 0.0,
        charge_items: vec![],
        channel_interval: 0.0,
        channel_buff: None,
        counter_buff: None,
    };
    let id = Id32::new(PROJECTILE_TYPE, NEXT.fetch_add(1, Ordering::Relaxed));
    let start = [from[0], from[1] + 1.0, from[2]];
    let end = [to[0], to[1] + 1.0, to[2]];
    let records = vec![create_game_object(id, playfield, start), full_update(id, &p, caster, target, spell, start, end, playfield, 0)];
    world::send_playfield(caster, true, world::Out::Records(records));
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    (id, Duration::from_secs_f32((dx * dx + dz * dz).sqrt() / speed.max(0.1)))
}

/// An enemy's projectile `id` (from `launch_enemy`) arrived: the hit, when it hit someone, else
/// its end where it came down (`landed`: DestroyOnPosition, so the clients play its destroy
/// effect there, as a player's lobbed shot's own client reports it: Captain Dreadleg's bomb's
/// thud), then it is removed.
pub fn enemy_impact(caster: Id32, id: Id32, hit: Option<(Id32, [f32; 3])>, landed: Option<[f32; 3]>) {
    let record = match (hit, landed) {
        (Some((target, at)), _) => Some(hit_record(caster, id, target, at)),
        (None, Some(at)) => Some(destroy_record(caster, id, at)),
        (None, None) => None,
    };
    if let Some(record) = record {
        world::send_playfield(caster, true, world::Out::Records(vec![record]));
    }
    tokio::spawn(async move {
        tokio::time::sleep(HIT_GRACE).await;
        world::send_playfield(caster, true, world::Out::Records(vec![despawn_record(id)]));
    });
}

/// `CoreProjectileIIR` HitTarget: u32 1, ID32 projectile, ID32 target, position (chest height);
/// identity the caster, as recorded.
fn hit_record(caster: Id32, id: Id32, target: Id32, at: [f32; 3]) -> Bytes {
    iir::record("CoreProjectileIIR", caster, false, |w| {
        let w = target.write(id.write(w.u32(1)));
        w.f32(at[0]).f32(at[1] + 1.0).f32(at[2])
    })
}

/// `CoreProjectileIIR` DestroyOnPosition: u32 6, ID32 projectile, position; identity the caster.
fn destroy_record(caster: Id32, id: Id32, at: [f32; 3]) -> Bytes {
    iir::record("CoreProjectileIIR", caster, false, |w| id.write(w.u32(6)).f32(at[0]).f32(at[1]).f32(at[2]))
}

/// `CoreProjectileFullUpdateIIR` (ReadSubClass 0x1244a60): header ProtoGameCodeDynelIIR, then
/// u32 item, u32 move type (1 straight, 2 ballistic, 3 orbiting), ID32 caster, ID32 target,
/// start, target position, f32 (5.0), f32 speed (m/s), u32 spell, start angles (3 × f32,
/// radians around x, y, z; an orbiting shot's start is the y one), local offset (3 × f32, m; 0,
/// 0, forward for shots, an orbit's offset from its target), u32 attractor (0, or the caster
/// attachment point it starts from), u32 ricochets, u32 chains (CoreProjectile +0x2b0:
/// `RicochetTarget` to the next enemy), i32 max distance (m; 0: the spell's AttackRange,
/// CoreProjectileFullUpdateIIR::Transfer). The caster and target go to CoreProjectile +0x23c and
/// +0x244 (`GetTarget`: what an orbiting shot circles).
#[allow(clippy::too_many_arguments)]
fn full_update(id: Id32, p: &Params, caster: Id32, target: Option<Id32>, spell: u32, start: [f32; 3], end: [f32; 3], playfield: Pf, max_distance: i32) -> Bytes {
    full_update_turned(id, p, caster, target, spell, start, end, playfield, max_distance, [0.0; 3], [0.0, 0.0, p.offset_z])
}

/// `full_update` with the start angles (radians around x, y, z: CoreProjectile +0x284..) and the
/// local offset (m: +0x290..) an orbiting shot circles with.
#[allow(clippy::too_many_arguments)]
fn full_update_turned(
    id: Id32,
    p: &Params,
    caster: Id32,
    target: Option<Id32>,
    spell: u32,
    start: [f32; 3],
    end: [f32; 3],
    playfield: Pf,
    max_distance: i32,
    angles: [f32; 3],
    offset: [f32; 3],
) -> Bytes {
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(start[0]), y: Some(start[1]), z: Some(start[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield.key()),
        running_effects: vec![],
    };
    iir::record("CoreProjectileFullUpdateIIR", id, false, |w| {
        let w = w.proto(&dynel).u32(p.item).u32(p.move_type);
        let w = target.unwrap_or_default().write(caster.write(w));
        let w = [start, end].iter().flatten().fold(w, |w, v| w.f32(*v));
        let w = w.f32(5.0).f32(p.speed).u32(spell);
        let w = angles.iter().chain(&offset).fold(w, |w, v| w.f32(*v));
        w.u32(p.attractor).u32(p.ricochets).u32(p.chains).i32(max_distance)
    })
}

/// A `CoreProjectileIIR` from the caster's client (`blob` is the whole record, `rest` its body).
/// Relayed to the others, except a straight shot's hits (the server's flight decides those, `fly`,
/// and takes the client's word for one its flight missed); the end of a straight shot's flight
/// (it hit a wall) stops it there, a lobbed one lands.
pub fn report(from: Id32, blob: &[u8], rest: &[u8]) {
    let u = |i: usize| rest.get(i..i + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let f = |i: usize| rest.get(i..i + 4).map(|b| f32::from_be_bytes(b.try_into().unwrap()));
    let (Some(action), Some(kind), Some(instance)) = (u(0), u(4), u(8)) else { return };
    let id = Id32::new(kind, instance);
    let (straight, orbiting) = match LIVE.lock().unwrap().get(&id).filter(|l| l.caster == from) {
        Some(l) => (Some(l.flight.is_some()), l.orbit.is_some()),
        None => (None, false),
    };
    if orbiting {
        // the server moves orbiting shots and decides their hits (`circle_on`); the caster's
        // client stopping one at a wall or a hit isn't the others' business
        return;
    }
    if !(action == 1 && straight == Some(true)) {
        world::send_playfield(from, false, world::Out::Records(vec![Bytes::copy_from_slice(blob)]));
    }
    match (action, straight) {
        // Ricochet (`CoreProjectile::RicochetStaticCollision`, `RicochetTarget`): position, new
        // direction; the server's flight turns there too (else the HitPosition after it ended
        // the shot at the wall and the bounced shot hit nothing)
        (5, Some(true)) => {
            let at = [12, 16, 20].map(|i| f(i).unwrap_or(0.0));
            let mut dir = [24, 28, 32].map(|i| f(i).unwrap_or(0.0));
            let len = dir.iter().map(|v| v * v).sum::<f32>().sqrt();
            if len > 1.5 {
                // (a point to fly to, not a direction)
                dir = [0, 1, 2].map(|i| dir[i] - at[i]);
            }
            let mut live = LIVE.lock().unwrap();
            if let Some(l) = live.get_mut(&id)
                && let Some(fl) = l.flight.as_mut()
            {
                fl.turn(at, dir);
                l.bounced_at = Some(at);
                debug!(?id, ?at, ?dir, "projectile ricochet");
            }
        }
        // HitTarget of a straight shot: the client's word, if the server's flight missed it (the
        // client hit the enemy where it drew it, a little off from the server's: the user saw
        // Revolutionary Soldier shots hit and do nothing; a bounce point and direction are the
        // client's, the server's only approximate), when the enemy is near where the client hit
        // it and that is within the shot's range
        (1, Some(true)) => {
            let (Some(tk), Some(ti)) = (u(12), u(16)) else { return };
            let target = Id32::new(tk, ti);
            let at = [20, 24, 28].map(|i| f(i).unwrap_or(0.0));
            let in_range = LIVE.lock().unwrap().get(&id).is_some_and(|l| {
                !l.hit.contains(&target) && l.flight.is_some_and(|f| horizontal(f.start, at) <= f.range + f.reach + CLIENT_HIT_SLACK)
            });
            let Some(p) = crate::npc::position(target) else { return };
            if in_range && horizontal(p, at) < CLIENT_HIT_SLACK && (crate::npc::is_enemy(target) || crate::npc::is_smashable(target)) {
                debug!(?id, ?target, off = horizontal(p, at), "projectile hit reported by the client");
                strike(id, target, p);
            }
        }
        // HitTarget: ID32 target, position; a lobbed shot that came down on someone
        (1, Some(false)) => {
            let (Some(tk), Some(ti)) = (u(12), u(16)) else { return };
            let target = Id32::new(tk, ti);
            let at = [20, 24, 28].map(|i| f(i).unwrap_or(0.0));
            land(id, Some(target), Some(crate::npc::position(target).unwrap_or(at)));
        }
        // HitPosition / DestroyOnPosition: the flight is over there
        (2 | 6, Some(straight)) => {
            let at = [12, 16, 20].map(f);
            let at = (at.iter().all(Option::is_some)).then(|| at.map(Option::unwrap_or_default));
            if !straight {
                land(id, None, at);
                return;
            }
            // (a little further than where the client stopped it: it also stops a shot that hit
            // a target, which the server's flight should still reach)
            let mut live = LIVE.lock().unwrap();
            let Some(l) = live.get_mut(&id) else { return };
            // the HitPosition that comes with a ricochet (at the wall it bounced off) isn't the end
            if action == 2
                && let (Some(at), Some(b)) = (at, l.bounced_at)
                && (0..3).map(|i| (at[i] - b[i]).powi(2)).sum::<f32>().sqrt() < BOUNCE_SAME_POINT
            {
                return;
            }
            if let Some(at) = at
                && let Some(f) = l.flight.as_mut()
            {
                let d = (0..3).map(|i| (at[i] - f.start[i]).powi(2)).sum::<f32>().sqrt();
                f.stop_at = Some(d + f.reach);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A ricochet or chain starts a new leg from the bounce point, along the new direction, for
    /// the full range again.
    #[test]
    fn bounce_turns_the_flight() {
        let mut f = Flight::new([0.0; 3], [10.0, 0.0, 0.0], 20.0, 10.0, 0.5, Side::Npcs);
        f.stop_at = Some(4.0);
        f.turn([4.0, 0.0, 0.0], [0.0, 0.0, -3.0]);
        assert_eq!((f.leg, f.at(0.0), f.at(2.0), f.limit()), (1, [4.0, 0.0, 0.0], [4.0, 0.0, -2.0], 10.0));
    }

    /// Flies `f` against one character moving as `at(t)` (seconds since the launch), in the
    /// server's steps; returns the distance flown when it was hit.
    fn first_hit(f: Flight, at: impl Fn(f32) -> [f32; 3]) -> Option<f32> {
        let step = STEP.as_secs_f32();
        let mut done = 0.0f32;
        for n in 1.. {
            let t = n as f32 * step;
            let to = (f.speed * t).min(f.limit());
            if crossing(f.at(done), f.at(to), at(t), f.reach).is_some() {
                return Some(to);
            }
            done = to;
            if done >= f.limit() {
                return None;
            }
        }
        None
    }

    /// The Alien Avenger's Blaster Shot: 20 m/s, 15 m, radius 0.5, along +z from the origin.
    fn blaster() -> Flight {
        Flight::new([0.0, 1.0, 0.0], [0.0, 1.0, 15.0], 20.0, 15.0, radius_of(38408) + TARGET_RADIUS, Side::Npcs)
    }

    #[test]
    fn sizes() {
        assert_eq!(radius_of(38408), 0.5);
        assert_eq!(radius_of(50699), 2.0);
        assert_eq!(radius_of(1), MIN_RADIUS);
        assert_eq!(range_of(38125, 0.0), 15.0);
        assert_eq!(range_of(38125, 8.0), 8.0);
    }

    #[test]
    fn standing_target_on_the_line_is_hit() {
        assert!(first_hit(blaster(), |_| [0.0, 0.0, 10.0]).is_some());
        assert!(first_hit(blaster(), |_| [0.9, 0.0, 10.0]).is_some());
        assert!(first_hit(blaster(), |_| [1.1, 0.0, 10.0]).is_none());
    }

    #[test]
    fn running_sideways_escapes() {
        // 10 m away, the shot takes 0.5 s; running at 5.6 m/s sideways leaves the 1 m reach
        assert!(first_hit(blaster(), |t| [5.6 * t, 0.0, 10.0]).is_none());
        // a slow sidestep doesn't
        assert!(first_hit(blaster(), |t| [1.0 * t, 0.0, 10.0]).is_some());
    }

    #[test]
    fn running_along_the_line_does_not_escape() {
        // towards the shooter: hit sooner
        let towards = first_hit(blaster(), |t| [0.0, 0.0, 10.0 - 5.6 * t]).unwrap();
        assert!(towards < 9.5, "{towards}");
        // away: the shot catches up within its range
        assert!(first_hit(blaster(), |t| [0.0, 0.0, 10.0 + 5.6 * t]).is_some());
        // away, starting near the end of its range: out of reach
        assert!(first_hit(blaster(), |t| [0.0, 0.0, 13.0 + 5.6 * t]).is_none());
    }

    #[test]
    fn beyond_range_or_where_the_client_stopped_is_missed() {
        assert!(first_hit(blaster(), |_| [0.0, 0.0, 16.5]).is_none());
        let walled = Flight { stop_at: Some(5.0), ..blaster() };
        assert!(first_hit(walled, |_| [0.0, 0.0, 10.0]).is_none());
    }

    #[test]
    fn another_floor_is_missed() {
        assert!(first_hit(blaster(), |_| [0.0, 6.0, 10.0]).is_none());
    }

    fn orbit(offset: [f32; 3], angle_deg: f32, speed: f32) -> Orbit {
        let now = Instant::now();
        Orbit { centre: Id32::new(1, 1), offset, angle: angle_deg.to_radians(), rate: orbit_rate(speed), reach: 1.0, side: Side::Npcs, launched: now, until: now }
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3)
    }

    /// Wheel of Fortune: Speed 300 (3 m/s on the wire) is 300° a second; a card starts ahead
    /// (its offset is only the radii, along z) turned by its RotY, and follows its centre.
    #[test]
    fn orbits_turn_around_their_centre() {
        assert!((orbit_rate(3.0) - 300f32.to_radians()).abs() < 1e-4);
        let card = orbit([0.0, 0.0, 1.0], 0.0, 3.0);
        assert!(near(card.at([0.0; 3], 0.0), [0.0, 0.0, 1.0]));
        // a quarter turn (0.3 s at 300°/s): +z turns to +x
        assert!(near(card.at([0.0; 3], 0.3), [1.0, 0.0, 0.0]));
        assert!(near(card.at([10.0, 2.0, 5.0], 0.0), [10.0, 2.0, 6.0]));
        // RotY 90 starts where the first is a quarter turn later
        assert!(near(orbit([0.0, 0.0, 1.0], 90.0, 3.0).at([0.0; 3], 0.0), [1.0, 0.0, 0.0]));
        // the Sad Clown's clouds: 1.5 m to the side, 0.5 m up, plus the radii ahead
        let cloud = orbit([1.5, 0.5, 1.0], 0.0, 2.0);
        let p = cloud.at([0.0; 3], 0.7);
        assert!(((p[0] * p[0] + p[2] * p[2]).sqrt() - 3.25f32.sqrt()).abs() < 1e-3 && (p[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn aim_modes() {
        assert_eq!(player_aim(Shots::Straight), Aim::Current);
        assert_eq!(player_aim(Shots::Lobbed), Aim::Lead);
        assert!(!set_player_aim(Shots::Straight, Aim::Homing));
        assert_eq!(player_aim(Shots::Straight), Aim::Current);
        assert!(set_player_aim(Shots::Lobbed, Aim::Current));
        assert_eq!(player_aim(Shots::Lobbed), Aim::Current);
        assert_eq!(player_aim(Shots::Straight), Aim::Current);
        assert!(set_player_aim(Shots::Lobbed, Aim::Lead));
    }
}
