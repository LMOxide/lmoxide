//! Jump pads (mortars, wind fans, catapults...): step on one and it throws you to its landing
//! point.
//!
//! The client sends nothing for a pad; the original server noticed the player standing on it
//! and drove the whole jump (seen in the recorded session): an effect on the pad, a `StopIIR_c`,
//! a ballistic `MoveTaskIIR_c` to the landing point and the "jumppad lock" buff, then at the end
//! a landing effect and a `StopIIR_c` at the landing point. Pads and landing points come from
//! content/worlds/*/bouncers.json (world_data.rs); outside
//! the recorded worlds the landing points are guesses.
//!
//! The world editor's bouncers are content records like the rest; the survey server puts its
//! working copy's in as they change ([`set_records`]).

use crate::instance::Pf;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, LazyLock, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{ProtoGameCodeDynelIir, ProtoQuat, ProtoSimpleDynelIir, ProtoVec3},
    types::Id32,
};
use lmoxide_survey::{
    self as sv, Kind as SKind, Placement,
    Category,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    effects,
    game::{create_game_object, int32_stat},
    world,
};

/// Dynel type of the pads (as in the recorded session).
const PAD_TYPE: u32 = 51320;
/// The pads' template item ("Jumper") and client script.
const PAD_TEMPLATE: u32 = 86651;
const PAD_CLIENT_SCRIPT: u32 = 601600;
const CARS_GROUP: u32 = 3;
const CARS_INACTIVE: u32 = 5;
const STAT_CARS_GROUP: u32 = 1050;
/// How close (horizontally) the player must come to a pad's centre (assumed; the original
/// server launched within ~0.5 m, see docs/TUNABLES.md).
const TRIGGER_RADIUS: f32 = 1.0;
const TRIGGER_HEIGHT: f32 = 2.0;
/// Click-to-move only reports the path's end: a pad there triggers once the player has walked
/// to it at about this speed (assumed).
const WALK_SPEED: f32 = 6.0;
const GRAVITY: f32 = 9.8;
const DEFAULT_APEX: f32 = 4.0;
/// "XX_system jumppad lock": no control while flying.
const LOCK_BUFF: u32 = 73527;
/// Launch effects on the pad: mortar (also used for pads with other looks) and wind fan.
const MORTAR_EFFECT: u32 = 367572;
const FAN_EFFECT: u32 = 102631;
/// Landing effect on the player, shortly before touching down.
const LANDING_EFFECT: u32 = 102632;
/// The plain invisible visual of markers and invisible jumpers.
const INVISIBLE_VISUAL: u32 = 95472;
/// Most pads one jump may chain through (the longest route, 6200's PondToSwamp, has 4).
const MAX_HOPS: usize = 8;

#[derive(Debug, Clone, Deserialize)]
struct Pad {
    #[serde(default)]
    name: String,
    pos: [f32; 3],
    rot: [f32; 4],
    /// [resource type, visual id]
    visual: [u32; 2],
    bounds: [f32; 6],
    guid: u32,
    #[serde(default)]
    look: Option<String>,
    /// Where it throws you (a placed bouncer may have none yet: it doesn't launch).
    #[serde(default)]
    target: Option<[f32; 3]>,
    #[serde(default)]
    apex: Option<f32>,
    #[serde(default)]
    target_by: Option<String>,
    /// "JUMP" on pads; the mortars of 4300's cannonball course (K28B) and an idle fan (28EI)
    /// look like pads but are scenery.
    #[serde(default)]
    tag: Option<String>,
}

impl Pad {
    fn id(&self) -> Id32 {
        Id32::new(PAD_TYPE, self.guid)
    }

    /// Whether landing on this pad throws the player on at once: the invisible jumpers (the
    /// clouds in 2210) and the middle stops of named routes (6200's SwampToVolcano1, 2, End;
    /// 1610's jumpie_1_chain_N). Other pads someone lands on fire only after a step off
    /// (5200's catapult pairs land you on the way back). Lily pads float on water, where
    /// nobody can step off (2210's lily crossings).
    fn throws_on(&self) -> bool {
        self.visual[1] == INVISIBLE_VISUAL
            || self.target_by.as_deref() == Some("chain")
            || self.name.contains("chain")
            || self.name.contains("lily")
    }

    fn launches(&self) -> bool {
        self.tag.as_deref() == Some("JUMP") && self.target.is_some()
    }

    /// Heading (radians) of its rotation around the vertical axis.
    fn heading(&self) -> f32 {
        2.0 * self.rot[1].atan2(self.rot[3])
    }

