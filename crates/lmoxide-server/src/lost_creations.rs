//! The Lost Creations (docs/LOST-CREATIONS.md): a team's run through random levels, one copy of
//! a level each (instance.rs), against the clock, until they stop or fail; then the Treasure
//! Chamber and its rewards. The levels are content `lc` records, the run's numbers tunables
//! `lost_creations`.
//!
//! The flow, as the live capture (a solo Funhouse run on the same machinery) and the client have
//! it, with the user's rules where the capture has nothing (teams, time-out, wipe, rewards):
//! - the door in Dinosaur Rise opens the "Lost Creations" window (`ScryShowEndless...` type 0);
//!   Enter asks the teammates (type 2: Join / Cancel), one Cancel or no answer aborts it for
//!   everyone (`SignalEndlessAborted`); with everyone in, the team goes to a new copy of a level;
//! - in it the start gate is red and the button has an arrow; any member presses it: "Ready to
//!   start?" (`SignalOpenEndlessWarmupDialog`), Start (`StartEndlessWarmup`) counts 5 s down for
//!   everyone (`SignalStartEndlessWarmup`), then the gate turns green and the timer buff (the
//!   HUD's clock) runs; no equipping from then on (CantEquipWeapons), no rebuilds all run
//!   (CantResurrectWeapons);
//! - Survival and the Gatekeeper's Realm run the level's script (scripts.rs) from then on: the
//!   copy's variant's enemies come in waves on the level's points, each wave after the points
//!   showed the spawn particle; each wave cleared counts toward the objective, the script over
//!   completes it (kills don't count: a Gatekeeper's summoned adds never do);
//! - the objective done, the quest's next step is "reach the end" and the end gate opens
//!   (Mayhems: open all along); crossing `End` completes it for that player (the timer and
//!   CantEquip go);
//! - with everyone across, the exit door asks each one Continue / End Run (types 1 and 3); End
//!   Run takes only that player out, the others go on to the next level;
//! - time up, a player's whole team smashed, End Run or the HUD's quit: that player goes to the
//!   Treasure Chamber, out of the team (the client won't leave a team in here), every minifigure
//!   rebuilt, with a bag of minifigure parts for the levels done.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use lmoxide_content::{LcLevel, LcMode, Shape};
use lmoxide_net::{
    Endpoint, Writer, iir,
    methods::{game_char_interface, game_char_interface_project as project},
    proto::{PlayfieldProxyData, ProtoGameCodeDynelIir, ProtoQuat, ProtoSimpleDynelIir, ProtoVec3},
    types::Id32,
};
use tracing::{info, warn};

use crate::achievements::Event;
use crate::instance::{Mode, Pf};
use crate::world;

/// The windows' texts (EndlessClientData RDB 1000647 id 1: "Lost Creations", "How far can you
/// go?", the splashes); 654716 is the Funhouse's.
const CLIENT_DATA: u32 = 1;
/// `ScryShowEndlessPlayfieldsTeleportMessageBox` types.
const WINDOW_ENTRY: u32 = 0;
const WINDOW_CONTINUE: u32 = 1;
const WINDOW_INVITE: u32 = 2;
const WINDOW_CONTINUE_TEAMMATE: u32 = 3;
/// How long the run-long buffs last (live: 2,000,000 cs, about 5.5 h).
const RUN_BUFF_CS: i32 = 2_000_000;
/// The timer buff is added with this, then updated to the level's time (live: 200 s, 300 s).
const TIMER_ADD_CS: i32 = 20_000;
/// CantEquipWeapons came this long after the gate opened (live).
const CANT_EQUIP_AFTER: Duration = Duration::from_secs(1);
/// A player stat live set with CantEquipWeapons (0x10000) and cleared with it (meaning unknown).
const STAT_EQUIP_FLAGS: u32 = 864;
const EQUIP_LOCKED: u32 = 0x1_0000;
/// The effect live played on a player crossing the end.
const LEVEL_DONE_EFFECT: u32 = 549_339;
/// The gates' visuals (the client's `GateStartVisual`): open (green), closed (red); resource type.
const GATE_OPEN: u32 = 155_858;
const GATE_CLOSED: u32 = 155_861;
const PLAIN_VISUAL: u32 = 1_010_030;
/// SimpleDynelVisualChangeIIR_c states: 3 the visual (with its resource type), 4 the collision.
const VISUAL_STATE: u32 = 3;
const STAT_VISUAL: u32 = 12;
/// The button as live named it ("XX_Interactable", TDB 50200).
const BUTTON_NAME: u32 = BUTTON_TEMPLATE;
/// Its arrow's carrier ("XX_InvisibleDummy").
const DUMMY_LOOK: u32 = 51_057;
const DUMMY_TEMPLATE: u32 = 1961;
/// Usable (ObjectInteraction), not usable.
const CARS_USABLE: u32 = 1;
const CARS_INACTIVE: u32 = 5;
const STAT_CARS_GROUP: u32 = 1050;
/// How far above or below the end area's heights still counts as inside.
const HEIGHT_SLACK: f32 = 3.0;
/// Spawns this close (m) stand on the same point.
const POINT_SAME: f32 = 0.3;
/// The points' dynels: base instances from here on (above any client guid).
const POINT_BASE: u32 = 0x00f0_0000;
/// After "Ready to start?", the button doesn't ask again for this long (unless Start was pressed).
const WARMUP_DIALOG_HOLD: Duration = Duration::from_secs(10);
/// A minifigure smashed in a run stays smashed this long, until the run is over for the player
/// (game.rs; as long as live's CantResurrectWeapons).
pub const NO_REBUILD: Duration = Duration::from_secs(RUN_BUFF_CS as u64 / 100);
/// Seconds as the windows count them.
fn secs(d: Duration) -> u32 {
    d.as_secs() as u32
}

fn tunables() -> &'static lmoxide_content::tunables::LostCreations {
    &crate::gamedata::get().tunables.lost_creations
}

fn duration(d: lmoxide_content::Duration) -> Duration {
    Duration::from_secs_f64(d.secs().max(0.0))
}

fn player(character: u32) -> Id32 {
    Id32::new(crate::territory::CHARACTER_TYPE, character)
}

// ------------------------------------------------------------------ levels

/// A simple dynel or door of a level (its client placement).
#[derive(Clone, Debug)]
struct Piece {
    guid: u32,
    name: String,
    pos: [f32; 3],
    rot: [f32; 4],
    scale: f32,
    visual: [u32; 2],
    bounds: [f32; 6],
}

/// A level as the server runs it.
struct Level {
    id: String,
    name: String,
    model: u32,
    mode: LcMode,
    start: ([f32; 3], f32),
    start_shield: Option<Piece>,
    start_visual: Piece,
    exit_shield: Option<Piece>,
    exit_visual: Piece,
    exit_open: bool,
    button: Piece,
    door: Piece,
    quest: crate::quests::Scripted,
    end: Shape,
    variants: Vec<lmoxide_content::LcVariant>,
    /// The waves' script, and the points its spawns stand on (each shows the spawn particle
    /// before its wave).
    script: Option<String>,
    points: Vec<[f32; 3]>,
    /// The variants of each of the script's spawns (empty: in every one).
    wave_spawns: Vec<Vec<String>>,
}

impl Level {
    /// Whether the script brings any enemy in a copy running `variant`.
    fn has_waves(&self, variant: Option<&String>) -> bool {
        self.wave_spawns.iter().any(|v| v.is_empty() || variant.is_some_and(|x| v.contains(x)))
    }
}

static LEVELS: LazyLock<Vec<Level>> = LazyLock::new(|| {
    let c = crate::gamedata::get();
    let mut levels: Vec<Level> = c.lc_levels.values().filter_map(|l| level_of_record(c, l).map_err(|e| warn!(level = %l.id, "Lost Creations level left out: {e}")).ok()).collect();
    levels.sort_by(|a, b| a.id.cmp(&b.id));
    levels
});

fn level_of_record(c: &lmoxide_content::Content, l: &LcLevel) -> Result<Level, String> {
    let world = c.worlds.get(&l.world).ok_or("no such world")?;
    let piece = |guid: u32| -> Result<Piece, String> {
        let s = c.in_world(&c.spawns, &l.world).find(|s| s.client.as_ref().and_then(|cl| cl.guid) == Some(guid)).ok_or(format!("no spawn with guid {guid}"))?;
        let cl = s.client.as_ref().unwrap();
        let rot = match s.rot.unwrap_or_default() {
            lmoxide_content::Rot::Quat(q) => q,
            lmoxide_content::Rot::Heading(h) => [0.0, (h / 2.0).sin(), 0.0, (h / 2.0).cos()],
        };
        Ok(Piece {
            guid,
            name: s.name.clone(),
            pos: s.pos,
            rot,
            scale: s.scale,
            visual: cl.visual.unwrap_or([PLAIN_VISUAL, 0]),
            bounds: cl.bounds.unwrap_or([-1.0, 0.0, -1.0, 1.0, 2.0, 1.0]),
        })
    };
    let end = c.areas.get(&l.end_area).and_then(|a| a.shape.clone()).ok_or("no end area")?;
    let mut points: Vec<[f32; 3]> = Vec::new();
    let mut wave_spawns = Vec::new();
    if let Some(script) = &l.script {
        c.scripts.get(script).ok_or(format!("no script {script}"))?;
        for s in c.in_world(&c.spawns, &l.world).filter(|s| !s.removed && s.wave.as_ref().is_some_and(|w| &w.script == script)) {
            wave_spawns.push(s.variants.clone());
            if !points.iter().any(|p| (p[0] - s.pos[0]).hypot(p[2] - s.pos[2]) < POINT_SAME) {
                points.push(s.pos);
            }
        }
    }
    let quest = crate::quests::scripted(&l.quest, world.playfield).ok_or("no such quest")?;
    Ok(Level {
        id: l.id.clone(),
        name: if l.name.is_empty() { world.name.clone() } else { l.name.clone() },
        model: world.playfield,
        mode: l.mode,
        start: (l.start.pos, l.start.rot.map_or(0.0, |r| r.heading())),
        start_shield: l.start_gate.shield.map(piece).transpose()?,
        start_visual: piece(l.start_gate.visual)?,
        exit_shield: l.exit_gate.shield.map(piece).transpose()?,
        exit_visual: piece(l.exit_gate.visual)?,
        exit_open: l.exit_gate.open,
        button: piece(l.button)?,
        door: piece(l.exit_door)?,
        quest,
        end,
        variants: l.variants.clone(),
        script: l.script.clone(),
        points,
        wave_spawns,
    })
}

