//! Trigger areas and invisible walls on play servers (docs/WORLD-CONTENT.md "Walls and trigger
//! areas"): the content's areas (`worlds/<world>/areas.json`, most of them the client's RDB 1000053
//! server bounded areas, tools/re/extract_server_areas.py) and the triggers that cover them
//! (`triggers.json` records with an `area`, or with a polygon of their own).
//!
//! - **Triggers.** Every `TICK` the players' positions are checked against the triggers of their
//!   playfield: walking in (`on: enter`) or out (`exit`) runs the trigger's actions, if its
//!   `requires` conditions hold and its `once` allows. A `kill` area (FallThroughWorld) counts
//!   everything below its top as inside, so a fall through its thin box isn't missed. The walls
//!   and radii the world editor draws stay walls.rs's (one action, no area).
//! - **Walls** (areas with role `wall`, `blocks`): NPC paths stop short of any wall that blocks
//!   pathing (`clip_path`, npc.rs `steer`); a player who crosses a *server* wall that blocks
//!   movement (the client doesn't know those: 1000053's collision section) hops back to where they
//!   were (`jumppad::throw_as`; how live kept players out is unknown, docs/TUNABLES.md). A
//!   trigger's `set_state` `open` / `closed` on a wall area switches it in that copy of the world.
//!
//! Actions run here: teleport (a position, a named point, a world's arrival, or the command
//! `nearest_respawn`), apply_buff / remove_buff, heal, message, effect, start_script /
//! end_script, open_window, discover, set_state (wall areas), command (`clear_hate`, and what
//! `register_command` added). `cinematic` and `dialogue` go to the commands of those names when
//! something registered them (phase 3: story), else they are only logged; so do the actions not
//! listed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use lmoxide_content::{Action, Area, AreaRole, Condition, Once, Shape, Target, Trigger, TriggerOn, WallSide};
use lmoxide_net::types::Id32;
use serde_json::Value;
use tracing::{info, warn};

use crate::instance::Pf;
use crate::world;

/// How often players' positions are checked (as walls.rs).
const TICK: Duration = Duration::from_millis(200);
/// A move longer than this between two checks is a teleport, not a walk through a wall.
const JUMP: f32 = 15.0;
/// Height slack of an area's band (m).
const HEIGHT_SLACK: f32 = 1.0;
/// How far short of a wall an NPC's path ends (m).
const WALL_MARGIN: f32 = 0.6;
/// A wall's height band counts this much wider (m) for paths and crossings (ground unevenness).
const WALL_SLACK: f32 = 1.5;
/// A player pushed back from a server wall isn't checked again for this long (the hop).
const PUSH_COOLDOWN: Duration = Duration::from_millis(1200);
/// The hop back from a server wall: its height above the higher end (m).
const PUSH_APEX: f32 = 0.4;

/// A trigger as this module runs it.
#[derive(Clone)]
struct Trig {
    id: String,
    name: String,
    area: Option<String>,
    shape: Shape,
    /// Everything below the shape's top counts as inside (kill areas).
    below: bool,
    on: TriggerOn,
    once: Once,
    requires: Vec<Condition>,
    actions: Vec<Action>,
}

impl Trig {
    fn contains(&self, pos: [f32; 3]) -> bool {
        match (&self.shape, self.below) {
            (Shape::Polygon { points, heights }, true) => {
                // (no slack on top: some worlds' lowest floors are just above their kill box)
                Shape::Polygon { points: points.clone(), heights: [f32::MIN, heights[1]] }.contains(pos, 0.0)
            }
            (shape, _) => shape.contains(pos, HEIGHT_SLACK),
        }
    }
}

/// A wall area: its segments on the ground (x, z) and its height band.
#[derive(Clone, Debug)]
pub struct Wall {
    pub id: String,
    pub name: String,
    pub by: WallSide,
    pub movement: bool,
    pub pathing: bool,
    /// Off unless something switched it on.
    pub off: bool,
    y: [f32; 2],
    segments: Vec<([f32; 2], [f32; 2])>,
    /// min x, min z, max x, max z
    bounds: [f32; 4],
}