    /// What it is, for people: "Mortar", "Wind fan", ... or its data name.
    fn readable(&self) -> String {
        let look = self.look.as_deref().unwrap_or("");
        let named = [("mortar", "Mortar"), ("wind_fan", "Wind fan"), ("catapult", "Catapult"), ("plane", "Plane")];
        if let Some((_, n)) = named.iter().find(|(k, _)| look.contains(k)) {
            return n.to_string();
        }
        if self.name.contains("lily") {
            "Lily pad".into()
        } else if self.visual[1] == INVISIBLE_VISUAL {
            "Invisible jumper".into()
        } else if self.name.is_empty() {
            "Jumper".into()
        } else {
            self.name.clone()
        }
    }

    /// Cannons (mortars) spin you; every other bouncer bounces you.
    fn tumble(&self) -> Tumble {
        if self.look.as_deref().is_some_and(|l| l.contains("mortar")) { Tumble::SPIN } else { Tumble::BOUNCE }
    }

    fn effect(&self) -> u32 {
        if self.look.as_deref().is_some_and(|l| l.contains("fan")) { FAN_EFFECT } else { MORTAR_EFFECT }
    }
}

#[derive(Deserialize)]
struct World {
    #[serde(default)]
    simple_dynels: Vec<serde_json::Value>,
}

/// The pads of the content tree (bouncers.json).
static BASE: LazyLock<HashMap<u32, Vec<Pad>>> = LazyLock::new(|| {
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
    worlds
        .into_iter()
        .filter_map(|(pf, w)| {
            let w = serde_json::from_value::<World>(w).ok()?;
            let pads = w
                .simple_dynels
                .into_iter()
                .filter(|d| d.get("kind").and_then(|k| k.as_str()) == Some("jump_pad"))
                .filter_map(|d| serde_json::from_value::<Pad>(d).ok())
                .collect();
            Some((pf.parse().ok()?, pads))
        })
        .collect()
});

/// A pad as it stands now: a base pad, or a bouncer placement's.
pub struct Live {
    pad: Pad,
    /// Its survey id: the base pad's (`ids::base_id`) or the placement's.
    pub id: String,
    pub category: Category,
    /// The placement it comes from (surveyed ones).
    pub placement: Option<Placement>,
}

/// The pads by playfield, with the bouncer placements applied ([`set`]).
static PADS: LazyLock<RwLock<HashMap<u32, Vec<Arc<Live>>>>> =
    LazyLock::new(|| RwLock::new(BASE.keys().map(|&pf| (pf, base_pads(pf))).collect()));
static STARTED: AtomicBool = AtomicBool::new(false);
/// Guids of placed pads (a hash of the placement id in this range, so they stay the same).
const PLACED_GUIDS: u32 = 0x00c0_0000;

/// The pads of playfield `playfield` (data: the same in every copy).
fn pads_of(playfield: u32) -> Vec<Arc<Live>> {
    PADS.read().unwrap().get(&playfield).cloned().unwrap_or_default()
}

/// A base pad's category: exact landing points (the recorded worlds) are demo content, the
/// guessed ones guesses (the world editor's to check).
fn base_category(p: &Pad) -> Category {
    if p.target_by.as_deref() == Some("demo") { Category::Demo } else { Category::Guessed }
}

/// Playfield `pf`'s pads as the content has them.
fn base_pads(pf: u32) -> Vec<Arc<Live>> {
    BASE.get(&pf)
        .into_iter()
        .flatten()
        .map(|p| Arc::new(Live { pad: p.clone(), id: sv::ids::base_id(pf, SKind::Bouncer, None, Some(p.visual[1]), p.pos), category: base_category(p), placement: None }))
        .collect()
}

/// A base pad of that visual (its resource type, bounds, look), if any.
fn model_of(visual: u32) -> Option<&'static Pad> {
    let mut all: Vec<(&u32, &Vec<Pad>)> = BASE.iter().collect();
    all.sort_by_key(|(pf, _)| **pf);
    all.into_iter().flat_map(|(_, v)| v).find(|p| p.visual[1] == visual)
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// A pad as the survey server has it from a bouncer record (survey/bouncers.rs).
pub struct RecordPad {
    pub id: String,
    pub category: Category,
    /// The editor's fields (name, heading, comment, feature, tags, author), as a placement.
    pub placement: Placement,
    /// [resource type, visual id]; none: a base pad's of that visual id, else the usual.
    pub visual: Option<[u32; 2]>,
    pub bounds: Option<[f32; 6]>,
    pub guid: Option<u32>,
    pub look: Option<String>,
    pub rot: Option<[f32; 4]>,
    pub apex: Option<f32>,
    pub target_by: Option<String>,
    pub tag: Option<String>,
}