fn level(i: usize) -> &'static Level {
    &LEVELS[i]
}

fn level_of_model(model: u32) -> Option<usize> {
    LEVELS.iter().position(|l| l.model == model)
}

/// Whether dynel `kind` with base guid `guid` in playfield `model` is one the Lost Creations
/// answer when used: the entry door, a level's start button or exit door.
pub fn is_fixture(model: u32, kind: u32, guid: u32) -> bool {
    match kind {
        crate::teleport::DOOR_TYPE => is_entry_door(model, guid) || level_of_model(model).is_some_and(|i| level(i).door.guid == guid),
        crate::scenery::SIMPLE_DYNEL_TYPE => level_of_model(model).is_some_and(|i| level(i).button.guid == guid),
        _ => false,
    }
}

/// Whether playfield `model` is a Lost Creations level.
pub fn is_level(model: u32) -> bool {
    level_of_model(model).is_some()
}

/// Reads the levels (at start, so problems show in the log).
pub fn start() {
    info!(levels = LEVELS.len(), "Lost Creations levels read");
}

// ------------------------------------------------------------------ state

/// What a window a player has open asks.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ask {
    /// The entry door's window.
    Entry,
    /// A teammate's invitation (the leader).
    Invite(u32),
    /// Continue / End Run at the exit (the run).
    Continue(u32),
}

/// A team being asked to come along (before the run exists).
struct Gathering {
    mates: Vec<u32>,
    joined: HashSet<u32>,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Gates closed, the button waits.
    Waiting,
    /// The 5 s before the gate opens.
    Countdown,
    /// The timer runs.
    Running,
    /// Everyone left is across the end (the timer stopped).
    Done,
}

struct Run {
    id: u32,
    mode: Mode,
    members: Vec<u32>,
    /// The level the team is on (1, 2, ...).
    level: u32,
    /// Levels each member completed in this run.
    done: HashMap<u32, u32>,
    current: usize,
    pf: Pf,
    element: u32,
    phase: Phase,
    /// The quest step and its count.
    step: usize,
    progress: u32,
    exit_open: bool,
    finished: HashSet<u32>,
    /// The continue question: who answered Continue.
    asked: bool,
    continues: HashSet<u32>,
    started: Instant,
    deadline: Option<Instant>,
    /// Raised by every change that ends a timer's business (a new level, the timer stopped).
    generation: u64,
}

#[derive(Default)]
struct State {
    runs: HashMap<u32, Run>,
    run_of: HashMap<u32, u32>,
    copies: HashMap<Pf, u32>,
    gathering: HashMap<u32, Gathering>,
    asks: HashMap<u32, (i32, Ask)>,
    next_ctx: i32,
    generation: u64,
    /// Levels done in a run a player left while offline (their rewards on return).
    pending: HashMap<u32, u32>,
    /// Members who lost their connection, still in their run until `instance::RETURN_WINDOW`
    /// passes (a reconnect puts them back into the level's copy): their mark.
    away: HashMap<u32, u64>,
    /// When each player last got "Ready to start?" (the client stacks a new one on top of an
    /// open one, and the stacked ones can't be closed by their X).
    warmup_asked: HashMap<u32, Instant>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

/// What other modules ask about the runs (who is in one, a level copy's proxy fields), kept apart
/// from `STATE` and only ever locked last: code holding `STATE` calls community.rs, whose lock
/// is held while it asks these.
#[derive(Default)]
struct Info {
    members: HashSet<u32>,
    /// A run's level copy: (level, run id, team size, element).
    copies: HashMap<Pf, (u32, u32, u32, u32)>,
}

static INFO: LazyLock<Mutex<Info>> = LazyLock::new(Default::default);

/// `run`'s current copy and members for `INFO`.
fn publish(run: &Run) {
    let mut info = INFO.lock().unwrap();
    info.copies.insert(run.pf, (run.level, run.id, run.members.len() as u32, run.element));
    info.members.extend(run.members.iter().copied());
}

impl State {
    fn ctx(&mut self) -> i32 {
        self.next_ctx = self.next_ctx.wrapping_add(1).max(1);
        self.next_ctx
    }

    fn generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    fn run_of(&mut self, character: u32) -> Option<&mut Run> {
        let id = *self.run_of.get(&character)?;
        self.runs.get_mut(&id)
    }
}

/// Whether `character` is in a run.
pub fn in_run(character: u32) -> bool {
    INFO.lock().unwrap().members.contains(&character)
}

/// Whether `character` may not equip minifigures now (from the start gate opening until they
/// cross the end).
pub fn cant_equip(character: u32) -> bool {
    let mut st = STATE.lock().unwrap();
    st.run_of(character).is_some_and(|r| r.phase == Phase::Running && !r.finished.contains(&character))
}

/// The element of copy `pf` if it is a run's level (the run's).
pub fn element(pf: Pf) -> Option<u32> {
    INFO.lock().unwrap().copies.get(&pf).map(|c| c.3)
}

/// The run's fields of a level's PlayfieldProxyData (`endless_progress` makes it an Endless
/// playfield for the client: the HUD's badge and quit button, the endless quest icon, the death
/// splash; `client_protobuf_resource_id` names the texts).
pub fn proxy(pf: Pf, mut proxy: PlayfieldProxyData) -> PlayfieldProxyData {
    if let Some(&(level, run, team, element)) = INFO.lock().unwrap().copies.get(&pf) {
        proxy.endless_progress = Some(level);
        proxy.identifier = Some(run);
        proxy.team_size = Some(team);
        proxy.elemental_type = Some(element);
        proxy.client_protobuf_resource_id = Some(CLIENT_DATA);
    }
    proxy
}

// ------------------------------------------------------------------ sending

fn rpc(character: u32, endpoint: Endpoint, method: u32, args: Bytes) {
    crate::sessions::send(character, world::Out::Rpc(endpoint, method, args));
}

fn records(character: u32, records: Vec<Bytes>) {
    if !records.is_empty() {
        crate::sessions::send(character, world::Out::Records(records));
    }
}

/// To every player in copy `pf`.
fn to_copy(pf: Pf, recs: Vec<Bytes>) {
    for (id, _) in world::positions(pf).into_iter().filter(|(id, _)| crate::npc::is_player(*id)) {
        world::send_to(id, world::Out::Records(recs.clone()));
    }
}

fn order(character: u32, o: Order) {
    crate::sessions::send(character, world::Out::Lc(o));
}

/// `ScryShowEndlessPlayfieldsTeleportMessageBox(u32 type, u32 required level, String quest,
/// i32 ctx, u32 time left, u32 client data)`. The entry window names the quest it requires (NUL
/// terminated: live's Funhouse window, which required none, sent just the NUL), the others an
/// empty string.
fn window(character: u32, kind: u32, ctx: i32, time_left: u32) {
    let w = Writer::new().u32(kind).u32(0);
    let w = if kind == WINDOW_ENTRY {
        let mut quest = tunables().requirement_quest.as_bytes().to_vec();
        quest.push(0);
        w.string_bytes(&quest)
    } else {
        w.u16(0)
    };
    rpc(character, Endpoint::GameCharInterfaceProject, project::SCRY_SHOW_ENDLESS_PLAYFIELDS_TELEPORT_MESSAGE_BOX, w.i32(ctx).u32(time_left).u32(CLIENT_DATA).finish());
}

fn close_window(character: u32) {
    rpc(character, Endpoint::GameCharInterfaceProject, project::SCRY_CLOSE_ENDLESS_PLAYFIELDS_TELEPORT_MESSAGE_BOX, Bytes::new());
}

/// What a player's connection does for a run (game.rs).
#[derive(Clone, Debug)]
pub enum Order {
    /// Into a level's copy, at its start.
    Enter(Pf, [f32; 3], f32),
    /// Out of the run: to the Treasure Chamber, every minifigure rebuilt, a bag of the rewards
    /// for `levels` levels done.
    Out { levels: u32 },
}

// ------------------------------------------------------------------ fixtures

/// A level copy's gates, button and door, the button's arrow carrier, and its points' dynels
/// with those showing the spawn particle now.
struct Fixtures {
    _presences: Vec<world::Presence>,
    dummy: Id32,
    lit: std::sync::Arc<Mutex<HashSet<usize>>>,
}

static FIXTURES: LazyLock<Mutex<HashMap<Pf, Fixtures>>> = LazyLock::new(Default::default);
/// Gates open, by copy and visual guid.
static OPEN: LazyLock<Mutex<HashSet<(Pf, u32)>>> = LazyLock::new(Default::default);
/// The variant each run copy picked.
static VARIANTS: LazyLock<Mutex<HashMap<Pf, String>>> = LazyLock::new(Default::default);
/// For testing: the variant the next level copy runs, if that level has it (`!lc variant`).
static FORCED_VARIANT: LazyLock<Mutex<Option<String>>> = LazyLock::new(Default::default);

fn piece_id(pf: Pf, guid: u32) -> Id32 {
    crate::scenery::piece_id(pf, guid)
}

/// The variant copy `pf` runs (`None`: every spawn; npc.rs).
pub fn variant(pf: Pf) -> Option<String> {
    VARIANTS.lock().unwrap().get(&pf).cloned()
}

/// How live made the button usable: its item template ("XX_Interactable") and reach.
const BUTTON_TEMPLATE: u32 = 482_763;
const BUTTON_REACH: u32 = 50;

/// A level's simple dynel: UpdateMessage + `SimpleDynelFullUpdateIIR_c` (as scenery.rs), its
/// visual and collision as given; `usable`: the button's display name, template and reach.
fn piece_records(p: &Piece, pf: Pf, visual: u32, collision: u32, usable: Option<String>) -> Vec<Bytes> {
    let cars = if usable.is_some() { CARS_USABLE } else { CARS_INACTIVE };
    let id = piece_id(pf, p.guid);
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(p.pos[0]), y: Some(p.pos[1]), z: Some(p.pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(p.rot[0]), y: Some(p.rot[1]), z: Some(p.rot[2]), w: Some(p.rot[3]) }),
        playfield_instance: Some(pf.key()),
        running_effects: vec![],
    };
    let simple = ProtoSimpleDynelIir {
        kdtree: Some(collision),
        name: Some(p.name.clone()),
        template_item_id: usable.is_some().then_some(BUTTON_TEMPLATE),
        interaction_radius: usable.is_some().then_some(BUTTON_REACH),
        display_name: usable,
        cars_group: Some(cars),
        is_closed: Some(true),
        ..Default::default()
    };
    let record = iir::record("SimpleDynelFullUpdateIIR_c", id, false, |w| {
        let w = w.proto(&dynel).proto(&simple).u32(visual).u32(p.visual[0]).u32(0);
        p.bounds.iter().fold(w, |w, v| w.f32(*v)).f32(p.scale).u32(0).u8(0)
    });
    vec![crate::game::create_game_object(id, pf, p.pos), record, crate::game::int32_stat(id, STAT_CARS_GROUP, cars)]
}