impl Wall {
    fn of(id: &str, a: &Area) -> Option<Wall> {
        let b = a.blocks.as_ref()?;
        let Some(Shape::Polygon { points, heights }) = &a.shape else { return None };
        if points.len() < 2 {
            return None;
        }
        let mut segments: Vec<([f32; 2], [f32; 2])> = points.windows(2).map(|w| (w[0], w[1])).collect();
        if !b.open_line && points.len() > 2 {
            segments.push((points[points.len() - 1], points[0]));
        }
        let bounds = points.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
        Some(Wall { id: id.into(), name: a.name.clone(), by: b.by, movement: b.movement, pathing: b.pathing, off: b.off, y: *heights, segments, bounds })
    }

    /// Where along `from` -> `to` (0..1) it first crosses this wall, if it does.
    fn crossing(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
        let (lo, hi) = (self.y[0] - WALL_SLACK, self.y[1] + WALL_SLACK);
        if from[1].max(to[1]) < lo || from[1].min(to[1]) > hi {
            return None;
        }
        let b = self.bounds;
        if from[0].max(to[0]) < b[0] || from[0].min(to[0]) > b[2] || from[2].max(to[2]) < b[1] || from[2].min(to[2]) > b[3] {
            return None;
        }
        self.segments.iter().filter_map(|(p, q)| segment_hit([from[0], from[2]], [to[0], to[2]], *p, *q)).min_by(f32::total_cmp)
    }

    /// The middle of the wall (for test commands).
    pub fn centre(&self) -> [f32; 3] {
        [(self.bounds[0] + self.bounds[2]) / 2.0, self.y[0], (self.bounds[1] + self.bounds[3]) / 2.0]
    }
}