/// The survey server's pads of playfield `pf`, from its bouncer records: they replace the
/// playfield's pads (spawned again where they changed).
pub fn set_records(pf: u32, pads: Vec<RecordPad>) {
    let new: Vec<Arc<Live>> = pads.into_iter().map(|r| Arc::new(live_of(r))).collect();
    let old = PADS.write().unwrap().insert(pf, new.clone()).unwrap_or_default();
    if STARTED.load(Ordering::SeqCst) {
        respawn(Pf::main(pf), &old, &new);
    }
}

/// A record's pad: what the record doesn't say comes from a base pad of its look.
fn live_of(r: RecordPad) -> Live {
    let p = &r.placement;
    let visual_id = r.visual.map(|v| v[1]).or(p.template).unwrap_or(0);
    let model = model_of(visual_id);
    let pad = Pad {
        name: if p.name.is_empty() { model.map(|m| m.name.clone()).unwrap_or_default() } else { p.name.clone() },
        pos: p.pos,
        rot: r.rot.unwrap_or([0.0, (p.heading / 2.0).sin(), 0.0, (p.heading / 2.0).cos()]),
        visual: r.visual.unwrap_or([model.map_or(1_010_201, |m| m.visual[0]), visual_id]),
        bounds: r.bounds.or(model.map(|m| m.bounds)).unwrap_or([-1.5, 0.0, -1.5, 1.5, 0.5, 1.5]),
        guid: r.guid.unwrap_or_else(|| PLACED_GUIDS + (fnv(&r.id) % 0x0010_0000) as u32),
        look: r.look.or_else(|| model.and_then(|m| m.look.clone())),
        target: p.target,
        apex: r.apex,
        target_by: r.target_by,
        tag: r.tag.or(Some("JUMP".into())),
    };
    Live { pad, id: r.id, category: r.category, placement: Some(r.placement) }
}

/// Takes pads that changed or went out of the world and puts new and changed ones in (a changed
/// pad keeps its id: it comes back after the client lets go of it).
fn respawn(playfield: Pf, old: &[Arc<Live>], new: &[Arc<Live>]) {
    let same = |a: &Live, b: &Live| a.pad.guid == b.pad.guid && format!("{:?}", a.pad) == format!("{:?}", b.pad);
    let mut presences = PRESENCES.lock().unwrap();
    for o in old.iter().filter(|o| !new.iter().any(|n| same(n, o))) {
        presences.remove(&o.pad.id());
    }
    for n in new.iter().filter(|n| !old.iter().any(|o| same(n, o))) {
        let reused = old.iter().any(|o| o.pad.guid == n.pad.guid);
        let live = n.clone();
        if !reused {
            presences.insert(live.pad.id(), join(&live, playfield));

            continue;
        }
        tokio::spawn(async move {
            // (the client needs ~1.5 s before an id is used again)
            tokio::time::sleep(Duration::from_secs(2)).await;
            let still = pads_of(playfield.model()).iter().any(|p| Arc::ptr_eq(p, &live));
            if still {
                let presence = join(&live, playfield);
                PRESENCES.lock().unwrap().insert(live.pad.id(), presence);
            }
        });
    }
}

fn join(live: &Arc<Live>, playfield: Pf) -> world::Presence {
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    let p = live.pad.clone();
    let pos = Arc::new(Mutex::new(p.pos));
    world::join(p.id(), playfield, pos, move || spawn_records(&p, playfield), outbox)
}

/// A pad as the world editor lists it.
pub struct Listed {
    pub id: String,
    pub playfield: u32,
    pub category: Category,
    pub name: String,
    /// The visual id (a placement's `template`).
    pub visual: u32,
    pub pos: [f32; 3],
    pub heading: f32,
    pub target: Option<[f32; 3]>,
    /// The hops after the first, when it lands on pads that throw on.
    pub chain: Vec<[f32; 3]>,
    pub placement: Option<Placement>,
    /// Its dynel.
    pub dynel: Id32,
}

fn listed(playfield: u32, l: &Live) -> Listed {
    let p = &l.pad;
    let name = l.placement.as_ref().map(|x| x.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| p.readable());
    Listed {
        id: l.id.clone(),
        playfield,
        category: l.category,
        name,
        visual: p.visual[1],
        pos: p.pos,
        heading: l.placement.as_ref().map_or_else(|| p.heading(), |x| x.heading),
        target: p.target,
        chain: chain(Pf::main(playfield), p),
        placement: l.placement.clone(),
        dynel: p.id(),
    }
}