fn join(id: Id32, pf: Pf, pos: [f32; 3], spawn: impl Fn() -> Vec<Bytes> + Send + Sync + 'static) -> world::Presence {
    let (outbox, mut inbox) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    world::join(id, pf, std::sync::Arc::new(Mutex::new(pos)), spawn, outbox)
}

/// A gate: its visual (red or green) and its shield (a wall while closed).
fn gate(pf: Pf, shield: &Option<Piece>, visual: &Piece, open_from_start: bool, presences: &mut Vec<world::Presence>) {
    if open_from_start {
        OPEN.lock().unwrap().insert((pf, visual.guid));
    }
    let v = visual.clone();
    let guid = visual.guid;
    presences.push(join(piece_id(pf, guid), pf, v.pos, move || {
        let open = OPEN.lock().unwrap().contains(&(pf, guid));
        piece_records(&v, pf, if open { GATE_OPEN } else { GATE_CLOSED }, 0, None)
    }));
    if let Some(s) = shield.clone() {
        presences.push(join(piece_id(pf, s.guid), pf, s.pos, move || {
            let open = OPEN.lock().unwrap().contains(&(pf, guid));
            let collision = if open { 0 } else { crate::encounter::SHIELD_COLLISION };
            piece_records(&s, pf, crate::encounter::SHIELD_COLLISION, collision, None)
        }));
    }
}

/// Opens a gate in copy `pf`: the shield's collision off, the visual green (as live).
fn open_gate(pf: Pf, shield: &Option<Piece>, visual: &Piece) {
    OPEN.lock().unwrap().insert((pf, visual.guid));
    let mut recs = Vec::new();
    if let Some(s) = shield {
        recs.push(crate::encounter::collision_record(piece_id(pf, s.guid), false));
    }
    let v = piece_id(pf, visual.guid);
    recs.push(iir::record("SimpleDynelVisualChangeIIR_c", v, false, |w| w.u32(VISUAL_STATE).u32(GATE_OPEN).u32(PLAIN_VISUAL)));
    recs.push(crate::game::stat_record(v, STAT_VISUAL, GATE_OPEN));
    to_copy(pf, recs);
}