/// Where segment a->b meets segment p->q, as a share of a->b.
fn segment_hit(a: [f32; 2], b: [f32; 2], p: [f32; 2], q: [f32; 2]) -> Option<f32> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [q[0] - p[0], q[1] - p[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-9 {
        return None;
    }
    let ap = [p[0] - a[0], p[1] - a[1]];
    let t = (ap[0] * s[1] - ap[1] * s[0]) / den;
    let u = (ap[0] * r[1] - ap[1] * r[0]) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

/// An area for test commands and named teleport points.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct AreaInfo {
    pub id: String,
    pub name: String,
    pub playfield: u32,
    pub role: AreaRole,
    pub category: Option<String>,
    pub shape: Shape,
}

impl AreaInfo {
    /// A point in the middle of the area (x, bottom, z), or the point itself.
    pub fn centre(&self) -> [f32; 3] {
        shape_centre(&self.shape)
    }
}

/// A point in the middle of a shape (x, bottom, z).
pub fn area_centre(shape: &Shape) -> [f32; 3] {
    shape_centre(shape)
}

fn shape_centre(shape: &Shape) -> [f32; 3] {
    match shape {
        Shape::Polygon { points, heights } if !points.is_empty() => {
            let n = points.len() as f32;
            [points.iter().map(|p| p[0]).sum::<f32>() / n, heights[0], points.iter().map(|p| p[1]).sum::<f32>() / n]
        }
        Shape::Wall { pos, .. } | Shape::Circle { pos, .. } | Shape::Point { pos, .. } => *pos,
        Shape::Ground { at } => [at[0], 0.0, at[1]],
        Shape::Polygon { .. } => [0.0; 3],
    }
}

#[derive(Default)]
struct Index {
    triggers: HashMap<u32, Vec<Trig>>,
    walls: HashMap<u32, Vec<Wall>>,
    areas: HashMap<String, AreaInfo>,
}

static INDEX: LazyLock<RwLock<Index>> = LazyLock::new(Default::default);

#[derive(Default)]
struct State {
    /// Each player's playfield, position and the triggers they were inside at the last check.
    last: HashMap<Id32, (Pf, [f32; 3], HashSet<String>)>,
    fired_player: HashSet<(String, Id32)>,
    fired_copy: HashSet<(String, Pf)>,
    fired_global: HashSet<String>,
    /// Walls switched in a copy: (copy, wall area) -> off.
    switched: HashMap<(Pf, String), bool>,
    /// Players hopping back from a server wall, until when.
    pushed: HashMap<Id32, Instant>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What a fired trigger hands its actions (and registered commands).
#[allow(dead_code)] // `area` and `player_name` are for registered commands
pub struct Fired<'a> {
    pub trigger: &'a str,
    pub area: Option<&'a str>,
    pub player: Id32,
    pub player_name: &'a str,
    pub pf: Pf,
    pub pos: [f32; 3],
}

pub type CommandFn = Arc<dyn Fn(&Fired, &BTreeMap<String, Value>) + Send + Sync>;

static COMMANDS: LazyLock<Mutex<HashMap<String, CommandFn>>> = LazyLock::new(Default::default);

/// Lets a trigger's `command` action (`{ "command": { "name": <name>, "args": {..} } }`) run `f`.
/// The names `cinematic` and `dialogue` also take those actions (args `name` / `dialogue`).
pub fn register_command(name: &str, f: CommandFn) {
    COMMANDS.lock().unwrap().insert(name.to_string(), f);
}

/// The content's areas and triggers (the world editor's work as loaded: content.rs, survey.rs).
pub fn set_from_work(work: &lmoxide_content::work::Work) {
    use lmoxide_content::id::RecordKind;
    let live = |kind: RecordKind| {
        work.records().filter(move |(k, _)| k.id.kind == kind && k.feature == "live").filter_map(|(k, e)| Some((k.id.world?, e.value.clone()))).collect::<Vec<_>>()
    };
    let areas: Vec<(u32, Area)> = live(RecordKind::Area).into_iter().filter_map(|(pf, v)| Some((pf, serde_json::from_value(v).ok()?))).collect();
    let triggers: Vec<(u32, Trigger)> = live(RecordKind::Trigger).into_iter().filter_map(|(pf, v)| Some((pf, serde_json::from_value(v).ok()?))).collect();
    set(areas, triggers);
}

fn set(areas: Vec<(u32, Area)>, triggers: Vec<(u32, Trigger)>) {
    let mut index = Index::default();
    for (pf, a) in &areas {
        if let Some(w) = Wall::of(&a.id, a) {
            index.walls.entry(*pf).or_default().push(w);
        }
        if let Some(shape) = &a.shape {
            index.areas.insert(
                a.id.clone(),
                AreaInfo {
                    id: a.id.clone(),
                    name: a.client_name.clone().unwrap_or_else(|| a.name.clone()),
                    playfield: *pf,
                    role: a.role,
                    category: a.category.clone(),
                    shape: shape.clone(),
                },
            );
        }
    }
    for (pf, t) in triggers {
        if t.removed {
            continue;
        }
        let (shape, below) = match (&t.area, &t.shape) {
            (Some(area), _) => match index.areas.get(area) {
                Some(a) => (a.shape.clone(), a.role == AreaRole::Kill),
                None => {
                    warn!(trigger = %t.id, area, "trigger: no such area");
                    continue;
                }
            },
            // a polygon of its own, or several actions (walls.rs runs the one-action walls and radii)
            (None, Some(s @ Shape::Polygon { .. })) => (s.clone(), false),
            (None, Some(s @ (Shape::Wall { .. } | Shape::Circle { .. }))) if t.actions.len() > 1 => (s.clone(), false),
            _ => continue,
        };
        index.triggers.entry(pf).or_default().push(Trig {
            id: t.id.clone(),
            name: t.name.clone(),
            area: t.area.clone(),
            shape,
            below,
            on: t.on,
            once: t.once,
            requires: t.requires.clone(),
            actions: t.actions.clone(),
        });
    }
    let (nt, nw): (usize, usize) = (index.triggers.values().map(Vec::len).sum(), index.walls.values().map(Vec::len).sum());
    info!(triggers = nt, walls = nw, areas = index.areas.len(), "trigger areas and walls");
    *INDEX.write().unwrap() = index;
    if nt + nw > 0 && !WATCHING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(watch());
        }
    }
}

/// Whether wall `w` blocks now in copy `pf`.
fn wall_on(st: &State, pf: Pf, w: &Wall) -> bool {
    !st.switched.get(&(pf, w.id.clone())).copied().unwrap_or(w.off)
}