/// Where a jump from `pad` goes on after its landing point (the pads it lands on that throw on).
fn chain(playfield: Pf, pad: &Pad) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    let (mut at, mut from) = (pad.target, pad.guid);
    while let Some(end) = at {
        if out.len() + 1 >= MAX_HOPS {
            break;
        }
        match pad_at(playfield, end).filter(|p| p.pad.guid != from && p.pad.throws_on()) {
            Some(next) => {
                at = next.pad.target;
                from = next.pad.guid;
                out.extend(next.pad.target);
            }
            None => break,
        }
    }
    out
}

/// The pads of `playfield` (all when none), as the world editor lists them.
pub fn list(playfield: Option<u32>) -> Vec<Listed> {
    let all: Vec<(u32, Vec<Arc<Live>>)> = match playfield {
        Some(pf) => vec![(pf, pads_of(pf))],
        None => PADS.read().unwrap().iter().map(|(pf, v)| (*pf, v.clone())).collect(),
    };
    all.into_iter().flat_map(|(pf, v)| v.into_iter().map(move |l| listed(pf, &l))).collect()
}

/// The pad with survey id `id`.
pub fn find(id: &str) -> Option<Listed> {
    // (listing reads the pads again: not under this lock)
    let found = PADS.read().unwrap().iter().find_map(|(pf, v)| v.iter().find(|l| l.id == id).map(|l| (*pf, l.clone())));
    found.map(|(pf, l)| listed(pf, &l))
}

/// The pad whose dynel is `dynel`.
pub fn by_dynel(dynel: Id32) -> Option<Listed> {
    if dynel.kind != PAD_TYPE {
        return None;
    }
    let found = PADS.read().unwrap().iter().find_map(|(pf, v)| v.iter().find(|l| l.pad.guid == dynel.instance).map(|l| (*pf, l.clone())));
    found.map(|(pf, l)| listed(pf, &l))
}

/// One pad look (visual) as the world editor's catalog has it.
pub struct Look {
    pub visual: u32,
    pub name: String,
    /// Where it is used most ("Pirate World ×12, 30 in all").
    pub used: String,
}