/// Puts a level copy's gates, button, exit door and the button's arrow in (worlds.rs).
pub fn spawn_world(pf: Pf) {
    let Some(i) = level_of_model(pf.model()) else { return };
    let l = level(i);
    let mut presences = Vec::new();
    gate(pf, &l.start_shield, &l.start_visual, false, &mut presences);
    gate(pf, &l.exit_shield, &l.exit_visual, l.exit_open, &mut presences);
    let b = l.button.clone();
    let name = format!(r#"<remoteformat id="{BUTTON_NAME}" category="50200" knubot="0"  ></remoteformat>"#);
    presences.push(join(piece_id(pf, b.guid), pf, b.pos, move || piece_records(&b, pf, b.visual[1], 0, Some(name.clone()))));
    let d = l.door.clone();
    let id = Id32::new(crate::teleport::DOOR_TYPE, crate::instance::fixture(pf, d.guid));
    presences.push(join(id, pf, d.pos, move || crate::teleport::door_dynel_records(d.guid, &d.name, d.pos, d.rot, d.scale, d.visual, 0, pf)));
    // the spawn points: invisible dynels that show the spawn particle while lit (live put it on
    // Monster Laboratory's pads)
    let lit: std::sync::Arc<Mutex<HashSet<usize>>> = Default::default();
    for (i, &pos) in l.points.iter().enumerate() {
        let id = point_id(pf, i);
        let lit = lit.clone();
        presences.push(join(id, pf, pos, move || {
            let mut recs = crate::scenery::invisible_records(id, pf, pos, [0.0, 0.0, 0.0, 1.0]);
            if lit.lock().unwrap().contains(&i) {
                recs.push(particle_record(id, Some(tunables().wave_particle)));
            }
            recs
        }));
    }
    // the arrow over the button: an invisible NPC carrying the Button Arrow buff (live)
    let def: Result<crate::npc::SpawnDef, _> = serde_json::from_value(serde_json::json!({
        "kind": "npc", "name": "XX_InvisibleDummy", "monster_data": DUMMY_LOOK, "npc_template": DUMMY_TEMPLATE,
        "cars_group": CARS_INACTIVE, "level": 1, "health": 100, "pos": l.button.pos,
    }));
    let dummy = match def {
        Ok(def) => {
            let id = crate::npc::new_id(&def);
            crate::npc::spawn_as(id, def, pf);
            id
        }
        Err(e) => {
            warn!("button dummy: {e}");
            Id32::default()
        }
    };
    FIXTURES.lock().unwrap().insert(pf, Fixtures { _presences: presences, dummy, lit });
}

fn point_id(pf: Pf, i: usize) -> Id32 {
    piece_id(pf, POINT_BASE + i as u32)
}

/// `SimpleDynelVisualChangeIIR_c`: state 0 starts a looping particle (attractor 35, as live), 2
/// stops the first one.
fn particle_record(id: Id32, particle: Option<u32>) -> Bytes {
    iir::record("SimpleDynelVisualChangeIIR_c", id, false, |w| match particle {
        Some(p) => w.u32(0).u32(p).i32(35).f32(0.0).f32(0.0).f32(0.0).f32(0.0),
        None => w.u32(2).u32(0),
    })
}

/// A wave of copy `pf`'s script is about to spawn on `points` (scripts.rs): if `pf` is a level's
/// copy, the level's points there show the spawn particle, and the wave waits this long.
pub fn telegraph(pf: Pf, points: &[[f32; 3]]) -> Option<Duration> {
    let i = level_of_model(pf.model())?;
    let fixtures = FIXTURES.lock().unwrap();
    let f = fixtures.get(&pf)?;
    let particle = tunables().wave_particle;
    let mut lit = f.lit.lock().unwrap();
    let mut recs = Vec::new();
    for (n, p) in level(i).points.iter().enumerate() {
        if points.iter().any(|q| (p[0] - q[0]).hypot(p[2] - q[2]) < POINT_SAME) && lit.insert(n) {
            recs.push(particle_record(point_id(pf, n), Some(particle)));
        }
    }
    drop(lit);
    drop(fixtures);
    to_copy(pf, recs);
    Some(duration(tunables().wave_telegraph))
}

/// The telegraph is over (the wave spawns): the particles stop.
pub fn telegraph_done(pf: Pf) {
    let fixtures = FIXTURES.lock().unwrap();
    let Some(f) = fixtures.get(&pf) else { return };
    let recs: Vec<Bytes> = f.lit.lock().unwrap().drain().map(|n| particle_record(point_id(pf, n), None)).collect();
    drop(fixtures);
    to_copy(pf, recs);
}

/// Takes a level copy's fixtures out (worlds.rs).
pub fn despawn_world(pf: Pf) {
    let gone = FIXTURES.lock().unwrap().remove(&pf);
    if let Some(f) = &gone {
        crate::npc::despawn(f.dummy);
    }
    drop(gone);
    OPEN.lock().unwrap().retain(|(p, _)| *p != pf);
    VARIANTS.lock().unwrap().remove(&pf);
    let mut st = STATE.lock().unwrap();
    // the copy closed with members away: they can't come back to it
    if let Some(run_id) = st.copies.remove(&pf) {
        let away: Vec<u32> = st.runs.get(&run_id).map(|r| r.members.iter().copied().filter(|m| st.away.contains_key(m)).collect()).unwrap_or_default();
        for m in away {
            leave_run(&mut st, m);
        }
    }
    drop(st);
    INFO.lock().unwrap().copies.remove(&pf);
}

fn dummy_of(pf: Pf) -> Option<Id32> {
    FIXTURES.lock().unwrap().get(&pf).map(|f| f.dummy)
}

// ------------------------------------------------------------------ entering

/// The entry door's guid (the client data's door in Dinosaur Rise).
pub fn is_entry_door(model: u32, guid: u32) -> bool {
    let t = tunables();
    guid == t.entry_door && crate::gamedata::get().worlds.get(&t.entry_world).is_some_and(|w| w.playfield == model)
}

/// The entry door, its world and where it stands (for the test commands and the door's spawn).
pub fn entry_point() -> Option<(u32, [f32; 3])> {
    let t = tunables();
    let c = crate::gamedata::get();
    let world = c.worlds.get(&t.entry_world)?;
    let s = c.in_world(&c.spawns, &t.entry_world).find(|s| s.client.as_ref().and_then(|cl| cl.guid) == Some(t.entry_door))?;
    Some((world.playfield, s.pos))
}

/// The entry door's spawn (in `entry_world`'s copies; teleport.rs spawns portal doors only).
pub fn entry_door_records(pf: Pf) -> Option<(Id32, [f32; 3], Vec<Bytes>)> {
    let t = tunables();
    let c = crate::gamedata::get();
    let world = c.worlds.get(&t.entry_world)?;
    if world.playfield != pf.model() {
        return None;
    }
    let s = c.in_world(&c.spawns, &t.entry_world).find(|s| s.client.as_ref().and_then(|cl| cl.guid) == Some(t.entry_door))?;
    let cl = s.client.as_ref()?;
    let rot = match s.rot.unwrap_or_default() {
        lmoxide_content::Rot::Quat(q) => q,
        lmoxide_content::Rot::Heading(h) => [0.0, (h / 2.0).sin(), 0.0, (h / 2.0).cos()],
    };
    let id = Id32::new(crate::teleport::DOOR_TYPE, crate::instance::fixture(pf, t.entry_door));
    let records = crate::teleport::door_dynel_records(t.entry_door, &s.name, s.pos, rot, s.scale, cl.visual.unwrap_or([1_010_201, 465_832]), 0, pf);
    Some((id, s.pos, records))
}

static ENTRY_DOORS: LazyLock<Mutex<HashMap<Pf, world::Presence>>> = LazyLock::new(Default::default);

/// Puts the entry door in copy `pf` of its world (worlds.rs).
pub fn spawn_entry(pf: Pf) {
    let Some((id, pos, _)) = entry_door_records(pf) else { return };
    let presence = join(id, pf, pos, move || entry_door_records(pf).map(|(_, _, r)| r).unwrap_or_default());
    ENTRY_DOORS.lock().unwrap().insert(pf, presence);
}

pub fn despawn_entry(pf: Pf) {
    let gone = ENTRY_DOORS.lock().unwrap().remove(&pf);
    drop(gone);
}

/// `character` used the entry door: the "Lost Creations" window.
pub fn use_entry(character: u32) {
    let mut st = STATE.lock().unwrap();
    if st.run_of.contains_key(&character) {
        return;
    }
    let ctx = st.ctx();
    st.asks.insert(character, (ctx, Ask::Entry));
    drop(st);
    info!(character, "Lost Creations: entry window");
    window(character, WINDOW_ENTRY, ctx, 0);
}

/// `GameCharAgentProject::EndlessPlayfieldsTeleportMessageBoxAnswer(answer, ctx)`: the entry
/// window (Enter 1, X -1) or the continue window at the exit door (Continue 1, End Run 2, X -1).
/// -2: the client replaced the window by a newer one.
pub fn window_answer(character: u32, answer: i32, ctx: i32) {
    let mut st = STATE.lock().unwrap();
    let Some(&(asked, ask)) = st.asks.get(&character) else { return };
    if asked != ctx || answer == -2 {
        return;
    }
    st.asks.remove(&character);
    info!(character, answer, ?ask, "Lost Creations: window answer");
    match (ask, answer) {
        (Ask::Entry, 1) => {
            close_window(character);
            gather(&mut st, character);
        }
        (Ask::Continue(run), 1) => continue_answer(&mut st, run, character, true),
        (Ask::Continue(run), 2) => continue_answer(&mut st, run, character, false),
        (Ask::Continue(run), _) => cancel_continue(&mut st, run),
        // X or Esc: the client waits for the close
        (Ask::Entry, _) => close_window(character),
        _ => {}
    }
}

/// `GameCharAgentProject::EndlessPlayfieldsInvitationAnswer(n)`: an invitation (Join 2, Cancel
/// 1) or a teammate's continue window (Continue 2, End Run 1, closed 0).
pub fn invitation_answer(character: u32, answer: i32) {
    let mut st = STATE.lock().unwrap();
    let Some(&(_, ask)) = st.asks.get(&character) else { return };
    st.asks.remove(&character);
    info!(character, answer, ?ask, "Lost Creations: invitation answer");
    match (ask, answer) {
        (Ask::Invite(leader), 2) => join_gathering(&mut st, leader, character),
        (Ask::Invite(leader), _) => abort_gathering(&mut st, leader, vec![character]),
        (Ask::Continue(run), 2) => continue_answer(&mut st, run, character, true),
        (Ask::Continue(run), 1) => continue_answer(&mut st, run, character, false),
        (Ask::Continue(run), _) => cancel_continue(&mut st, run),
        _ => {}
    }
}

/// Enter: the teammates online are asked to come along, or a player alone starts at once.
fn gather(st: &mut State, leader: u32) {
    let mates: Vec<u32> =
        crate::community::teammates(leader).into_iter().filter(|m| *m != leader && crate::sessions::is_online(*m) && !st.run_of.contains_key(m)).collect();
    if mates.is_empty() {
        start_run(st, vec![leader], None);
        return;
    }
    let generation = st.generation();
    let time = duration(tunables().invitation_time);
    for &m in &mates {
        let ctx = st.ctx();
        st.asks.insert(m, (ctx, Ask::Invite(leader)));
        window(m, WINDOW_INVITE, ctx, secs(time));
        rpc(m, Endpoint::GameCharInterfaceProject, project::SIGNAL_START_ENDLESS_PLAYFIELDS_INVITATION, Writer::new().u32(secs(time)).finish());
    }
    info!(leader, ?mates, "Lost Creations: teammates invited");
    st.gathering.insert(leader, Gathering { mates, joined: HashSet::new(), generation });
    tokio::spawn(async move {
        tokio::time::sleep(time).await;
        let mut st = STATE.lock().unwrap();
        let late = match st.gathering.get(&leader) {
            Some(g) if g.generation == generation => g.mates.iter().copied().filter(|m| !g.joined.contains(m)).collect(),
            _ => return,
        };
        abort_gathering(&mut st, leader, late);
    });
}

fn join_gathering(st: &mut State, leader: u32, mate: u32) {
    let Some(g) = st.gathering.get_mut(&leader) else { return };
    g.joined.insert(mate);
    close_window(mate);
    if g.mates.iter().all(|m| g.joined.contains(m)) {
        let g = st.gathering.remove(&leader).unwrap();
        let mut members = vec![leader];
        members.extend(g.mates);
        start_run(st, members, None);
    }
}

/// Someone declined (or didn't answer): nobody goes ("Failed to start...", with the names).
fn abort_gathering(st: &mut State, leader: u32, declined: Vec<u32>) {
    let Some(g) = st.gathering.remove(&leader) else { return };
    info!(leader, ?declined, "Lost Creations: start aborted");
    let args = crate::community::group_vec(crate::community::group_vec(Writer::new(), &declined), &[]).finish();
    for &m in std::iter::once(&leader).chain(&g.mates) {
        if st.asks.get(&m).is_some_and(|(_, a)| *a == Ask::Invite(leader)) {
            st.asks.remove(&m);
            close_window(m);
        }
        rpc(m, Endpoint::GameCharInterfaceProject, project::SIGNAL_ENDLESS_ABORTED, args.clone());
    }
}

/// A random number below `n` (n > 0).
fn random_below(n: u32) -> u32 {
    crate::loot::random_below(n.max(1))
}

/// A new run for `members`, at level 1 (or `first`: test commands, (level, level index)).
fn start_run(st: &mut State, members: Vec<u32>, first: Option<(u32, Option<usize>)>) {
    let Some(&leader) = members.first() else { return };
    let mode = crate::sessions::mode_of(leader).unwrap_or(Mode::Progression);
    let id = loop {
        let id = crate::loot::random_below(u32::MAX).max(1);
        if !st.runs.contains_key(&id) {
            break id;
        }
    };
    let level = first.map_or(1, |(n, _)| n.max(1));
    let run = Run {
        id,
        mode,
        members: members.clone(),
        level,
        done: members.iter().map(|&m| (m, 0)).collect(),
        current: 0,
        pf: Pf::default(),
        element: 1 + random_below(5),
        phase: Phase::Waiting,
        step: 0,
        progress: 0,
        exit_open: false,
        finished: HashSet::new(),
        asked: false,
        continues: HashSet::new(),
        started: Instant::now(),
        deadline: None,
        generation: 0,
    };
    for &m in &members {
        st.run_of.insert(m, id);
    }
    st.runs.insert(id, run);
    info!(run = id, ?members, level, "Lost Creations: run started");
    enter_level(st, id, first.and_then(|(_, l)| l));
}

/// Which level comes as level `n`: the forced one (tunables, temporary), else the Gatekeeper's on
/// every `gatekeeper_every`-th and never otherwise, else any other at random.
fn pick_level(n: u32) -> Option<usize> {
    let t = tunables();
    if let Some(i) = t.force_level.as_ref().and_then(|id| LEVELS.iter().position(|l| &l.id == id)) {
        return Some(i);
    }
    let gatekeeper = t.gatekeeper_every > 0 && n % t.gatekeeper_every == 0;
    let pool: Vec<usize> = (0..LEVELS.len()).filter(|&i| (level(i).mode == LcMode::Gatekeeper) == gatekeeper).collect();
    (!pool.is_empty()).then(|| pool[random_below(pool.len() as u32) as usize])
}

/// The team goes to a new copy of the run's next level (`chosen`: that level).
fn enter_level(st: &mut State, run_id: u32, chosen: Option<usize>) {
    let generation = st.generation();
    let Some(run) = st.runs.get_mut(&run_id) else { return };
    let Some(current) = chosen.or_else(|| pick_level(run.level)) else {
        warn!(run = run_id, "Lost Creations: no level to go to");
        return end_run(st, run_id);
    };
    let l = level(current);
    let Some(pf) = crate::instance::new_copy(l.model, run.mode) else {
        warn!(run = run_id, level = %l.id, "Lost Creations: no free copy");
        return end_run(st, run_id);
    };
    // the element changes after each block of levels
    let block = tunables().element_block.max(1);
    if run.level > 1 && (run.level - 1) % block == 0 {
        let old = run.element;
        run.element = 1 + (old + random_below(4)) % 5;
    }
    let forced = FORCED_VARIANT.lock().unwrap().take_if(|v| l.variants.iter().any(|x| &x.id == v));
    if let Some(v) = forced {
        info!(run = run_id, variant = %v, "Lost Creations: the variant forced (test command)");
        VARIANTS.lock().unwrap().insert(pf, v);
    } else if !l.variants.is_empty() {
        let total: u32 = l.variants.iter().map(|v| v.weight).sum();
        let mut r = random_below(total);
        if let Some(v) = l.variants.iter().find(|v| {
            let hit = r < v.weight;
            r = r.saturating_sub(v.weight);
            hit
        }) {
            VARIANTS.lock().unwrap().insert(pf, v.id.clone());
        }
    }
    let old = run.pf;
    run.current = current;
    run.pf = pf;
    run.phase = Phase::Waiting;
    run.step = 0;
    run.progress = 0;
    run.exit_open = l.exit_open;
    run.finished.clear();
    run.asked = false;
    run.continues.clear();
    run.deadline = None;
    run.generation = generation;
    let members = run.members.clone();
    let (level_n, element) = (run.level, run.element);
    st.copies.remove(&old);
    st.copies.insert(pf, run_id);
    INFO.lock().unwrap().copies.remove(&old);
    publish(&st.runs[&run_id]);
    info!(run = run_id, level = level_n, map = %l.id, playfield = ?pf, element, "Lost Creations: level");
    for m in members {
        close_window(m);
        rpc(m, Endpoint::GameCharInterface, game_char_interface::TELEPORT_STARTED, Bytes::new());
        order(m, Order::Enter(pf, l.start.0, l.start.1));
    }
}

/// `character` arrived in copy `pf` (CharInPlay): the run's records for them (the buffs, the
/// quest, the button's arrow). One in a level who isn't in its run (a reconnect after the run,
/// a server restart) goes to the Treasure Chamber.
pub fn arrived(character: u32, pf: Pf) -> Vec<Bytes> {
    let mut st = STATE.lock().unwrap();
    if st.away.remove(&character).is_some() {
        info!(character, ?pf, "Lost Creations: back after a lost connection");
    }
    let Some(run) = st.run_of(character) else {
        if let Some(i) = level_of_model(pf.model()).filter(|_| !crate::instance::rules(pf).editor) {
            match st.pending.remove(&character) {
                // a run that ended while they were away: its rewards
                Some(levels) => {
                    drop(st);
                    info!(character, ?pf, "Lost Creations: in a level outside a run, with rewards waiting");
                    order(character, Order::Out { levels });
                }
                // (a teleport or a reconnect into a level): a run of their own on this level, in
                // a new copy (the Gatekeeper's as level `gatekeeper_every`)
                None => {
                    let n = if level(i).mode == LcMode::Gatekeeper { tunables().gatekeeper_every.max(1) } else { 1 };
                    info!(character, ?pf, level = %level(i).id, "Lost Creations: in a level outside a run: a run starts there");
                    start_run(&mut st, vec![character], Some((n, Some(i))));
                }
            }
        }
        return Vec::new();
    };
    if run.pf != pf {
        return Vec::new();
    }
    let t = tunables();
    let me = player(character);
    let l = level(run.current);
    let left = RUN_BUFF_CS - (run.started.elapsed().as_millis() / 10) as i32;
    let mut recs = vec![crate::effects::buff_add(me, t.cant_resurrect_buff, me, left.max(1))];
    if !run.finished.contains(&character) {
        recs.extend(l.quest.show(character, run.step, run.progress));
    }
    match run.phase {
        Phase::Waiting => {
            if let Some(d) = dummy_of(pf) {
                recs.push(crate::effects::buff_add(d, t.button_arrow_buff, d, RUN_BUFF_CS));
            }
        }
        Phase::Running if !run.finished.contains(&character) => {
            let left = run.deadline.map_or(0, |d| d.saturating_duration_since(Instant::now()).as_millis() / 10) as i32;
            recs.push(crate::effects::buff_add(me, t.timer_buff, me, left.max(1)));
            recs.push(crate::effects::buff_add(me, t.cant_equip_buff, me, RUN_BUFF_CS));
        }
        _ => {}
    }
    recs
}

// ------------------------------------------------------------------ the level

/// `character` used a simple dynel (base guid `guid`) in copy `pf`: the start button. True if it
/// was this level's button.
pub fn press(character: u32, pf: Pf, guid: u32) -> bool {
    let Some(i) = level_of_model(pf.model()) else { return false };
    if level(i).button.guid != guid {
        return false;
    }
    let mut st = STATE.lock().unwrap();
    if !st.run_of(character).is_some_and(|r| r.pf == pf && r.phase == Phase::Waiting) {
        return true;
    }
    // not again while the last one may be open (pressing E again stacks them; the X closes only
    // the newest, and closing sends nothing)
    if st.warmup_asked.get(&character).is_some_and(|at| at.elapsed() < WARMUP_DIALOG_HOLD) {
        return true;
    }
    st.warmup_asked.insert(character, Instant::now());
    info!(character, "Lost Creations: button");
    rpc(character, Endpoint::GameCharInterfaceProject, project::SIGNAL_OPEN_ENDLESS_WARMUP_DIALOG, Bytes::new());
    true
}

/// `GameCharAgentProject::StartEndlessWarmup()`: Start in "Ready to start?". The countdown, then
/// the gate opens and the timer runs.
pub fn start_warmup(character: u32) {
    let mut st = STATE.lock().unwrap();
    st.warmup_asked.remove(&character);
    let Some(run) = st.run_of(character) else { return };
    if run.phase != Phase::Waiting {
        return;
    }
    run.phase = Phase::Countdown;
    let (run_id, pf, generation, members) = (run.id, run.pf, run.generation, run.members.clone());
    let t = tunables();
    let warmup = duration(t.warmup);
    info!(character, run = run_id, "Lost Creations: countdown");
    for m in members {
        rpc(m, Endpoint::GameCharInterfaceProject, project::SIGNAL_START_ENDLESS_WARMUP, Writer::new().u32(secs(warmup)).finish());
    }
    let mut recs = Vec::new();
    if let Some(d) = dummy_of(pf) {
        recs.push(crate::effects::buff_remove(d, t.button_arrow_buff, d));
    }
    if let Some(i) = level_of_model(pf.model()) {
        recs.push(crate::effects::effect_package(piece_id(pf, level(i).button.guid), t.button_effect));
    }
    to_copy(pf, recs);
    tokio::spawn(async move {
        tokio::time::sleep(warmup).await;
        open_start(run_id, generation);
    });
}

/// The countdown is over: the start gate opens, the timer runs, no more equipping.
fn open_start(run_id: u32, generation: u64) {
    let mut st = STATE.lock().unwrap();
    let Some(run) = st.runs.get_mut(&run_id).filter(|r| r.generation == generation && r.phase == Phase::Countdown) else { return };
    let t = tunables();
    let time = duration(t.level_time);
    run.phase = Phase::Running;
    run.deadline = Some(Instant::now() + time);
    let (pf, members) = (run.pf, run.members.clone());
    let l = level(run.current);
    info!(run = run_id, "Lost Creations: gate open, timer running");
    open_gate(pf, &l.start_shield, &l.start_visual);
    let variant = variant(pf);
    if l.script.is_some() && !l.has_waves(variant.as_ref()) {
        warn!(run = run_id, level = %l.id, ?variant, "Lost Creations: no wave enemies for this variant: the objective stays open");
    } else if let Some(script) = l.script.clone() {
        // (scripts.rs locks after this module: started once `STATE` is free)
        tokio::spawn(async move {
            match crate::scripts::start(pf, &script, None) {
                Ok(m) => info!(run = run_id, "Lost Creations: waves: {m}"),
                Err(e) => warn!(run = run_id, "Lost Creations: the waves didn't start: {e}"),
            }
        });
    }
    for &m in &members {
        let me = player(m);
        records(m, vec![
            crate::effects::buff_add(me, t.timer_buff, me, TIMER_ADD_CS),
            crate::effects::buff_update_counter(me, t.timer_buff, me, 0, (time.as_millis() / 10) as u32),
        ]);
    }
    drop(st);
    tokio::spawn(async move {
        tokio::time::sleep(CANT_EQUIP_AFTER).await;
        {
            let mut st = STATE.lock().unwrap();
            let Some(run) = st.runs.get_mut(&run_id).filter(|r| r.generation == generation && r.phase == Phase::Running) else { return };
            for &m in run.members.iter().filter(|m| !run.finished.contains(m)) {
                let me = player(m);
                records(m, vec![
                    crate::effects::buff_add(me, t.cant_equip_buff, me, RUN_BUFF_CS),
                    crate::game::stat_record(me, STAT_EQUIP_FLAGS, EQUIP_LOCKED),
                    crate::game::stat_update_record(me, &[(STAT_EQUIP_FLAGS, EQUIP_LOCKED)]),
                ]);
            }
        }
        tokio::time::sleep(time.saturating_sub(CANT_EQUIP_AFTER)).await;
        time_up(run_id, generation);
    });
}

/// The timer ran out: whoever isn't across the end is out. The timer buff is taken off (removed
/// with less than a second left, the client shows "Time's Up!"; it doesn't end by itself: the
/// clock counts on past 0).
fn time_up(run_id: u32, generation: u64) {
    let mut st = STATE.lock().unwrap();
    let Some(run) = st.runs.get_mut(&run_id).filter(|r| r.generation == generation && r.phase == Phase::Running) else { return };
    let late: Vec<u32> = run.members.iter().copied().filter(|m| !run.finished.contains(m)).collect();
    info!(run = run_id, ?late, "Lost Creations: time up");
    let buff = tunables().timer_buff;
    for &m in &late {
        records(m, vec![crate::effects::buff_remove(player(m), buff, player(m))]);
    }
    let delay = duration(tunables().smashed_delay);
    drop(st);
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let mut st = STATE.lock().unwrap();
        for m in late {
            leave_run(&mut st, m);
        }
    });
}