/// Where an NPC walking `from` -> `to` in copy `pf` has to stop: `WALL_MARGIN` short of the first
/// wall that blocks pathing, or `to`.
pub fn clip_path(pf: Pf, from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    let index = INDEX.read().unwrap();
    let Some(walls) = index.walls.get(&pf.model()) else { return to };
    let st = STATE.lock().unwrap();
    let hit = walls.iter().filter(|w| w.pathing).filter_map(|w| w.crossing(from, to).filter(|_| wall_on(&st, pf, w))).min_by(f32::total_cmp);
    let Some(t) = hit else { return to };
    let len = ((to[0] - from[0]).powi(2) + (to[2] - from[2]).powi(2)).sqrt().max(0.001);
    let t = (t - WALL_MARGIN / len).max(0.0);
    [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t, from[2] + (to[2] - from[2]) * t]
}

/// A copy of a world emptied: its once-per-copy triggers and switched walls start over.
pub fn playfield_emptied(pf: Pf) {
    let mut st = STATE.lock().unwrap();
    st.fired_copy.retain(|(_, p)| *p != pf);
    st.switched.retain(|(p, _), _| *p != pf);
}

async fn watch() {
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        let players: Vec<(Id32, String, Pf, [f32; 3])> = crate::sessions::players()
            .into_iter()
            .filter_map(|p| {
                let id = Id32::new(crate::territory::CHARACTER_TYPE, p.character_id);
                let (pf, _) = world::whereabouts(id).filter(|(pf, _)| !crate::instance::rules(*pf).editor)?;
                Some((id, p.character_name, pf, world::live_position(id)?))
            })
            .collect();
        let (fire, push) = check(&players, Instant::now());
        for (player, back) in push {
            push_back(player, back);
        }
        for (t, player, name, pf, pos) in fire {
            run(&t, player, &name, pf, pos);
        }
    }
}