/// Every pad look of the base content, by name.
pub fn looks() -> Vec<Look> {
    let mut seen: BTreeMap<u32, (&Pad, BTreeMap<u32, u32>)> = BTreeMap::new();
    for (pf, pads) in BASE.iter() {
        for p in pads {
            let e = seen.entry(p.visual[1]).or_insert_with(|| (p, BTreeMap::new()));
            *e.1.entry(*pf).or_default() += 1;
        }
    }
    let mut out: Vec<Look> = seen
        .into_iter()
        .map(|(visual, (p, uses))| {
            let total: u32 = uses.values().sum();
            let (top, n) = uses.iter().max_by_key(|(_, n)| **n).map(|(pf, n)| (*pf, *n)).unwrap_or((0, 0));
            let place = crate::teleport::destination(top).map_or_else(|| top.to_string(), |d| d.name.clone());
            let used = if uses.len() > 1 { format!("{place} ×{n}, {total} in all") } else { format!("{place} ×{n}") };
            // "jump_pad_wind_fan_b_buildie": Wind fan (b); pads without a look: by their visual
            let look = p.look.as_deref().unwrap_or("");
            let variant = ["_a_", "_b_"].iter().find(|v| look.contains(*v)).map(|v| v.trim_matches('_'));
            let readable = if p.look.is_some() || p.name.contains("lily") || visual == INVISIBLE_VISUAL { p.readable() } else { "Jumper".into() };
            let name = match (variant, look.ends_with("_idle"), p.look.is_some()) {
                (_, true, _) => format!("{readable} (idle)"),
                (Some(v), _, _) => format!("{readable} ({v})"),
                (None, _, false) if readable == "Jumper" => format!("Jumper ({visual})"),
                _ => readable,
            };
            Look { visual, name, used }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// What a player is doing with pads.
#[derive(Default)]
struct State {
    /// Flying until then.
    airborne_until: Option<Instant>,
    /// The pad the player stands on and that already fired (or that they landed on): it fires
    /// again only after they step off.
    on: Option<u32>,
    /// Bumped on every movement report; a pending (walk-there) launch is dropped if it changed.
    moves: u64,
}

static STATES: LazyLock<Mutex<HashMap<Id32, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static PRESENCES: LazyLock<Mutex<HashMap<Id32, world::Presence>>> = LazyLock::new(Default::default);

/// Puts every pad into its world.
pub fn start() {
    let all: Vec<(u32, Vec<Arc<Live>>)> = PADS.read().unwrap().iter().map(|(pf, v)| (*pf, v.clone())).collect();
    let mut presences = PRESENCES.lock().unwrap();
    for (playfield, pads) in all {
        for live in pads {
            presences.insert(live.pad.id(), join(&live, Pf::main(playfield)));

        }
    }
    STARTED.store(true, Ordering::SeqCst);
    info!(count = presences.len(), "jump pads spawned");
}

/// UpdateMessage + `SimpleDynelFullUpdateIIR_c` (ProtoGameCodeDynelIIR, ProtoSimpleDynelIIR,
/// VisualData: visual id, resource type, no particles, bounds, scale, u32, u8), then CarsGroup;
/// matches the recorded session's pads.
fn spawn_records(pad: &Pad, playfield: Pf) -> Vec<Bytes> {
    let id = pad.id();
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pad.pos[0]), y: Some(pad.pos[1]), z: Some(pad.pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(pad.rot[0]), y: Some(pad.rot[1]), z: Some(pad.rot[2]), w: Some(pad.rot[3]) }),
        playfield_instance: Some(playfield.key()),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(0),
        name: Some("Jumper".into()),
        cars_group: Some(CARS_GROUP),
        template_item_id: Some(PAD_TEMPLATE),
        is_closed: Some(true),
        client_script_id: Some(PAD_CLIENT_SCRIPT),
        ..Default::default()
    };
    let record = iir::record("SimpleDynelFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(pad.visual[1]).u32(pad.visual[0]).u32(0);
        pad.bounds.iter().fold(w, |w, v| w.f32(*v)).f32(1.0).u32(0).u8(0)
    });
    let mut records = vec![create_game_object(id, playfield, pad.pos), record, int32_stat(id, STAT_CARS_GROUP, cars_group(pad, playfield))];
    if crate::buildie::unbuilt_at(playfield, pad.pos) {
        // live: the pad under a buildie appears once the buildie is built (recorded: the wind
        // propeller hidden, jumper 602 shown)
        records.push(show_record(id, false));
    }
    records
}

/// `UpdateSimpleDynelIIR_c` as recorded: u32 0, u8 hidden, u8 1, u8 0, u8 1.
fn show_record(id: Id32, shown: bool) -> Bytes {
    iir::record("UpdateSimpleDynelIIR_c", id, false, |w| w.u32(0).u8(u8::from(!shown)).u8(1).u8(0).u8(1))
}

/// Whether a jump pad stands at `at` (under a buildie: a bouncer).
pub fn pad_at_spot(playfield: Pf, at: [f32; 3]) -> bool {
    pads_of(playfield.model()).iter().any(|l| ((l.pad.pos[0] - at[0]).powi(2) + (l.pad.pos[2] - at[2]).powi(2)).sqrt() < 1.0)
}

/// Pads under an unbuilt buildie are inactive (CARS group 5, as recorded) until it is built.
fn cars_group(pad: &Pad, playfield: Pf) -> u32 {
    if !pad.launches() || crate::buildie::unbuilt_at(playfield, pad.pos) { CARS_INACTIVE } else { CARS_GROUP }
}

/// Every pad, for the test commands (devcmd.rs): (name, playfield, position).
/// Where pad `guid` stands.
pub fn pad_pos(playfield: Pf, guid: u32) -> Option<[f32; 3]> {
    pads_of(playfield.model()).iter().find(|l| l.pad.guid == guid).map(|l| l.pad.pos)
}

pub fn points() -> Vec<(String, u32, [f32; 3])> {
    let mut v: Vec<_> = PADS
        .read()
        .unwrap()
        .iter()
        .flat_map(|(&pf, pads)| pads.iter().map(move |l| (l.pad.look.clone().unwrap_or_else(|| "pad".into()), pf, l.pad.pos)).collect::<Vec<_>>())
        .collect();
    v.sort_by_key(|p| p.1);
    v
}