/// Something counted (game.rs: the player's own events, not a teammate's shared ones): toward the
/// level's objective.
pub fn on_event(character: u32, event: &Event) {
    let mut st = STATE.lock().unwrap();
    let Some(run) = st.run_of(character) else { return };
    let l = level(run.current);
    let playfield = match event {
        Event::Defeated { playfield, .. } => *playfield,
        _ => return,
    };
    // (a level with waves counts the waves cleared instead)
    if playfield != l.model || l.script.is_some() || !matches!(run.phase, Phase::Running) || run.step + 1 >= l.quest.steps() || !l.quest.counts(run.step, event) {
        return;
    }
    let need = l.quest.counted(run.step).map_or(1, |g| g.count.max(1));
    run.progress += 1;
    let members = run.members.clone();
    if run.progress < need {
        for m in members {
            records(m, l.quest.progress(m, run.step, 1).into_iter().collect());
        }
        return;
    }
    objective_done(run);
}

/// The run whose level copy `pf` runs `script` as its waves, while its objective is open.
fn waves_run<'a>(st: &'a mut State, pf: Pf, script: &str) -> Option<&'a mut Run> {
    let id = *st.copies.get(&pf)?;
    let run = st.runs.get_mut(&id)?;
    let l = level(run.current);
    (run.pf == pf && run.phase == Phase::Running && l.script.as_deref() == Some(script) && run.step + 1 < l.quest.steps()).then_some(run)
}