/// One check of everyone: the triggers to fire, the players to push back (to where).
#[allow(clippy::type_complexity)]
fn check(players: &[(Id32, String, Pf, [f32; 3])], now: Instant) -> (Vec<(Trig, Id32, String, Pf, [f32; 3])>, Vec<(Id32, [f32; 3])>) {
    let index = INDEX.read().unwrap();
    let mut st = STATE.lock().unwrap();
    st.last.retain(|id, _| players.iter().any(|p| p.0 == *id));
    st.pushed.retain(|_, until| *until > now);
    let (mut fire, mut push) = (Vec::new(), Vec::new());
    for (player, name, pf, pos) in players {
        let model = pf.model();
        let (was, before) = match st.last.remove(player) {
            Some((p, at, inside)) if p == *pf => (Some(at), inside),
            _ => (None, HashSet::new()),
        };
        // a server wall in the way: back to where they were
        // (jump pads fly over them: not in the air)
        if let Some(was) = was.filter(|w| dist(*w, *pos) <= JUMP && !st.pushed.contains_key(player) && !crate::jumppad::airborne(*player)) {
            let blocked = index.walls.get(&model).into_iter().flatten().any(|w| w.by == WallSide::Server && w.movement && w.crossing(was, *pos).is_some() && wall_on(&st, *pf, w));
            if blocked {
                st.pushed.insert(*player, now + PUSH_COOLDOWN);
                push.push((*player, was));
                st.last.insert(*player, (*pf, was, before));
                continue;
            }
        }
        let triggers = index.triggers.get(&model).map_or(&[][..], Vec::as_slice);
        let inside: HashSet<String> = triggers.iter().filter(|t| t.contains(*pos)).map(|t| t.id.clone()).collect();
        for t in triggers {
            let fires = match t.on {
                TriggerOn::Enter => inside.contains(&t.id) && !before.contains(&t.id),
                TriggerOn::Exit => !inside.contains(&t.id) && before.contains(&t.id),
                TriggerOn::Interact => false,
            };
            if !fires || !t.requires.iter().all(|c| holds(&index, &st, *pf, c)) {
                continue;
            }
            let first = match t.once {
                Once::Never => true,
                Once::PerPlayer => st.fired_player.insert((t.id.clone(), *player)),
                Once::PerInstance => st.fired_copy.insert((t.id.clone(), *pf)),
                Once::Global => st.fired_global.insert(t.id.clone()),
            };
            if first {
                fire.push((t.clone(), *player, name.clone(), *pf, *pos));
            }
        }
        st.last.insert(*player, (*pf, *pos, inside));
    }
    (fire, push)
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether condition `c` holds in copy `pf` (called with the state locked).
fn holds(index: &Index, st: &State, pf: Pf, c: &Condition) -> bool {
    let Condition::State { target, state } = c;
    let Target::Placement(id) = target else {
        warn!(?target, "trigger condition: only placements are known");
        return false;
    };
    if id.starts_with("area/") {
        let Some(w) = index.walls.get(&pf.model()).and_then(|ws| ws.iter().find(|w| &w.id == id)) else { return false };
        let on = wall_on(st, pf, w);
        return match state.as_str() {
            "closed" | "on" => on,
            "open" | "off" => !on,
            _ => false,
        };
    }
    match (crate::gamedata::get().spawns.get(id), state.as_str()) {
        (Some(s), "built") => crate::buildie::built_at(pf, s.pos),
        (Some(s), "unbuilt") => !crate::buildie::built_at(pf, s.pos),
        _ => false,
    }
}

/// Hops `player` back to `to` (they crossed a server wall).
fn push_back(player: Id32, to: [f32; 3]) {
    let Some((pf, _)) = world::whereabouts(player) else { return };
    let Some((_, handle)) = world::position_handles(pf).into_iter().find(|(id, _)| *id == player) else { return };
    info!(player = player.instance, ?to, "server wall: pushed back");
    let hop = crate::jumppad::Tumble { knockback: false, launched: false, direction: 0 };
    tokio::spawn(async move { crate::jumppad::throw_as(player, &handle, to, PUSH_APEX, hop).await });
}

/// Runs fired trigger `t`'s actions.
fn run(t: &Trig, player: Id32, player_name: &str, pf: Pf, pos: [f32; 3]) {
    info!(trigger = %t.id, name = %t.name, player = player.instance, actions = t.actions.len(), "trigger area");
    let fired = Fired { trigger: &t.id, area: t.area.as_deref(), player, player_name, pf, pos };
    for a in &t.actions {
        act(&fired, a);
    }
}

fn command(f: &Fired, name: &str, args: &BTreeMap<String, Value>) -> bool {
    let cmd = COMMANDS.lock().unwrap().get(name).cloned();
    match cmd {
        Some(c) => {
            c(f, args);
            true
        }
        None => false,
    }
}

/// Shows content buff `buff` (`buff/...`) on `player`, cast by the player, for `duration` or the
/// buff's own (a trigger's `apply_buff`, `!buff`). Only its nano is sent (`MagicBuffIIR_c` add):
/// the client plays the nano's EffectPackage by itself (the speed pad's trail 463557; live sent
/// no EffectPackageIIR with buff 459025, only its add and its remove 10 s later) and its client
/// actions do the rest. Returns how long it lasts.
pub fn apply_buff(player: Id32, buff: &str, duration: Option<Duration>) -> Result<Option<Duration>, String> {
    let c = crate::gamedata::get();
    let b = c.buffs.get(buff).ok_or_else(|| format!("no such buff {buff}"))?;
    let nano = b.nano.ok_or_else(|| format!("buff {buff} has no nano"))?;
    let d = duration.or(b.duration.map(|d| d.to_std()));
    let records = crate::effects::show_buff(player, nano, player, d);
    world::send_playfield(player, true, world::Out::Records(records));
    Ok(d)
}

/// One action for the player who set the trigger off.
fn act(f: &Fired, a: &Action) {
    let c = crate::gamedata::get();
    match a {
        Action::Teleport { to } => {
            let target_pf = to.world.as_ref().and_then(|w| c.worlds.get(w)).map(|w| w.playfield);
            let place = match (to.command.as_deref(), to.pos, to.point.as_deref()) {
                (Some("nearest_respawn"), _, _) => crate::teleport::nearest_spot(f.pf.model(), f.pos).map(|s| (None, s.pos, s.rot)),
                (Some(other), _, _) => {
                    let args = to.args.clone();
                    if !command(f, other, &args) {
                        info!(trigger = f.trigger, command = other, "trigger: teleport command not known");
                    }
                    return;
                }
                (None, Some(pos), _) => Some((target_pf, pos, 0.0)),
                (None, None, Some(point)) => {
                    let model = target_pf.unwrap_or(f.pf.model());
                    named_point(model, point).map(|(p, rot)| (target_pf, p, rot))
                }
                (None, None, None) => target_pf.and_then(|pf| crate::teleport::destination(pf).map(|d| (Some(pf), d.pos, d.rot))),
            };
            match place {
                Some((pf, at, rot)) => world::send_to(f.player, world::Out::Teleport(pf, at, rot)),
                None => info!(trigger = f.trigger, ?to, "trigger: nowhere to teleport to"),
            }
        }
        Action::ApplyBuff { buff, duration, .. } => {
            if let Err(why) = apply_buff(f.player, buff, duration.map(|d| d.to_std())) {
                warn!(trigger = f.trigger, buff, "trigger: {why}");
            }
        }
        Action::RemoveBuff { buff, .. } => {
            if let Some(nano) = c.buffs.get(buff).and_then(|b| b.nano) {
                crate::effects::forget_buff(f.player, nano);
                world::send_playfield(f.player, true, world::Out::Records(vec![crate::effects::buff_remove(f.player, nano, f.player)]));
            }
        }
        Action::Heal { amount, .. } => {
            let heal = (crate::npc::player_max_health(f.player) as f64 * amount.fraction()) as u32;
            crate::npc::heal_player(f.player, heal);
        }
        Action::Message { text } => crate::chat::system(f.player.instance, text),
        Action::Effect { effect_package, .. } => {
            world::send_playfield(f.player, true, world::Out::Records(vec![crate::effects::effect_package(f.player, *effect_package)]));
        }
        Action::StartScript { script, wave } => match crate::scripts::start(f.pf, script, wave.as_deref()) {
            Ok(what) => info!(trigger = f.trigger, "trigger: {what}"),
            Err(why) => info!(trigger = f.trigger, "trigger: script not started: {why}"),
        },
        Action::EndScript { script } => {
            crate::scripts::stop(f.pf, Some(script.as_str()));
        }
        Action::OpenWindow { window } => world::send_to(f.player, world::Out::Window(*window)),
        Action::Discover { location, .. } => {
            if let Some(name) = location.as_ref().filter(|n| !n.is_empty()) {
                world::send_to(f.player, world::Out::Progress(crate::achievements::Event::Found { playfield: f.pf.model(), name: name.clone() }));
            }
        }
        Action::SetState { target: Target::Placement(id), state } if id.starts_with("area/") => set_wall(f.pf, id, state == "open" || state == "off"),
        Action::Command { name, args } => match name.as_str() {
            "clear_hate" => {
                let lasts = args.get("for").and_then(|v| serde_json::from_value::<lmoxide_content::Duration>(v.clone()).ok()).map_or(Duration::from_secs(5), |d| d.to_std());
                crate::npc::clear_hate(f.player, lasts);
            }
            other => {
                if !command(f, other, args) {
                    info!(trigger = f.trigger, command = other, "trigger: command not known");
                }
            }
        },
        Action::Cinematic { name } => {
            let args = BTreeMap::from([("name".to_string(), Value::from(name.clone()))]);
            if !command(f, "cinematic", &args) {
                info!(trigger = f.trigger, name, "trigger: cinematic (story: not run yet)");
            }
        }
        Action::Dialogue { dialogue } => {
            let args = BTreeMap::from([("dialogue".to_string(), Value::from(dialogue.clone()))]);
            if !command(f, "dialogue", &args) {
                info!(trigger = f.trigger, dialogue, "trigger: dialogue (story: not run yet)");
            }
        }
        other => info!(trigger = f.trigger, action = ?other, "trigger: action not run here"),
    }
}

/// A named point of playfield `model`: a content area of that name (a marker's point, an area's
/// middle).
fn named_point(model: u32, name: &str) -> Option<([f32; 3], f32)> {
    let index = INDEX.read().unwrap();
    let a = index.areas.values().find(|a| a.playfield == model && a.name.eq_ignore_ascii_case(name))?;
    let rot = match &a.shape {
        Shape::Point { rot: Some(lmoxide_content::Rot::Heading(h)), .. } => *h,
        _ => 0.0,
    };
    Some((a.centre(), rot))
}

/// Switches wall area `id` in copy `pf` (`off`: open).
/// Forgets which once-per-player triggers `player` set off (a story reset: the story's areas
/// go off again).
pub fn forget_player(player: Id32) {
    STATE.lock().unwrap().fired_player.retain(|(_, p)| *p != player);
}

pub fn set_wall(pf: Pf, id: &str, off: bool) {
    info!(playfield = ?pf, wall = id, off, "wall switched");
    STATE.lock().unwrap().switched.insert((pf, id.to_string()), off);
}

/// The walls of playfield `model` and whether each blocks now in copy `pf` (test commands).
pub fn walls_of(pf: Pf) -> Vec<(Wall, bool)> {
    let index = INDEX.read().unwrap();
    let st = STATE.lock().unwrap();
    index.walls.get(&pf.model()).into_iter().flatten().map(|w| (w.clone(), wall_on(&st, pf, w))).collect()
}

/// The areas of playfield `model` (test commands).
pub fn areas_of(model: u32) -> Vec<AreaInfo> {
    let mut v: Vec<AreaInfo> = INDEX.read().unwrap().areas.values().filter(|a| a.playfield == model).cloned().collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Ids of the triggers covering area `id`.
pub fn triggers_on(area: &str) -> Vec<String> {
    INDEX.read().unwrap().triggers.values().flatten().filter(|t| t.area.as_deref() == Some(area)).map(|t| t.id.clone()).collect()
}

/// Test targets: (label, playfield, position) of every area of a role (`kill`: above the
/// volume's top, to fall in), or of every wall.
pub fn points(kind: &str) -> Vec<(String, u32, [f32; 3])> {
    let index = INDEX.read().unwrap();
    let mut v: Vec<(String, u32, [f32; 3])> = match kind {
        "wall" => index
            .walls
            .iter()
            .flat_map(|(pf, ws)| ws.iter().map(|w| (format!("{} ({:?}{})", w.name, w.by, if w.movement { "" } else { ", no movement" }), *pf, w.centre())))
            .collect(),
        "fallzone" => index
            .areas
            .values()
            .filter(|a| a.role == AreaRole::Kill)
            .map(|a| {
                let c = a.centre();
                let top = match &a.shape {
                    Shape::Polygon { heights, .. } => heights[1],
                    _ => c[1],
                };
                (a.name.clone(), a.playfield, [c[0], top + 2.0, c[2]])
            })
            .collect(),
        "speedpad" => index.areas.values().filter(|a| a.name.starts_with("BA_Speedbuff")).map(|a| (a.name.clone(), a.playfield, a.centre())).collect(),
        "area" => index
            .areas
            .values()
            .filter(|a| matches!(a.role, AreaRole::Trigger | AreaRole::Story | AreaRole::Territory))
            .map(|a| (format!("{} [{:?}{}]", a.name, a.role, a.category.as_deref().map(|c| format!(" {c}")).unwrap_or_default()), a.playfield, a.centre()))
            .collect(),
        _ => Vec::new(),
    };
    v.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmoxide_content::Blocks;

    fn area(id: &str, role: AreaRole, points: Vec<[f32; 2]>, heights: [f32; 2], blocks: Option<Blocks>) -> (u32, Area) {
        (6300, Area { id: id.into(), name: id.into(), role, shape: Some(Shape::Polygon { points, heights }), blocks, ..Default::default() })
    }

    fn trigger(id: &str, area: &str, actions: Vec<Action>) -> (u32, Trigger) {
        (6300, Trigger { id: id.into(), area: Some(area.into()), actions, ..Default::default() })
    }

    #[test]
    fn registered_commands_run() {
        static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
        register_command(
            "story_test",
            Arc::new(|f: &Fired, args: &BTreeMap<String, Value>| {
                SEEN.lock().unwrap().push(format!("{} {:?} {}", f.trigger, f.area, args["beat"]));
            }),
        );
        register_command(
            "cinematic",
            Arc::new(|f: &Fired, args: &BTreeMap<String, Value>| {
                SEEN.lock().unwrap().push(format!("{} cinematic {}", f.trigger, args["name"]));
            }),
        );
        let f = Fired { trigger: "trigger/1300/t", area: Some("area/1300/a"), player: Id32::default(), player_name: "me", pf: Pf::main(1300), pos: [0.0; 3] };
        act(&f, &Action::Command { name: "story_test".into(), args: BTreeMap::from([("beat".into(), Value::from(3))]) });
        act(&f, &Action::Cinematic { name: "Oops".into() });
        assert_eq!(*SEEN.lock().unwrap(), ["trigger/1300/t Some(\"area/1300/a\") 3", "trigger/1300/t cinematic \"Oops\""]);
    }

    #[test]
    fn segments_cross() {
        assert_eq!(segment_hit([0.0, 0.0], [10.0, 0.0], [5.0, -1.0], [5.0, 1.0]), Some(0.5));
        assert_eq!(segment_hit([0.0, 0.0], [4.0, 0.0], [5.0, -1.0], [5.0, 1.0]), None);
        assert_eq!(segment_hit([0.0, 0.0], [10.0, 0.0], [0.0, 1.0], [10.0, 1.0]), None);
    }

    #[tokio::test]
    async fn walls_paths_and_triggers() {
        let line = Blocks { by: WallSide::Server, movement: true, pathing: true, open_line: true, ..Default::default() };
        let client = Blocks { by: WallSide::Client, movement: true, pathing: false, ..Default::default() };
        set(
            vec![
                area("area/6300/ring", AreaRole::Wall, vec![[10.0, -5.0], [10.0, 5.0]], [0.0, 10.0], Some(line)),
                area("area/6300/pillar", AreaRole::Wall, vec![[20.0, -1.0], [22.0, -1.0], [22.0, 1.0], [20.0, 1.0]], [0.0, 10.0], Some(client)),
                area("area/6300/fall", AreaRole::Kill, vec![[-100.0, -100.0], [100.0, -100.0], [100.0, 100.0], [-100.0, 100.0]], [-1.0, 0.0], None),
                area("area/6300/pad", AreaRole::Trigger, vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]], [1.0, 3.0], None),
            ],
            vec![
                trigger("trigger/6300/fall", "area/6300/fall", vec![Action::Message { text: "fell".into() }]),
                (6300, Trigger { id: "trigger/6300/pad".into(), area: Some("area/6300/pad".into()), once: Once::PerPlayer, ..Default::default() }),
            ],
        );
        let pf = Pf::main(6300);
        // an NPC path stops short of the ring; the client pillar doesn't block paths
        let to = clip_path(pf, [0.0, 1.0, 0.0], [20.0, 1.0, 0.0]);
        assert!((to[0] - (10.0 - WALL_MARGIN)).abs() < 0.01, "{to:?}");
        assert_eq!(clip_path(pf, [15.0, 1.0, 0.0], [30.0, 1.0, 0.0]), [30.0, 1.0, 0.0]);
        // above the wall's band: no wall
        assert_eq!(clip_path(pf, [0.0, 30.0, 0.0], [20.0, 30.0, 0.0]), [20.0, 30.0, 0.0]);
        // opened, the ring lets paths through
        set_wall(pf, "area/6300/ring", true);
        assert_eq!(clip_path(pf, [0.0, 1.0, 0.0], [20.0, 1.0, 0.0]), [20.0, 1.0, 0.0]);
        set_wall(pf, "area/6300/ring", false);

        let me = Id32::new(crate::territory::CHARACTER_TYPE, 77);
        let now = Instant::now();
        let at = |pos: [f32; 3]| vec![(me, "me".to_string(), pf, pos)];
        // walking onto the pad fires it once
        let (fire, push) = check(&at([5.0, 2.0, 5.0]), now);
        assert!(fire.is_empty() && push.is_empty());
        let (fire, _) = check(&at([1.0, 2.0, 1.0]), now);
        assert_eq!(fire.iter().map(|f| f.0.id.as_str()).collect::<Vec<_>>(), ["trigger/6300/pad"]);
        let _ = check(&at([5.0, 2.0, 5.0]), now);
        let (fire, _) = check(&at([1.0, 2.0, 1.0]), now);
        assert!(fire.is_empty(), "once per player");
        // walking through the server ring: pushed back to where they were
        let _ = check(&at([8.0, 1.0, 0.0]), now);
        let (_, push) = check(&at([12.0, 1.0, 0.0]), now);
        assert_eq!(push, vec![(me, [8.0, 1.0, 0.0])]);
        // falling far below the thin kill box still counts
        let (fire, _) = check(&at([30.0, -40.0, 30.0]), now + PUSH_COOLDOWN * 2);
        assert_eq!(fire.iter().map(|f| f.0.id.as_str()).collect::<Vec<_>>(), ["trigger/6300/fall"]);
    }
}