/// A working pad at `at`, the nearest if several (6200's pond routes cross: SwampToPond3 and
/// PondToSwamp2 stand 0.8 m apart).
fn pad_at(playfield: Pf, at: [f32; 3]) -> Option<Arc<Live>> {
    let flat = |p: &Pad| ((p.pos[0] - at[0]).powi(2) + (p.pos[2] - at[2]).powi(2)).sqrt();
    pads_of(playfield.model())
        .into_iter()
        .filter(|l| flat(&l.pad) <= TRIGGER_RADIUS && (l.pad.pos[1] - at[1]).abs() <= TRIGGER_HEIGHT)
        .min_by(|a, b| flat(&a.pad).total_cmp(&flat(&b.pad)))
        .filter(|l| l.pad.launches() && !crate::buildie::unbuilt_at(playfield, l.pad.pos))
}

/// The buildie at `at` was built: the pad there starts working, and whoever stands on it (the
/// builder, usually) is launched right away rather than at their next step.
pub fn activate(playfield: Pf, at: [f32; 3]) {
    for live in pads_of(playfield.model()).into_iter().filter(|l| ((l.pad.pos[0] - at[0]).powi(2) + (l.pad.pos[2] - at[2]).powi(2)).sqrt() < 1.0) {
        let pad = &live.pad;
        let records = vec![show_record(pad.id(), true), int32_stat(pad.id(), STAT_CARS_GROUP, CARS_GROUP)];
        world::send_playfield(pad.id(), false, world::Out::Records(records));
        for (player, pos) in world::position_handles(playfield) {
            let here = *pos.lock().unwrap();
            if !crate::npc::is_player(player) || pad_at(playfield, here).is_none_or(|p| p.pad.guid != pad.guid) {
                continue;
            }
            STATES.lock().unwrap().entry(player).or_default().on = Some(pad.guid);
            info!(?player, pad = pad.guid, "launched on the pad as it was built");
            tokio::spawn(launch(player, playfield, live.clone(), pos));
        }
    }
}

/// `UpdateSimpleDynelIIR_c` showing or hiding a pad and its CARS group, for one player.
pub fn pad_records(guid: u32, shown: bool) -> Vec<Bytes> {
    let id = Id32::new(PAD_TYPE, guid);
    vec![show_record(id, shown), int32_stat(id, STAT_CARS_GROUP, if shown { CARS_GROUP } else { CARS_INACTIVE })]
}

/// The player reported being at `at` (`from`: where a click-to-move path started, `at` being
/// its end). Launches them if that is on a pad.
pub fn moved(player: Id32, playfield: Pf, pos: &world::Position, at: [f32; 3], from: Option<[f32; 3]>) {
    // (the tutorial keeps a pad shut until the player beat the wave before it)
    let pad = pad_at(playfield, at).filter(|p| crate::tutorial::pad_open(player, playfield, p.pad.guid));
    let moves = {
        let mut states = STATES.lock().unwrap();
        let s = states.entry(player).or_default();
        s.moves += 1;
        if s.airborne_until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        s.airborne_until = None;
        let id = pad.as_ref().map(|p| p.pad.guid);
        if id.is_none() || s.on == id {
            s.on = id;
            return;
        }
        if from.is_none() {
            s.on = id;
        }
        s.moves
    };
    let Some(pad) = pad else { return };
    let walk = from.map_or(0.0, |f| ((at[0] - f[0]).powi(2) + (at[2] - f[2]).powi(2)).sqrt() / WALK_SPEED);
    let pos = pos.clone();
    tokio::spawn(async move {
        if walk > 0.0 {
            tokio::time::sleep(Duration::from_secs_f32(walk)).await;
            let mut states = STATES.lock().unwrap();
            let s = states.entry(player).or_default();
            if s.moves != moves {
                return;
            }
            s.on = Some(pad.pad.guid);
        }
        launch(player, playfield, pad, pos).await;
    });
}

/// Forgets a player who left.
pub fn player_left(player: Id32) {
    STATES.lock().unwrap().remove(&player);
}