/// A wave of copy `pf`'s script `script` was cleared (scripts.rs): one toward the objective
/// (Survival: "Survive the assault" 1/3, 2/3, 3/3).
pub fn wave_cleared(pf: Pf, script: &str) {
    let mut st = STATE.lock().unwrap();
    let Some(run) = waves_run(&mut st, pf, script) else { return };
    let l = level(run.current);
    let need = l.quest.counted(run.step).map_or(1, |g| g.count.max(1));
    run.progress += 1;
    info!(run = run.id, progress = run.progress, need, "Lost Creations: wave cleared");
    if run.progress < need {
        for &m in &run.members {
            records(m, l.quest.progress(m, run.step, 1).into_iter().collect());
        }
        return;
    }
    objective_done(run);
}

/// Copy `pf`'s script `script` is over, every wave cleared (scripts.rs; `spawns`: its spawns):
/// the objective is done if it isn't yet. With `gatekeeper_adds_vanish`, what those spawns
/// summoned goes too.
pub fn script_over(pf: Pf, script: &str, spawns: &[Id32]) {
    let mut st = STATE.lock().unwrap();
    if let Some(run) = waves_run(&mut st, pf, script) {
        objective_done(run);
    }
    drop(st);
    let gatekeeper = level_of_model(pf.model()).is_some_and(|i| level(i).mode == LcMode::Gatekeeper);
    if gatekeeper && tunables().gatekeeper_adds_vanish {
        let n = crate::npc::despawn_summons_of(spawns);
        info!(playfield = ?pf, gone = n, "Lost Creations: the Gatekeeper's adds vanish");
    }
}

/// The objective is done: the quest's "reach the end" step, the end gate opens.
fn objective_done(run: &mut Run) {
    let l = level(run.current);
    info!(run = run.id, "Lost Creations: objective done");
    for &m in &run.members {
        records(m, l.quest.step_done(m, run.step));
    }
    run.step += 1;
    run.progress = 0;
    if !run.exit_open {
        run.exit_open = true;
        open_gate(run.pf, &l.exit_shield, &l.exit_visual);
    }
}

/// The player moved (game.rs): crossing the end completes the level for them.
pub fn moved(character: u32, pf: Pf, pos: [f32; 3]) {
    reward_check(character, pf, pos);
    let mut st = STATE.lock().unwrap();
    let Some(run) = st.run_of(character) else { return };
    let l = level(run.current);
    if run.pf != pf || run.phase != Phase::Running || !run.exit_open || run.step + 1 != l.quest.steps() || run.finished.contains(&character) || !l.end.contains(pos, HEIGHT_SLACK) {
        return;
    }
    finish(run, character);
}

/// `character` crossed the end.
fn finish(run: &mut Run, character: u32) {
    let t = tunables();
    let l = level(run.current);
    let me = player(character);
    run.finished.insert(character);
    *run.done.entry(character).or_default() += 1;
    info!(character, run = run.id, level = run.level, "Lost Creations: level done");
    let mut recs = l.quest.step_done(character, run.step);
    recs.push(crate::effects::effect_package(me, LEVEL_DONE_EFFECT));
    recs.push(crate::effects::buff_remove(me, t.timer_buff, me));
    recs.push(crate::effects::buff_remove(me, t.cant_equip_buff, me));
    recs.push(crate::game::stat_record(me, STAT_EQUIP_FLAGS, 0));
    recs.push(crate::game::stat_update_record(me, &[(STAT_EQUIP_FLAGS, 0)]));
    records(character, recs);
    crate::sessions::send(character, world::Out::Progress(Event::LostCreations { level: run.level }));
    if run.members.iter().all(|m| run.finished.contains(m)) {
        run.phase = Phase::Done;
    }
}

// ------------------------------------------------------------------ the exit

/// `character` used a door (base guid `guid`) in copy `pf`: the level's exit door. True if it
/// was one.
pub fn use_door(character: u32, pf: Pf, guid: u32) -> bool {
    let Some(i) = level_of_model(pf.model()) else { return false };
    if level(i).door.guid != guid {
        return false;
    }
    let mut st = STATE.lock().unwrap();
    let Some(run) = st.run_of(character).filter(|r| r.pf == pf) else { return true };
    if !run.finished.contains(&character) {
        return true;
    }
    if run.members.iter().any(|m| !run.finished.contains(m)) {
        // "wait until all cross the finish line"
        rpc(character, Endpoint::GameCharInterfaceProject, project::ENDLESS_PLAYFIELDS_INVITATION_FAILED, Bytes::new());
        return true;
    }
    if run.asked {
        return true;
    }
    run.asked = true;
    run.continues.clear();
    let (run_id, members, generation) = (run.id, run.members.clone(), run.generation);
    let time = duration(tunables().continue_time);
    info!(character, run = run_id, "Lost Creations: continue?");
    for m in members {
        let ctx = st.ctx();
        st.asks.insert(m, (ctx, Ask::Continue(run_id)));
        if m == character {
            window(m, WINDOW_CONTINUE, ctx, 0);
        } else {
            window(m, WINDOW_CONTINUE_TEAMMATE, ctx, secs(time));
        }
    }
    tokio::spawn(async move {
        tokio::time::sleep(time).await;
        let mut st = STATE.lock().unwrap();
        let Some(run) = st.runs.get(&run_id).filter(|r| r.generation == generation && r.asked) else { return };
        // no answer: End Run
        let silent: Vec<u32> = run.members.iter().copied().filter(|m| !run.continues.contains(m)).collect();
        for m in silent {
            continue_answer(&mut st, run_id, m, false);
        }
    });
    true
}

/// An answer at the exit: Continue, or End Run (only that player out).
fn continue_answer(st: &mut State, run_id: u32, character: u32, go_on: bool) {
    let Some(run) = st.runs.get_mut(&run_id) else { return };
    if !run.asked || !run.members.contains(&character) {
        return;
    }
    if go_on {
        run.continues.insert(character);
        close_window(character);
    } else {
        close_window(character);
        leave_run(st, character);
    }
    let Some(run) = st.runs.get_mut(&run_id) else { return };
    if !run.members.is_empty() && run.members.iter().all(|m| run.continues.contains(m)) {
        run.level += 1;
        enter_level(st, run_id, None);
    }
}

/// The question was closed: nobody goes yet (the door asks again).
fn cancel_continue(st: &mut State, run_id: u32) {
    let Some(run) = st.runs.get_mut(&run_id) else { return };
    run.asked = false;
    run.continues.clear();
    let members = run.members.clone();
    for m in members {
        if st.asks.get(&m).is_some_and(|(_, a)| *a == Ask::Continue(run_id)) {
            st.asks.remove(&m);
        }
        close_window(m);
    }
}

// ------------------------------------------------------------------ leaving

/// The player's whole team is smashed in a run (game.rs): out, after the splash.
pub fn wiped(character: u32) {
    if !in_run(character) {
        return;
    }
    info!(character, "Lost Creations: team smashed");
    let delay = duration(tunables().smashed_delay);
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let mut st = STATE.lock().unwrap();
        leave_run(&mut st, character);
    });
}

/// `GameCharAgentProject::LeaveEndlessPlayfields()`: the HUD's quit (after the client's own
/// confirmation).
pub fn quit(character: u32) {
    let mut st = STATE.lock().unwrap();
    leave_run(&mut st, character);
}

/// The player's connection ended: out of the run (their rewards wait for their return).
pub fn disconnected(character: u32) {
    let mut st = STATE.lock().unwrap();
    st.asks.remove(&character);
    if st.gathering.contains_key(&character) {
        abort_gathering(&mut st, character, Vec::new());
    }
    let leaders: Vec<u32> = st.gathering.iter().filter(|(_, g)| g.mates.contains(&character)).map(|(l, _)| *l).collect();
    for l in leaders {
        abort_gathering(&mut st, l, vec![character]);
    }
    // still in the run (the timer runs on) until the return window passes
    if !st.run_of.contains_key(&character) {
        return;
    }
    let mark = st.generation();
    st.away.insert(character, mark);
    info!(character, "Lost Creations: connection lost, the run waits");
    tokio::spawn(async move {
        tokio::time::sleep(crate::instance::RETURN_WINDOW).await;
        let mut st = STATE.lock().unwrap();
        if st.away.get(&character) == Some(&mark) && !crate::sessions::is_online(character) {
            st.away.remove(&character);
            leave_run(&mut st, character);
        }
    });
}

/// Takes `character` out of their run; the levels they completed in it.
fn remove_member(st: &mut State, character: u32) -> Option<u32> {
    let run_id = st.run_of.remove(&character)?;
    st.asks.retain(|c, (_, a)| *c != character || !matches!(a, Ask::Continue(_)));
    let run = st.runs.get_mut(&run_id)?;
    run.members.retain(|&m| m != character);
    run.finished.remove(&character);
    run.continues.remove(&character);
    let levels = run.done.remove(&character).unwrap_or(0);
    info!(character, run = run_id, levels, left = run.members.len(), "Lost Creations: out of the run");
    INFO.lock().unwrap().members.remove(&character);
    if run.members.is_empty() {
        let run = st.runs.remove(&run_id).unwrap();
        st.copies.remove(&run.pf);
        INFO.lock().unwrap().copies.remove(&run.pf);
        info!(run = run_id, "Lost Creations: run over");
    } else {
        publish(run);
        // the others go on: everyone left across the end, or still asked to continue
        if run.members.iter().all(|m| run.finished.contains(m)) && run.phase == Phase::Running {
            run.phase = Phase::Done;
        }
        if run.asked && run.members.iter().all(|m| run.continues.contains(m)) {
            run.level += 1;
            enter_level(st, run_id, None);
        }
    }
    Some(levels)
}

/// `character` is out of the run: out of the team too (the client won't leave a team inside),
/// to the Treasure Chamber.
fn leave_run(st: &mut State, character: u32) {
    let Some(levels) = remove_member(st, character) else { return };
    st.away.remove(&character);
    if !crate::sessions::is_online(character) {
        // out while offline: the Treasure Chamber and the rewards when they come back
        crate::community::leave(character);
        st.pending.insert(character, levels);
        return;
    }
    close_window(character);
    crate::community::leave(character);
    order(character, Order::Out { levels });
}

fn end_run(st: &mut State, run_id: u32) {
    let members = st.runs.get(&run_id).map(|r| r.members.clone()).unwrap_or_default();
    for m in members {
        leave_run(st, m);
    }
}

/// Where the Treasure Chamber's players arrive: its playfield and point.
pub fn treasure_chamber() -> Option<(u32, [f32; 3], f32)> {
    let t = tunables();
    let world = crate::gamedata::get().worlds.get(&t.treasure_chamber)?;
    Some((world.playfield, t.treasure_arrival.pos, t.treasure_arrival.rot.map_or(0.0, |r| r.heading())))
}

/// The Treasure Chamber's area that drops the rewards' chest when a player walks in.
const REWARD_AREA: &str = "BA_TriggerDropLoot";

/// Rewards waiting for their player in the Treasure Chamber: levels done, and when their chest
/// last dropped (it drops again once that bag is gone unopened).
static REWARDS: LazyLock<Mutex<HashMap<u32, (u32, Option<Instant>)>>> = LazyLock::new(Default::default);

static REWARD_SHAPE: LazyLock<Option<Shape>> = LazyLock::new(|| {
    let c = crate::gamedata::get();
    c.in_world(&c.areas, &tunables().treasure_chamber).find(|a| a.name == REWARD_AREA).and_then(|a| a.shape.clone())
});

/// `character` is out of a run with `levels` levels done: their rewards wait in the Treasure
/// Chamber (game.rs).
pub fn rewards_wait(character: u32, levels: u32) {
    if levels > 0 {
        REWARDS.lock().unwrap().insert(character, (levels, None));
    }
}

/// The player moved in the Treasure Chamber: into the loot area, their rewards' chest drops (at
/// tunables `reward_chest_pos`).
fn reward_check(character: u32, pf: Pf, pos: [f32; 3]) {
    if treasure_chamber().is_none_or(|(model, _, _)| model != pf.model()) || !REWARD_SHAPE.as_ref().is_some_and(|s| s.contains(pos, HEIGHT_SLACK)) {
        return;
    }
    let t = tunables();
    let c = crate::gamedata::get();
    let lifetime = c.tunables.loot.bag_lifetime.to_std();
    let mut rewards = REWARDS.lock().unwrap();
    let Some((levels, dropped)) = rewards.get_mut(&character) else { return };
    if dropped.is_some_and(|at| at.elapsed() < lifetime) {
        return;
    }
    let Some(chest) = c.items.get(&t.reward_chest).and_then(|i| i.template) else {
        warn!(chest = %t.reward_chest, "Lost Creations: no reward chest item");
        return;
    };
    *dropped = Some(Instant::now());
    info!(character, levels = *levels, "Lost Creations: reward chest");
    drop(rewards);
    crate::loot::drop_items(player(character), pf, t.reward_chest_pos, player(character), vec![chest]);
}

/// `character` opened their rewards' chest (game.rs): the parts drop where it stood.
pub fn open_chest(character: u32, pf: Pf, pos: [f32; 3]) {
    let Some((levels, _)) = REWARDS.lock().unwrap().remove(&character) else { return };
    give_rewards(player(character), pf, pos, levels);
}

/// The rewards for `levels` levels done: minifigure parts (item templates), 0-2 per unlocked
/// pool, a pool unlocked every `levels_per_pool` levels (rounded up), either minifigure of the
/// pool, any of its parts (docs/TUNABLES.md).
pub fn rewards(levels: u32) -> Vec<u32> {
    let t = tunables();
    let c = crate::gamedata::get();
    let per = t.levels_per_pool.max(1);
    let pools = levels.div_ceil(per) as usize;
    let [from, to] = t.parts_per_pool;
    let mut items = Vec::new();
    for pool in t.reward_pools.iter().take(pools) {
        let figures: Vec<u32> = pool.iter().filter_map(|m| c.minifigures.get(m)).map(|m| m.template).collect();
        if figures.is_empty() {
            continue;
        }
        let n = from + random_below(to.saturating_sub(from) + 1);
        for _ in 0..n {
            let figure = figures[random_below(figures.len() as u32) as usize];
            items.extend(crate::loot::part_item(figure));
        }
    }
    items
}