/// The jump, and on through every pad it lands on that throws on (`Pad::throws_on`).
async fn launch(player: Id32, playfield: Pf, mut pad: Arc<Live>, pos: world::Position) {
    for _ in 0..MAX_HOPS {
        let Some(end) = hop(player, &pad.pad, &pos).await else { break };
        let next = pad_at(playfield, end).filter(|p| p.pad.guid != pad.pad.guid);
        if let Some(s) = STATES.lock().unwrap().get_mut(&player) {
            // landing on another pad doesn't fire it until the player steps off
            s.on = next.as_ref().map(|p| p.pad.guid);
        }
        match next {
            Some(p) if p.pad.throws_on() => pad = p,
            _ => break,
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    world::send_playfield(player, true, world::Out::Records(vec![effects::buff_remove(player, LOCK_BUFF, Id32::default())]));
}

/// One jump: arc maths from the recorded jumps (all seven fit exactly): the arc peaks `apex`
/// above the higher of start and end; T = time up + time down. Returns where it landed.
async fn hop(player: Id32, pad: &Pad, pos: &world::Position) -> Option<[f32; 3]> {
    let target = pad.target?;
    info!(?player, pad = pad.guid, end = ?target, "jump pad");
    let how = pad.tumble();
    world::send_playfield(player, true, world::Out::Records(how.start_records(player)));
    let end = fly(player, pos, target, pad.apex.unwrap_or(DEFAULT_APEX), Some(effects::effect_package(pad.id(), pad.effect())), how.direction).await;
    world::send_playfield(player, true, world::Out::Records(how.end_records(player)));
    Some(end)
}

/// Throws `player` (as a jump pad does, without a pad: pistons.rs) to `end`, arcing `apex` m
/// above the higher end, spinning as a cannon throws; returns when they land. The lock buff is
/// the caller's to remove.
pub async fn throw(player: Id32, pos: &world::Position, end: [f32; 3], apex: f32) {
    throw_as(player, pos, end, apex, Tumble::SPIN).await
}

/// How a thrown character moves in the air: the BCT substate "Knockbacked" (24) sets the
/// minifigure graph's `knockback` (its `launch` motion, spinning; client: EmotionFXCharacter::
/// SetBCTSubstate), effect package 623093 its `launched` flag (the bounce; the landing effect
/// 102632 clears it), and `direction` is the flight's KnockawayDirection. Both at once made
/// the graph restart the motion over and over.
#[derive(Clone, Copy, Debug)]
pub struct Tumble {
    pub knockback: bool,
    pub launched: bool,
    pub direction: u8,
}

impl Tumble {
    /// Bouncers (the user, 2026-10-26).
    pub const BOUNCE: Tumble = Tumble { knockback: false, launched: true, direction: 0 };
    /// Cannons (the mortar pads) and the Gauntlet's pistons.
    pub const SPIN: Tumble = Tumble { knockback: true, launched: false, direction: 0 };

    fn start_records(self, player: Id32) -> Vec<Bytes> {
        let mut records = Vec::new();
        if self.knockback {
            records.push(knockback_record(player, BCC_ENTER_SUBSTATE));
        }
        if self.launched {
            records.push(effects::effect_package(player, LAUNCHED_EFFECT));
        }
        records
    }

    fn end_records(self, player: Id32) -> Vec<Bytes> {
        if self.knockback { vec![knockback_record(player, BCC_EXIT_SUBSTATE)] } else { Vec::new() }
    }
}

/// `throw` with the animation `how` (`!bounce`).
pub async fn throw_as(player: Id32, pos: &world::Position, end: [f32; 3], apex: f32, how: Tumble) {
    world::send_playfield(player, true, world::Out::Records(how.start_records(player)));
    fly(player, pos, end, apex, None, how.direction).await;
    world::send_playfield(player, true, world::Out::Records(how.end_records(player)));
    tokio::time::sleep(Duration::from_millis(300)).await;
    world::send_playfield(player, true, world::Out::Records(vec![effects::buff_remove(player, LOCK_BUFF, Id32::default())]));
}

/// The flight: arc maths from the recorded jumps; `launch` is the pad's launch effect.
async fn fly(player: Id32, pos: &world::Position, end: [f32; 3], apex: f32, launch: Option<Bytes>, direction: u8) -> [f32; 3] {
    let start = *pos.lock().unwrap();
    let top = start[1].max(end[1]) + apex;
    let up = (2.0 * (top - start[1]) / GRAVITY).sqrt();
    let down = (2.0 * (top - end[1]) / GRAVITY).sqrt();
    let t = up + down;
    let vy = GRAVITY * up;
    let (dx, dz) = (end[0] - start[0], end[2] - start[2]);
    let h = (dx * dx + dz * dz).sqrt().max(0.001);
    let dir = [dx / h, dz / h];
    {
        let mut states = STATES.lock().unwrap();
        states.entry(player).or_default().airborne_until = Some(Instant::now() + Duration::from_secs_f32(t + 0.3));
    }
    // `ParameterizedBallisticMoveTask` (12): u8 0, start, u8 12, u8 0, f32 T, f32 T, f32 0,
    // start, end, horizontal direction, u8 0, u8 0, f32 horizontal speed, f32 vertical speed,
    // u8 1, u8 1
    let flight = iir::record("MoveTaskIIR_c", player, false, |w| {
        let w = w.u8(0).f32(start[0]).f32(start[1]).f32(start[2]).u8(12).u8(0).f32(t).f32(t).f32(0.0);
        let w = [start, end, [dir[0], 0.0, dir[1]]].iter().flatten().fold(w, |w, v| w.f32(*v));
        w.u8(0).u8(direction).f32(h / t).f32(vy).u8(1).u8(1)
    });
    let mut records: Vec<Bytes> = launch.into_iter().collect();
    records.extend([stop_record(player, start, dir), flight, effects::buff_add(player, LOCK_BUFF, Id32::default(), 1500)]);
    world::send_playfield(player, true, world::Out::Records(records));

    tokio::time::sleep(Duration::from_secs_f32((t - 0.2).max(0.0))).await;
    world::send_playfield(player, true, world::Out::Records(vec![effects::effect_package(player, LANDING_EFFECT)]));
    tokio::time::sleep(Duration::from_secs_f32(t.min(0.2))).await;
    *pos.lock().unwrap() = end;
    world::send_playfield(player, true, world::Out::Records(vec![stop_record(player, end, dir)]));
    end
}

const KNOCKBACKED: u32 = 24;
const LAUNCHED_EFFECT: u32 = 623093;
const BCC_ENTER_SUBSTATE: u8 = 11;
const BCC_EXIT_SUBSTATE: u8 = 12;

/// `BehaviorControlCenterIIR_c` action 11/12 (enter/exit substate) with substate 24, as dash.rs.
fn knockback_record(id: Id32, action: u8) -> Bytes {
    iir::record("BehaviorControlCenterIIR_c", id, false, |w| w.u8(action).u32(KNOCKBACKED))
}

/// `StopIIR_c`: position, rotation quaternion (yaw towards `dir`), u8 0, u8 0.
fn stop_record(id: Id32, at: [f32; 3], dir: [f32; 2]) -> Bytes {
    let half = dir[0].atan2(dir[1]) / 2.0;
    iir::record("StopIIR_c", id, false, |w| {
        w.f32(at[0]).f32(at[1]).f32(at[2]).f32(0.0).f32(half.sin()).f32(0.0).f32(half.cos()).u8(0).u8(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bouncer(id: &str, pos: [f32; 3], target: Option<[f32; 3]>) -> Placement {
        Placement {
            id: id.into(),
            kind: SKind::Bouncer,
            name: String::new(),
            monster_data: None,
            template: Some(212_668),
            level: None,
            pos,
            heading: 0.0,
            target,
            replaces: None,
            removed: false,
            wave: None,
            patrol: Vec::new(),
            comment: String::new(),
            tags: Vec::new(),
            feature: sv::features::live(),
            author: String::new(),
            updated: 0,
        }
    }

    #[test]
    fn base_pads_and_record_pads() {
        let base = base_pads(6200);
        let first = base.iter().find(|l| l.pad.name == "SwampToVolcano1").unwrap();
        assert_eq!(first.category, Category::Guessed);
        // its chain: on through SwampToVolcano2's landing point
        let l = listed(6200, first);
        assert_eq!(l.target, Some([73.22, 56.95, 159.13]));
        assert_eq!(l.chain, [[59.64, 78.0, 185.13]]);
        // a placed pad (a record without the client's details): a mortar's look, its own guid
        let rec = |p: Placement| RecordPad { id: p.id.clone(), category: Category::Surveyed, placement: p, visual: None, bounds: None, guid: None, look: None, rot: None, apex: None, target_by: None, tag: None };
        let placed = live_of(rec(bouncer("bouncer/6200/new", [10.0, 0.0, 10.0], None)));
        assert!(!placed.pad.launches(), "no landing point yet");
        assert_eq!(placed.pad.readable(), "Mortar");
        assert!(placed.pad.guid >= PLACED_GUIDS);
        assert!(live_of(rec(bouncer("bouncer/6200/x", [0.0; 3], Some([1.0, 2.0, 3.0])))).pad.launches());
        // the recorded worlds' pads are exact (demo)
        assert!(base_pads(1300).iter().all(|l| l.category == Category::Demo));
    }

    #[test]
    fn looks_are_named() {
        let names: Vec<String> = looks().into_iter().map(|l| l.name).collect();
        for n in ["Mortar (a)", "Wind fan (b)", "Wind fan (idle)", "Plane (a)", "Catapult (b)", "Lily pad", "Invisible jumper"] {
            assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
        }
    }
}