/// Drops the rewards' bag for `owner` at `pos` in Treasure Chamber copy `pf`.
fn give_rewards(owner: Id32, pf: Pf, pos: [f32; 3], levels: u32) {
    let items = rewards(levels);
    info!(?owner, levels, parts = items.len(), "Lost Creations: rewards");
    crate::loot::drop_items(owner, pf, pos, owner, items);
}

// ------------------------------------------------------------------ test commands

/// What `!lc` does (devcmd.rs).
#[derive(Clone, Debug)]
pub enum Command {
    /// The run's state.
    Status,
    /// A run alone from level `level` on map `map` (a level id or world name part).
    Start { level: u32, map: Option<String> },
    /// The objective done at once.
    Objective,
    /// The timer set to this many seconds.
    Time(u32),
    /// Across the end at once.
    Finish,
    /// Out of the run.
    End,
    /// The next level copy runs this variant (if its level has it); none: list them.
    Variant(Option<String>),
}

/// Runs `!lc` for `character`; the lines to answer.
pub fn command(character: u32, cmd: Command) -> Vec<String> {
    let mut st = STATE.lock().unwrap();
    match cmd {
        Command::Status => match st.run_of(character) {
            Some(r) => vec![format!(
                "run {} level {} on {} (copy {}), element {}, {:?}, step {}/{} ({} counted), {} of {} across, {} s left",
                r.id,
                r.level,
                level(r.current).name,
                r.pf.copy(),
                r.element,
                r.phase,
                r.step + 1,
                level(r.current).quest.steps(),
                r.progress,
                r.finished.len(),
                r.members.len(),
                r.deadline.map_or(0, |d| d.saturating_duration_since(Instant::now()).as_secs())
            )],
            None => vec![format!("not in a run; levels: {}", LEVELS.iter().map(|l| l.id.trim_start_matches("lc/")).collect::<Vec<_>>().join(", "))],
        },
        Command::Start { level: n, map } => {
            if st.run_of.contains_key(&character) {
                return vec!["already in a run (!lc end first)".into()];
            }
            let chosen = match map {
                Some(m) => {
                    let m = m.to_ascii_lowercase();
                    match LEVELS.iter().position(|l| l.id.trim_start_matches("lc/") == m || l.name.to_ascii_lowercase().contains(&m)) {
                        Some(i) => Some(i),
                        None => return vec![format!("no level {m}")],
                    }
                }
                None => None,
            };
            start_run(&mut st, vec![character], Some((n, chosen)));
            vec![format!("Lost Creations: level {n}")]
        }
        Command::Objective => match st.run_of(character) {
            Some(r) if r.step + 1 < level(r.current).quest.steps() => {
                objective_done(r);
                vec!["objective done".into()]
            }
            Some(_) => vec!["no objective left (reach the end)".into()],
            None => vec!["not in a run".into()],
        },
        Command::Time(s) => match st.run_of(character).map(|r| r.phase) {
            Some(Phase::Running) => {
                // a new deadline: the old timer task gives up
                let generation = st.generation();
                let r = st.run_of(character).unwrap();
                r.generation = generation;
                let time = Duration::from_secs(s.into());
                r.deadline = Some(Instant::now() + time);
                let (run_id, members, finished) = (r.id, r.members.clone(), r.finished.clone());
                let buff = tunables().timer_buff;
                for m in members.into_iter().filter(|m| !finished.contains(m)) {
                    let me = player(m);
                    records(m, vec![crate::effects::buff_update_counter(me, buff, me, 0, s * 100)]);
                }
                tokio::spawn(async move {
                    tokio::time::sleep(time).await;
                    time_up(run_id, generation);
                });
                vec![format!("{s} s left")]
            }
            Some(_) => vec!["the timer isn't running".into()],
            None => vec!["not in a run".into()],
        },
        Command::Finish => match st.run_of(character) {
            Some(r) if r.phase == Phase::Running && !r.finished.contains(&character) => {
                if r.step + 1 < level(r.current).quest.steps() {
                    objective_done(r);
                }
                finish(r, character);
                vec!["across the end".into()]
            }
            Some(_) => vec!["the timer isn't running, or you're across already".into()],
            None => vec!["not in a run".into()],
        },
        Command::End => {
            if st.run_of.contains_key(&character) {
                leave_run(&mut st, character);
                vec!["out of the run".into()]
            } else {
                vec!["not in a run".into()]
            }
        }
        Command::Variant(None) => LEVELS
            .iter()
            .filter(|l| !l.variants.is_empty())
            .map(|l| format!("{}: {}", l.name, l.variants.iter().map(|v| format!("{} (weight {})", v.id, v.weight)).collect::<Vec<_>>().join(", ")))
            .chain(FORCED_VARIANT.lock().unwrap().as_ref().map(|v| format!("next level copy: {v}")))
            .collect(),
        Command::Variant(Some(v)) => {
            let levels: Vec<&str> = LEVELS.iter().filter(|l| l.variants.iter().any(|x| x.id == v)).map(|l| l.name.as_str()).collect();
            if levels.is_empty() {
                return vec![format!("no level has a variant {v} (!lc variant lists them)")];
            }
            let line = format!("the next copy of {} runs variant {v}", levels.join(" or "));
            *FORCED_VARIANT.lock().unwrap() = Some(v);
            vec![line]
        }
    }
}

/// Test targets: the entry door, each level's start.
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    let mut v: Vec<_> = entry_point().map(|(pf, p)| ("the Lost Creations door".to_string(), pf, p)).into_iter().collect();
    v.extend(LEVELS.iter().map(|l| (format!("{} start", l.name), l.model, l.start.0)));
    v.extend(treasure_chamber().map(|(model, _, _)| ("the Treasure Chamber's reward chest".to_string(), model, tunables().reward_chest_pos)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_read() {
        // the 11 finished maps
        assert_eq!(LEVELS.len(), 11, "{:?}", LEVELS.iter().map(|l| &l.id).collect::<Vec<_>>());
        let candy = &LEVELS[level_of_model(7104).unwrap()];
        assert_eq!((candy.button.guid, candy.door.guid, candy.exit_open), (549_964, 595_122, true));
        assert!(candy.exit_shield.is_none());
        assert_eq!(candy.quest.steps(), 1);
        let gk = &LEVELS[level_of_model(7600).unwrap()];
        assert_eq!(gk.mode, LcMode::Gatekeeper);
        assert_eq!(gk.quest.steps(), 2);
        assert!(gk.quest.counted(0).is_some());
        assert!(entry_point().is_some_and(|(pf, _)| pf == 6200));
    }

    #[test]
    fn waves_and_points() {
        // Jungle Arena: the survival script on the designers' pads (only the test variant has
        // enemies so far)
        let ja = &LEVELS[level_of_model(7400).unwrap()];
        assert_eq!(ja.script.as_deref(), Some("script/7400/survival"));
        assert!(!ja.points.is_empty() && ja.points.len() <= 12, "{:?}", ja.points);
        assert!(ja.points.contains(&[234.15, 42.0, 297.23]), "NW_1");
        assert!(!ja.has_waves(None));
        assert!(ja.has_waves(Some(&"monster-lab".to_string())));
        // the Gatekeeper's Realm: a Gatekeeper per variant, all on one point
        let gk = &LEVELS[level_of_model(7600).unwrap()];
        assert_eq!(gk.variants.len(), 11);
        assert_eq!(gk.points, vec![[240.0, 42.0, 277.0]]);
        for v in &gk.variants {
            assert!(gk.has_waves(Some(&v.id)), "{}", v.id);
        }
        // its spawns face west
        let c = crate::gamedata::get();
        let s = &c.spawns["spawn/7600/gatekeeper-red-drake"];
        assert_eq!(s.rot.map(|r| r.heading()), Some(-1.571));
    }

    #[test]
    fn levels_picked() {
        // the Gatekeeper's on every 5th level and only then; the others at random, all of them
        let gk = level_of_model(7600).unwrap();
        assert_eq!(pick_level(5), Some(gk));
        assert_eq!(pick_level(10), Some(gk));
        let picked: HashSet<usize> = (0..500).filter_map(|_| pick_level(1)).collect();
        assert!(!picked.contains(&gk));
        assert_eq!(picked.len(), LEVELS.len() - 1);
    }

    #[test]
    fn rewards_per_pool() {
        // nothing before a level is done; then up to 2 per unlocked pool, a pool per 2 levels
        assert!(rewards(0).is_empty());
        assert_eq!(tunables().reward_pools.len(), 18);
        for _ in 0..20 {
            assert!(rewards(2).len() <= 2);
            assert!(rewards(3).len() <= 4);
            assert!(rewards(36).len() <= 36);
        }
        assert!((0..50).any(|_| !rewards(36).is_empty()));
    }

    #[test]
    fn window_args_as_live() {
        // live's continue window: type 1, level 0, "" (u16 0), ctx, 0, client data
        let w = Writer::new().u32(WINDOW_CONTINUE).u32(0).u16(0).i32(0x22a4_a381).u32(0).u32(654_716).finish();
        assert_eq!(crate::conn::hex(&w), "0000000100000000000022a4a381000000000009fd7c");
    }
}
