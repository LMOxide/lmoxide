//! Survey mode (`--mode survey`, docs/SURVEY.md): nothing moves or attacks; surveyors place,
//! confirm, edit and delete the enemies, NPCs and smashables of each playfield, and mark areas,
//! scripts and waves, through the web UI (`/v1/survey` on the control interface, [`routes`]).
//!
//! Every object spawned here has a survey id: a base object's (`lmoxide_survey::ids::base_id`)
//! or a placement's. Their work is the survey set (`--survey-set`), saved after every change.
//! Hiding is per surveyor and never saved: what a surveyor hid (by hand, or in a wave of a
//! script they don't show) isn't sent to them ([`hides`], used by `world::join`).
//!
//! Locks: `STATE`, then `npc::NPCS`, then `world::MEMBERS`, then `HIDDEN`. `npc::hit` calls
//! [`attacked`] before it locks NPCS, and `world::join` reads only `HIDDEN`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use lmoxide_net::types::Id32;
use lmoxide_survey::{
    self as sv, Kind as SKind, Note, Origin, Placement, Script, TriggerWall, WallAction, WaveRef,
    apply::{Base, Category, Spawn},
    ids,
    merge::{self, Prefer, Tolerance},
    set::{Set, SetInfo},
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    npc::{self, Kind, SpawnDef},
    world,
};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether this is a survey server.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// How far a regenerated guess may have moved and still be the one a placement replaces.
const MATCH_DISTANCE: f32 = 1.0;
/// What marks a selected character: the champions' looping feet aura
/// (particle_generic_champion_aura, "XX_champion feet aura castable").
const SELECT_EFFECT: u32 = 162435;
/// Selected smashables and trigger walls show no effects themselves (simple dynels), and the
/// feet aura played at a ground spot shows on its owner: they get an invisible character at
/// their spot ("invisible_monsterdata", the marker dynels' mesh), sent only to the surveyor,
/// wearing the aura.
const INVISIBLE_LOOK: u32 = 51057;
/// Character instances of those helpers (the client's NPC range, under 16,000,000).
static NEXT_HELPER: AtomicU32 = AtomicU32::new(0x00f0_0000);
/// CARS group that lets players attack (select) friendly NPCs in survey mode.
const CARS_ATTACKABLE: u32 = 3;

struct Object {
    id: String,
    playfield: u32,
    category: Category,
    /// As spawned, but with its plain name; `rot` is the heading.
    def: SpawnDef,
    wave: Option<WaveRef>,
    replaces: Option<Origin>,
    comment: String,
    author: String,
    updated: u64,
    npc: Option<Id32>,
}

#[derive(Default)]
struct Surveyor {
    selected: Option<String>,
    hidden: HashSet<String>,
    /// script id -> wave id or "all"
    shown: HashMap<String, String>,
    wearing: Option<Placeable>,
    /// The running selection marker: the character wearing it (the helper's, if one was
    /// spawned for this surveyor) and the effect instance.
    marker: Option<(Id32, u32, bool)>,
}

struct State {
    dir: PathBuf,
    set: Set,
    /// The base content of each playfield (what `npc::start` spawns elsewhere).
    bases: BTreeMap<u32, Vec<SpawnDef>>,
    objects: HashMap<String, Object>,
    by_npc: HashMap<Id32, String>,
    revision: HashMap<u32, u64>,
    surveyors: HashMap<Id32, Surveyor>,
    catalog: Vec<(Placeable, SpawnDef)>,
    /// Trigger walls by id.
    walls: HashMap<String, Wall>,
    /// Where each player was at the last wall check (playfield, position).
    last_pos: HashMap<Id32, (u32, [f32; 3])>,
}

static STATE: LazyLock<Mutex<Option<State>>> = LazyLock::new(|| Mutex::new(None));
/// viewer -> the NPCs not sent to them.
static HIDDEN: LazyLock<Mutex<HashMap<Id32, HashSet<Id32>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Makes this a survey server working on the set in `dir`.
pub fn init(dir: PathBuf) {
    let set = match Set::load(&dir) {
        Ok(s) => s,
        Err(e) => {
            warn!("survey set {}: {e}; starting an empty one (the folder is overwritten on the first change)", dir.display());
            Set::default()
        }
    };
    let placements: usize = set.placements.values().map(|f| f.placements.len()).sum();
    info!(dir = %dir.display(), placements, "survey mode");
    ENABLED.store(true, Ordering::Relaxed);
    *STATE.lock().unwrap() = Some(State {
        dir,
        set,
        bases: BTreeMap::new(),
        objects: HashMap::new(),
        by_npc: HashMap::new(),
        revision: HashMap::new(),
        surveyors: HashMap::new(),
        catalog: Vec::new(),
        walls: HashMap::new(),
        last_pos: HashMap::new(),
    });
}

/// Whether `thing` isn't sent to `viewer` (what they hid).
pub fn hides(viewer: Id32, thing: Id32) -> bool {
    enabled() && HIDDEN.lock().unwrap().get(&viewer).is_some_and(|h| h.contains(&thing))
}

// ------------------------------------------------------------------ conversions

fn skind(k: Kind) -> SKind {
    match k {
        Kind::Enemy => SKind::Enemy,
        Kind::Smashable => SKind::Smashable,
        Kind::Npc | Kind::Fighter | Kind::Pet => SKind::Npc,
    }
}

fn nkind(k: SKind) -> Result<Kind, Error> {
    match k {
        SKind::Enemy => Ok(Kind::Enemy),
        SKind::Npc => Ok(Kind::Npc),
        SKind::Smashable => Ok(Kind::Smashable),
        SKind::Buildie | SKind::Interactable => Err(Error::Invalid("buildies and interactables can't be placed yet".into())),
    }
}

fn nonzero(v: u32) -> Option<u32> {
    (v != 0).then_some(v)
}

fn walks(k: Kind) -> bool {
    matches!(k, Kind::Enemy | Kind::Npc | Kind::Fighter)
}

fn label(c: Category) -> &'static str {
    match c {
        Category::Live => "live",
        Category::Demo => "demo",
        Category::Client => "client",
        Category::Guessed => "guess",
        Category::Surveyed => "surveyed",
    }
}

/// Heading (radians, facing (sin, cos) in x/z) of a facing vector.
fn heading_of(facing: [f32; 2]) -> f32 {
    facing[0].atan2(facing[1])
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Health for `level`, keeping how much tougher than a regular enemy of its level `def` is
/// (champions and bosses stay champions and bosses).
fn health_at(def: &SpawnDef, level: u32) -> u32 {
    if def.kind != Kind::Enemy || level == def.level {
        return def.health.max(1);
    }
    let ratio = def.health as f32 / npc::regular_life(def.level).max(1) as f32;
    ((npc::regular_life(level) as f32 * ratio).round() as u32).max(1)
}

impl Object {
    fn base_origin(&self) -> Origin {
        Origin {
            id: self.id.clone(),
            kind: skind(self.def.kind),
            monster_data: nonzero(self.def.monster_data),
            template: nonzero(self.def.template),
            pos: self.def.pos,
        }
    }

    fn placement(&self) -> Placement {
        Placement {
            id: self.id.clone(),
            kind: skind(self.def.kind),
            name: self.def.name.clone(),
            monster_data: nonzero(self.def.monster_data),
            template: nonzero(self.def.template),
            level: Some(self.def.level),
            pos: self.def.pos,
            heading: self.def.rot.unwrap_or(0.0),
            replaces: self.replaces.clone(),
            removed: false,
            wave: self.wave.clone(),
            patrol: self.def.patrol.clone(),
            comment: self.comment.clone(),
            author: self.author.clone(),
            updated: self.updated,
        }
    }

    /// What is spawned: named by its category, and attackable (selectable).
    fn spawn_def(&self) -> SpawnDef {
        let mut def = self.def.clone();
        def.name = format!("[{}] {}", label(self.category), def.name);
        if def.kind == Kind::Fighter {
            def.kind = Kind::Npc;
        }
        if def.kind == Kind::Npc {
            def.cars_group = Some(CARS_ATTACKABLE);
        }
        def
    }

    fn view(&self) -> SurveyObject {
        SurveyObject {
            id: self.id.clone(),
            kind: skind(self.def.kind).as_str().into(),
            name: self.def.name.clone(),
            monster_data: nonzero(self.def.monster_data),
            template: nonzero(self.def.template),
            level: Some(self.def.level),
            pos: self.def.pos,
            heading: self.def.rot.unwrap_or(0.0),
            category: self.category,
            wave: self.wave.clone(),
            patrol: self.def.patrol.clone(),
            comment: self.comment.clone(),
            author: self.author.clone(),
            updated: self.updated,
            playfield: self.playfield,
            replaces: self.replaces.clone(),
            size: None,
            color: None,
            action: None,
            once: None,
        }
    }
}

// ------------------------------------------------------------------ startup

/// Spawns the base content (`npc::start`'s list) with the survey set applied: guesses the set
/// replaces are left out, its placements (waves too: every wave is there, surveyors choose
/// which they see) spawned.
pub fn start(all: Vec<(u32, SpawnDef)>) {
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    for (pf, def) in all {
        st.bases.entry(pf).or_default().push(def);
    }
    st.catalog = catalog_of(&st.bases);
    let playfields: Vec<u32> = st.bases.keys().chain(st.set.placements.keys()).copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    for pf in playfields {
        build_playfield(st, pf);
    }
    info!(objects = st.objects.len(), walls = st.walls.len(), "survey: content spawned");
    tokio::spawn(watch_walls());
}

/// Spawns playfield `pf`'s objects from its base content and the set.
fn build_playfield(st: &mut State, pf: u32) {
    let empty = Vec::new();
    let defs = st.bases.get(&pf).unwrap_or(&empty);
    let bases: Vec<Base<SpawnDef>> = defs
        .iter()
        .map(|d| Base {
            kind: skind(d.kind),
            monster_data: nonzero(d.monster_data),
            template: nonzero(d.template),
            pos: d.pos,
            category: Category::from_source(d.source.as_deref()),
            data: d.clone(),
        })
        .collect();
    let placements: Vec<Placement> = st.set.placements.get(&pf).map(|f| f.placements.clone()).unwrap_or_default();
    let applied = sv::apply::apply(pf, &bases, &placements, MATCH_DISTANCE);
    for p in &applied.orphans {
        warn!(playfield = pf, placement = %p.id, "survey: the guess this placement replaces is gone; not spawned");
    }
    let mut objects = Vec::new();
    for s in applied.start {
        match s {
            Spawn::Base { id, category, data } => objects.push(Object {
                id,
                playfield: pf,
                category,
                def: data.clone(),
                wave: None,
                replaces: None,
                comment: String::new(),
                author: String::new(),
                updated: 0,
                npc: None,
            }),
            Spawn::Surveyed { placement, replaced } => {
                if let Some(o) = from_placement(&st.catalog, pf, placement, replaced) {
                    objects.push(o);
                }
            }
        }
    }
    for p in applied.waves {
        if let Some(o) = from_placement(&st.catalog, pf, p, None) {
            objects.push(o);
        }
    }
    for mut o in objects {
        spawn(&st.surveyors, &st.set, &mut o);
        if let Some(n) = o.npc {
            st.by_npc.insert(n, o.id.clone());
        }
        st.objects.insert(o.id.clone(), o);
    }
    let walls: Vec<TriggerWall> = st.set.placements.get(&pf).map(|f| f.triggers.iter().filter(|w| !w.removed).cloned().collect()).unwrap_or_default();
    for def in walls {
        let mut w = Wall { def, playfield: pf, pieces: Vec::new(), fired: HashSet::new() };
        spawn_wall(&st.surveyors, &mut w);
        st.walls.insert(w.def.id.clone(), w);
    }
}

/// The catalog's spawn of that look (its spells, NPC template, health, ...), if any.
fn template_of(catalog: &[(Placeable, SpawnDef)], kind: SKind, monster_data: Option<u32>, template: Option<u32>) -> Option<SpawnDef> {
    catalog
        .iter()
        .find(|(p, _)| p.kind == kind && p.monster_data == monster_data && p.template == template)
        .or_else(|| catalog.iter().find(|(p, _)| p.kind == kind && p.monster_data == monster_data))
        .map(|(_, d)| d.clone())
}

fn from_placement(catalog: &[(Placeable, SpawnDef)], pf: u32, p: &Placement, replaced: Option<&SpawnDef>) -> Option<Object> {
    let kind = match nkind(p.kind) {
        Ok(k) => k,
        Err(_) => {
            warn!(playfield = pf, placement = %p.id, kind = p.kind.as_str(), "survey: not spawned (kind not supported yet)");
            return None;
        }
    };
    let mut def = replaced
        .filter(|d| skind(d.kind) == p.kind && nonzero(d.monster_data) == p.monster_data)
        .cloned()
        .or_else(|| template_of(catalog, p.kind, p.monster_data, p.template))
        .unwrap_or_else(|| bare_def(kind, p.monster_data.unwrap_or(0), p.template.unwrap_or(0), p.level.unwrap_or(1)));
    def.kind = if kind == Kind::Npc && def.kind == Kind::Fighter { Kind::Fighter } else { kind };
    let level = p.level.unwrap_or(def.level).max(1);
    def.health = health_at(&def, level);
    def.level = level;
    if !p.name.is_empty() {
        def.name = p.name.clone();
    }
    def.pos = p.pos;
    def.rot = Some(p.heading);
    def.patrol = if walks(def.kind) { p.patrol.clone() } else { Vec::new() };
    def.source = Some("survey".into());
    Some(Object {
        id: p.id.clone(),
        playfield: pf,
        category: Category::Surveyed,
        def,
        wave: p.wave.clone(),
        replaces: p.replaces.clone(),
        comment: p.comment.clone(),
        author: p.author.clone(),
        updated: p.updated,
        npc: None,
    })
}

/// A spawn of a look nothing in the base content uses.
fn bare_def(kind: Kind, monster_data: u32, template: u32, level: u32) -> SpawnDef {
    serde_json::from_value::<SpawnDef>(serde_json::json!({
        "name": "?", "monster_data": monster_data, "template": template, "level": level,
        "health": npc::regular_life(level).max(1), "pos": [0.0, 0.0, 0.0],
    }))
    .map(|d| SpawnDef { kind, ..d })
    .expect("SpawnDef")
}

/// Every look the base content uses, with where it is used most.
/// (kind, name, look, template)
type LookKey = (SKind, String, Option<u32>, Option<u32>);

fn catalog_of(bases: &BTreeMap<u32, Vec<SpawnDef>>) -> Vec<(Placeable, SpawnDef)> {
    // each look: the first spawn of it, and how often each playfield uses it
    let mut seen: BTreeMap<LookKey, (SpawnDef, BTreeMap<u32, u32>)> = BTreeMap::new();
    for (pf, defs) in bases {
        for d in defs {
            let key = (skind(d.kind), d.name.clone(), nonzero(d.monster_data), nonzero(d.template));
            let e = seen.entry(key).or_insert_with(|| (d.clone(), BTreeMap::new()));
            *e.1.entry(*pf).or_default() += 1;
        }
    }
    let mut out: Vec<(Placeable, SpawnDef)> = seen
        .into_iter()
        .map(|((kind, name, monster_data, template), (def, uses))| {
            let total: u32 = uses.values().sum();
            let (top, n) = uses.iter().max_by_key(|(_, n)| **n).map(|(pf, n)| (*pf, *n)).unwrap_or((0, 0));
            let place = crate::teleport::destination(top).map_or_else(|| top.to_string(), |d| d.name.clone());
            let used = if uses.len() > 1 { format!("{place} ×{n}, {total} in all") } else { format!("{place} ×{n}") };
            let level = (kind == SKind::Enemy).then_some(def.level);
            (Placeable { kind, name, monster_data, template, level, used: Some(used) }, def)
        })
        .collect();
    out.sort_by(|a, b| (a.0.kind, &a.0.name).cmp(&(b.0.kind, &b.0.name)));
    out
}

// ------------------------------------------------------------------ the world

/// Spawns the object under a new NPC id (the client needs ~1.5 s before an id is reused).
/// Surveyors who hide it have the id in their hidden set before it joins the world, so it
/// isn't sent to them.
fn spawn(surveyors: &HashMap<Id32, Surveyor>, set: &Set, o: &mut Object) {
    let def = o.spawn_def();
    let id = npc::new_id(&def);
    {
        let mut hidden = HIDDEN.lock().unwrap();
        for (viewer, s) in surveyors {
            if is_hidden_for(s, o, set) {
                hidden.entry(*viewer).or_default().insert(id);
            }
        }
    }
    o.npc = Some(id);
    npc::spawn_as(id, def, o.playfield);
}

fn despawn(o: &mut Object) {
    if let Some(id) = o.npc.take() {
        npc::despawn(id);
    }
}

/// Spawns `o` again (after an edit), keeping track of its NPC id.
fn respawn(st: &mut State, id: &str) {
    let Some(o) = st.objects.get_mut(id) else { return };
    if let Some(old) = o.npc {
        st.by_npc.remove(&old);
    }
    despawn(o);
    spawn(&st.surveyors, &st.set, o);
    if let Some(new) = o.npc {
        st.by_npc.insert(new, id.to_string());
    }
}

fn bump(st: &mut State, pf: u32) {
    *st.revision.entry(pf).or_default() += 1;
}

fn save(st: &State) {
    if let Err(e) = st.set.save(&st.dir) {
        warn!("survey set {}: not saved: {e}", st.dir.display());
    }
}

fn scripts_of(set: &Set, pf: u32) -> &[Script] {
    set.notes.get(&pf).map_or(&[], |n| n.scripts.as_slice())
}

/// Whether playfield `pf` has that script wave.
fn has_wave(set: &Set, pf: u32, w: &WaveRef) -> bool {
    scripts_of(set, pf).iter().any(|s| s.id == w.script && s.waves.iter().any(|x| x.id == w.wave))
}

/// The wave of a script a surveyor sees: chosen, else the first; "all" for one-wave scripts.
fn shown_wave<'a>(s: &'a Surveyor, script: &'a Script) -> &'a str {
    if script.waves.len() < 2 {
        return "all";
    }
    match s.shown.get(&script.id) {
        Some(w) if w == "all" || script.waves.iter().any(|x| &x.id == w) => w,
        _ => &script.waves[0].id,
    }
}

fn is_hidden_for(s: &Surveyor, o: &Object, set: &Set) -> bool {
    if s.hidden.contains(&o.id) {
        return true;
    }
    let Some(w) = &o.wave else { return false };
    let Some(script) = scripts_of(set, o.playfield).iter().find(|x| x.id == w.script) else { return false };
    let shown = shown_wave(s, script);
    shown != "all" && shown != w.wave
}

/// Brings what `viewer` is sent in line with what they hide: despawns newly hidden objects on
/// their playfield, spawns ones they show again.
fn sync_hidden(st: &State, viewer: Id32) {
    let Some(s) = st.surveyors.get(&viewer) else { return };
    // every spawned id with its playfield and whether the viewer hides it
    let mut all: HashMap<Id32, (u32, bool)> = HashMap::new();
    for o in st.objects.values() {
        if let Some(n) = o.npc {
            all.insert(n, (o.playfield, is_hidden_for(s, o, &st.set)));
        }
    }
    for w in st.walls.values() {
        for id in w.ids() {
            all.insert(id, (w.playfield, s.hidden.contains(&w.def.id)));
        }
    }
    for b in crate::buildie::list(None) {
        all.insert(b.id, (b.playfield, s.hidden.contains(&buildie_id(&b))));
    }
    let want: HashSet<Id32> = all.iter().filter(|(_, (_, h))| *h).map(|(id, _)| *id).collect();
    let had = HIDDEN.lock().unwrap().insert(viewer, want.clone()).unwrap_or_default();
    let here = world::whereabouts(viewer).map(|(pf, _)| pf);
    let on_here = |id: &Id32| all.get(id).is_some_and(|(pf, _)| Some(*pf) == here);
    let mut records = Vec::new();
    for id in want.difference(&had).filter(|id| on_here(id)) {
        records.push(crate::game::despawn_record(*id));
    }
    for id in had.difference(&want).filter(|id| on_here(id)) {
        records.extend(world::spawn_records_of(*id).unwrap_or_default());
    }
    if !records.is_empty() {
        world::send_to(viewer, world::Out::Records(records));
    }
}

fn sync_all(st: &State) {
    for viewer in st.surveyors.keys().copied().collect::<Vec<_>>() {
        sync_hidden(st, viewer);
    }
}

/// Moves the selection marker of `viewer` to their selection (or stops it).
fn mark(st: &mut State, viewer: Id32) {
    let selected = st.surveyors.get(&viewer).and_then(|s| s.selected.clone());
    // (a trigger wall: a ground marker at its foot, as a smashable's)
    // (a trigger wall: as a smashable, at its foot)
    // (a buildie: as a smashable)
    let target = selected.as_ref().and_then(|id| st.objects.get(id)).and_then(|o| o.npc.map(|n| (n, o.def.kind, o.def.pos, o.playfield))).or_else(|| {
        let w = st.walls.get(selected.as_ref()?)?;
        Some((w.pieces.first()?.0, Kind::Smashable, w.def.pos, w.playfield))
    }).or_else(|| buildie(selected.as_ref()?).map(|b| (b.id, Kind::Smashable, b.pos, b.playfield)));
    let Some(s) = st.surveyors.get_mut(&viewer) else { return };
    let mut records = Vec::new();
    if let Some((wearer, instance, helper)) = s.marker.take() {
        records.push(crate::effects::stop_effect_package(wearer, SELECT_EFFECT, 0, instance));
        if helper {
            records.push(crate::game::despawn_record(wearer));
        }
    }
    if let Some((npc, kind, pos, playfield)) = target {
        let instance = crate::effects::new_instance();
        let wearer = if kind == Kind::Smashable {
            let helper = Id32::new(crate::territory::CHARACTER_TYPE, NEXT_HELPER.fetch_add(1, Ordering::Relaxed));
            records.extend(crate::game::remote_spawn_records(
                helper,
                &crate::game::CharSpec {
                    name: "",
                    playfield,
                    pos,
                    monster_data: INVISIBLE_LOOK,
                    level: 1,
                    health: 1,
                    max_health: 1,
                    // nobody can target it (attacks select what is under it)
                    cars_group: 5,
                    npc: Some(lmoxide_net::proto::proto_core_character_iir::NpcStats { template_id: Some(INVISIBLE_LOOK), ..Default::default() }),
                    speeds: &[],
                },
            ));
            helper
        } else {
            npc
        };
        records.push(crate::effects::effect_package_hook(wearer, SELECT_EFFECT, "", "", 0.0, 0, instance));
        s.marker = Some((wearer, instance, kind == Kind::Smashable));
    }
    if !records.is_empty() {
        world::send_to(viewer, world::Out::Records(records));
    }
}

/// Everyone whose selection is `id` (to move their markers after a respawn).
fn mark_selecting(st: &mut State, id: &str) {
    let viewers: Vec<Id32> = st.surveyors.iter().filter(|(_, s)| s.selected.as_deref() == Some(id)).map(|(v, _)| *v).collect();
    for v in viewers {
        mark(st, v);
    }
}

/// A player attacked NPC `target`: it becomes their selection.
pub fn attacked(attacker: Id32, target: Id32) {
    if attacker.kind != crate::territory::CHARACTER_TYPE {
        return;
    }
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    let Some(id) = st.by_npc.get(&target).cloned() else { return };
    let s = st.surveyors.entry(attacker).or_default();
    if s.selected.as_deref() == Some(id.as_str()) {
        return;
    }
    s.selected = Some(id.clone());
    mark(st, attacker);
    if let Some(o) = st.objects.get(&id) {
        crate::chat::system(attacker.instance, &format!("selected [{}] {}", label(o.category), o.def.name));
    }
}

/// A player interacted with buildie `target` (survey servers don't build): it becomes their
/// selection.
pub fn interacted(player: Id32, target: Id32) {
    let Some(b) = crate::buildie::list(None).into_iter().find(|b| b.id == target) else { return };
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    let id = buildie_id(&b);
    let s = st.surveyors.entry(player).or_default();
    if s.selected.as_deref() == Some(id.as_str()) {
        return;
    }
    s.selected = Some(id);
    mark(st, player);
    crate::chat::system(player.instance, &format!("selected [buildie] {}", b.name));
}

// ------------------------------------------------------------------ buildies

/// Buildies (buildie.rs) are listed and can be selected, hidden and walked to, but not changed
/// yet: their survey id is their instance (never saved: selections and hiding aren't).
fn buildie_id(b: &crate::buildie::Listed) -> String {
    format!("buildie-{}", b.id.instance)
}

fn buildie(id: &str) -> Option<crate::buildie::Listed> {
    let instance: u32 = id.strip_prefix("buildie-")?.parse().ok()?;
    crate::buildie::list(None).into_iter().find(|b| b.id.instance == instance)
}

fn buildie_view(b: &crate::buildie::Listed) -> SurveyObject {
    // the heading of the rotation quaternion (x, y, z, w) around the vertical axis
    let heading = 2.0 * b.rot[1].atan2(b.rot[3]);
    SurveyObject {
        id: buildie_id(b),
        kind: SKind::Buildie.as_str().into(),
        name: b.name.clone(),
        monster_data: nonzero(b.monster_data),
        template: nonzero(b.template),
        level: None,
        pos: b.pos,
        heading,
        // the recorded session's, or a module's from the client data (Medusa's mirrors)
        category: if b.recorded { Category::Demo } else { Category::Client },
        wave: None,
        patrol: Vec::new(),
        comment: if b.built { "built".into() } else { String::new() },
        author: String::new(),
        updated: 0,
        playfield: b.playfield,
        replaces: None,
        size: None,
        color: None,
        action: None,
        once: None,
    }
}

// ------------------------------------------------------------------ API shapes

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placeable {
    pub kind: SKind,
    pub name: String,
    #[serde(default)]
    pub monster_data: Option<u32>,
    #[serde(default)]
    pub template: Option<u32>,
    #[serde(default)]
    pub level: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<String>,
}

#[derive(Serialize)]
struct SurveyObject {
    id: String,
    /// enemy, npc, smashable, ... or "trigger" (a trigger wall)
    kind: String,
    name: String,
    monster_data: Option<u32>,
    template: Option<u32>,
    level: Option<u32>,
    pos: [f32; 3],
    heading: f32,
    category: Category,
    wave: Option<WaveRef>,
    patrol: Vec<[f32; 3]>,
    comment: String,
    author: String,
    updated: u64,
    playfield: u32,
    /// The guess it replaces (confirmed or edited), which Reset brings back.
    #[serde(skip_serializing_if = "Option::is_none")]
    replaces: Option<Origin>,
    /// Trigger walls: width, height, depth; look; what crossing does; only once.
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<WallAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    once: Option<bool>,
}

#[derive(Serialize)]
struct PlayerState {
    account_id: u32,
    character_name: String,
    playfield: u32,
    playfield_name: Option<String>,
    pos: [f32; 3],
    heading: f32,
    selected: Option<String>,
    hidden: Vec<String>,
    shown_waves: HashMap<String, String>,
    wearing: Option<Placeable>,
    revision: u64,
}

#[derive(Serialize)]
struct SurveyorInfo {
    account_id: u32,
    username: String,
    character_name: String,
    playfield: Option<u32>,
}

#[derive(Deserialize, Default)]
struct Changes {
    kind: Option<SKind>,
    #[serde(default, deserialize_with = "some")]
    monster_data: Option<Option<u32>>,
    #[serde(default, deserialize_with = "some")]
    template: Option<Option<u32>>,
    patrol: Option<Vec<[f32; 3]>>,
    pos: Option<[f32; 3]>,
    heading: Option<f32>,
    #[serde(default, deserialize_with = "some")]
    level: Option<Option<u32>>,
    name: Option<String>,
    comment: Option<String>,
    #[serde(default, deserialize_with = "some")]
    wave: Option<Option<WaveRef>>,
    /// Trigger walls
    size: Option<[f32; 3]>,
    color: Option<String>,
    action: Option<WallAction>,
    once: Option<bool>,
}

/// A field that may be given as null (`Some(None)`) or left out (`None`).
fn some<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// A whole set (or a part) in one file, as the web UI exports and imports it.
#[derive(Serialize, Deserialize)]
struct Bundle {
    format: u32,
    #[serde(default)]
    info: SetInfo,
    playfields: BTreeMap<String, BundlePlayfield>,
}

#[derive(Serialize, Deserialize, Default)]
struct BundlePlayfield {
    #[serde(default)]
    placements: Vec<Placement>,
    #[serde(default)]
    notes: Vec<Note>,
    #[serde(default)]
    scripts: Vec<Script>,
}

#[derive(Deserialize)]
struct MergeOptions {
    #[serde(default = "default_distance")]
    distance: f32,
    #[serde(default = "default_heading")]
    heading: f32,
    #[serde(default)]
    prefer: Option<String>,
    #[serde(default)]
    take: Vec<String>,
}

fn default_distance() -> f32 {
    1.0
}
fn default_heading() -> f32 {
    20.0
}

// ------------------------------------------------------------------ errors

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    Invalid(String),
    Off,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Error::NotFound(m) => (StatusCode::NOT_FOUND, "not_found", m),
            Error::Invalid(m) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid", m),
            Error::Off => (StatusCode::NOT_FOUND, "not_found", "this server is not a survey server (--mode survey)".into()),
        };
        (status, Json(serde_json::json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}

type R<T> = Result<Json<T>, Error>;

/// Runs `f` on the survey state.
fn with<T>(f: impl FnOnce(&mut State) -> Result<T, Error>) -> Result<T, Error> {
    let mut guard = STATE.lock().unwrap();
    let st = guard.as_mut().ok_or(Error::Off)?;
    f(st)
}

/// The character of `account` online here.
fn player_of(account: u32) -> Result<(Id32, u32), Error> {
    let character = crate::sessions::characters_of(account).into_iter().next().ok_or_else(|| Error::NotFound(format!("account {account} has no character online")))?;
    let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
    let (pf, _) = world::whereabouts(id).ok_or_else(|| Error::NotFound(format!("account {account} isn't in a playfield yet")))?;
    Ok((id, pf))
}

/// Who made the request (lmoxide-api's actor: "account:<name>" or "key:<name>"): the name.
fn author_of(headers: &HeaderMap) -> String {
    let actor = headers.get("x-lmoxide-actor").and_then(|v| v.to_str().ok()).unwrap_or("");
    let name = actor.split_once(':').map_or(actor, |(_, n)| n);
    name.chars().take(64).collect()
}

// ------------------------------------------------------------------ routes

/// The `/v1/survey` routes of the control interface (docs/SURVEY.md "API").
pub fn routes() -> Router {
    Router::new()
        .route("/v1/survey/surveyors", get(surveyors))
        .route("/v1/survey/catalog", get(catalog))
        .route("/v1/survey/players/{account}/state", get(player_state))
        .route("/v1/survey/players/{account}/wear", post(wear))
        .route("/v1/survey/players/{account}/place", post(place))
        .route("/v1/survey/players/{account}/place-trigger", post(place_trigger))
        .route("/v1/survey/wall-looks", get(wall_looks))
        .route("/v1/survey/trigger-targets", get(trigger_targets))
        .route("/v1/survey/players/{account}/select", post(select))
        .route("/v1/survey/players/{account}/hide", post(hide))
        .route("/v1/survey/players/{account}/unhide-all", post(unhide_all))
        .route("/v1/survey/players/{account}/shown-wave", post(shown_wave_route))
        .route("/v1/survey/players/{account}/preview-wave", post(preview_wave))
        .route("/v1/survey/players/{account}/teleport", post(teleport))
        .route("/v1/survey/playfields/{pf}/objects", get(objects))
        .route("/v1/survey/playfields/{pf}/notes", get(notes))
        .route("/v1/survey/playfields/{pf}/notes/{id}", put(save_note).delete(delete_note))
        .route("/v1/survey/playfields/{pf}/scripts/{id}", put(save_script).delete(delete_script))
        .route("/v1/survey/objects/{id}", axum::routing::patch(edit).delete(remove).put(put_object))
        .route("/v1/survey/objects/{id}/confirm", post(confirm))
        .route("/v1/survey/objects/{id}/forget", post(forget))
        .route("/v1/survey/set", get(set_info).put(save_set_info))
        .route("/v1/survey/export", get(export))
        .route("/v1/survey/import", post(import))
}

async fn surveyors() -> R<Vec<SurveyorInfo>> {
    with(|_| Ok(()))?;
    Ok(Json(
        crate::sessions::players()
            .into_iter()
            .map(|p| SurveyorInfo { account_id: p.account_id, username: p.username, character_name: p.character_name, playfield: p.playfield })
            .collect(),
    ))
}

async fn catalog() -> R<Vec<Placeable>> {
    with(|st| Ok(Json(st.catalog.iter().map(|(p, _)| p.clone()).collect())))
}

fn state_of(st: &mut State, account: u32) -> Result<PlayerState, Error> {
    let (id, pf) = player_of(account)?;
    let s = st.surveyors.entry(id).or_default();
    let name = crate::sessions::players().into_iter().find(|p| p.account_id == account).map(|p| p.character_name).unwrap_or_default();
    Ok(PlayerState {
        account_id: account,
        character_name: name,
        playfield: pf,
        playfield_name: crate::teleport::destination(pf).map(|d| d.name.clone()),
        pos: world::live_position(id).unwrap_or_default(),
        heading: heading_of(world::facing_of(id).unwrap_or([0.0, 1.0])),
        selected: s.selected.clone(),
        hidden: s.hidden.iter().cloned().collect(),
        shown_waves: s.shown.clone(),
        wearing: s.wearing.clone(),
        revision: st.revision.get(&pf).copied().unwrap_or(0),
    })
}

async fn player_state(Path(account): Path<u32>) -> R<PlayerState> {
    with(|st| state_of(st, account)).map(Json)
}

#[derive(Deserialize)]
struct WearBody {
    what: Option<Placeable>,
}

async fn wear(Path(account): Path<u32>, Json(b): Json<WearBody>) -> R<PlayerState> {
    with(|st| {
        let (id, _) = player_of(account)?;
        let command = match &b.what {
            None => "!skin reset".to_string(),
            Some(p) => match (p.kind, p.monster_data) {
                (SKind::Enemy | SKind::Npc | SKind::Smashable, Some(md)) => format!("!skin {md}"),
                _ => return Err(Error::Invalid(format!("{} can't be worn yet (no ghost for buildies and interactables)", p.name))),
            },
        };
        world::send_to(id, world::Out::Command(command, None));
        st.surveyors.entry(id).or_default().wearing = b.what;
        state_of(st, account)
    })
    .map(Json)
}

#[derive(Deserialize)]
struct PlaceBody {
    what: Placeable,
    #[serde(default)]
    wave: Option<WaveRef>,
}

async fn place(Path(account): Path<u32>, headers: HeaderMap, Json(b): Json<PlaceBody>) -> R<SurveyObject> {
    with(|st| {
        let (player, pf) = player_of(account)?;
        let kind = nkind(b.what.kind)?;
        let pos = world::live_position(player).unwrap_or_default();
        let heading = heading_of(world::facing_of(player).unwrap_or([0.0, 1.0]));
        let author = Some(author_of(&headers)).filter(|a| !a.is_empty()).unwrap_or_else(|| crate::sessions::players().into_iter().find(|p| p.account_id == account).map(|p| p.character_name).unwrap_or_default());
        let mut def = template_of(&st.catalog, b.what.kind, b.what.monster_data, b.what.template)
            .unwrap_or_else(|| bare_def(kind, b.what.monster_data.unwrap_or(0), b.what.template.unwrap_or(0), b.what.level.unwrap_or(1)));
        let level = b.what.level.unwrap_or(def.level).max(1);
        def.health = health_at(&def, level);
        def.level = level;
        def.name = b.what.name.clone();
        def.pos = pos;
        def.rot = Some(heading);
        def.patrol.clear();
        def.source = Some("survey".into());
        if b.wave.as_ref().is_some_and(|w| !has_wave(&st.set, pf, w)) {
            return Err(Error::Invalid("no such script wave in this playfield".into()));
        }
        let mut o = Object {
            id: ids::new_id("p", &author),
            playfield: pf,
            category: Category::Surveyed,
            def,
            wave: b.wave.clone(),
            replaces: None,
            comment: String::new(),
            author,
            updated: ids::now(),
            npc: None,
        };
        st.set.placements_mut(pf).placements.push(o.placement());
        save(st);
        let s = st.surveyors.entry(player).or_default();
        s.selected = Some(o.id.clone());
        if let Some(w) = &b.wave {
            // placing into a wave shows that wave
            s.shown.insert(w.script.clone(), w.wave.clone());
        }
        let view = o.view();
        let id = o.id.clone();
        spawn(&st.surveyors, &st.set, &mut o);
        if let Some(n) = o.npc {
            st.by_npc.insert(n, id.clone());
        }
        st.objects.insert(id, o);
        sync_all(st);
        mark(st, player);
        bump(st, pf);
        Ok(view)
    })
    .map(Json)
}

#[derive(Deserialize)]
struct SelectBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    nearest: bool,
}

async fn select(Path(account): Path<u32>, Json(b): Json<SelectBody>) -> R<PlayerState> {
    with(|st| {
        let (player, pf) = player_of(account)?;
        let id = if b.nearest {
            let here = world::live_position(player).unwrap_or_default();
            let surveyor = st.surveyors.get(&player);
            let objects = st.objects.values().filter(|o| o.playfield == pf && surveyor.is_none_or(|s| !is_hidden_for(s, o, &st.set))).map(|o| (&o.id, o.def.pos));
            let walls = st.walls.values().filter(|w| w.playfield == pf && surveyor.is_none_or(|s| !s.hidden.contains(&w.def.id))).map(|w| (w.def.id.clone(), w.def.pos));
            let buildies = crate::buildie::list(Some(pf)).into_iter().map(|b| (buildie_id(&b), b.pos)).filter(|(id, _)| surveyor.is_none_or(|s| !s.hidden.contains(id)));
            objects
                .map(|(id, p)| (id.clone(), p))
                .chain(walls)
                .chain(buildies)
                .min_by(|a, b| distance(a.1, here).total_cmp(&distance(b.1, here)))
                .map(|(id, _)| id)
        } else {
            if let Some(id) = b.id.as_ref().filter(|id| !st.objects.contains_key(*id) && !st.walls.contains_key(*id) && buildie(id).is_none()) {
                return Err(Error::NotFound(format!("no object {id}")));
            }
            b.id
        };
        st.surveyors.entry(player).or_default().selected = id;
        mark(st, player);
        state_of(st, account)
    })
    .map(Json)
}

#[derive(Deserialize)]
struct HideBody {
    ids: Vec<String>,
    hidden: bool,
}

async fn hide(Path(account): Path<u32>, Json(b): Json<HideBody>) -> R<PlayerState> {
    with(|st| {
        let (player, _) = player_of(account)?;
        let s = st.surveyors.entry(player).or_default();
        for id in b.ids {
            if b.hidden {
                s.hidden.insert(id);
            } else {
                s.hidden.remove(&id);
            }
        }
        sync_hidden(st, player);
        state_of(st, account)
    })
    .map(Json)
}

async fn unhide_all(Path(account): Path<u32>) -> R<PlayerState> {
    with(|st| {
        let (player, _) = player_of(account)?;
        st.surveyors.entry(player).or_default().hidden.clear();
        sync_hidden(st, player);
        state_of(st, account)
    })
    .map(Json)
}

#[derive(Deserialize)]
struct ShownWaveBody {
    script: String,
    wave: String,
}

async fn shown_wave_route(Path(account): Path<u32>, Json(b): Json<ShownWaveBody>) -> R<PlayerState> {
    with(|st| {
        let (player, _) = player_of(account)?;
        st.surveyors.entry(player).or_default().shown.insert(b.script, b.wave);
        sync_hidden(st, player);
        state_of(st, account)
    })
    .map(Json)
}

#[derive(Deserialize)]
struct WaveBody {
    wave: WaveRef,
}

/// Shows the wave and spawns its placements again, so they pop in as the trigger would.
async fn preview_wave(Path(account): Path<u32>, Json(b): Json<WaveBody>) -> R<PlayerState> {
    with(|st| {
        let (player, pf) = player_of(account)?;
        preview(st, player, pf, &b.wave);
        state_of(st, account)
    })
    .map(Json)
}

#[derive(Deserialize)]
struct TeleportBody {
    pos: [f32; 3],
}

async fn teleport(Path(account): Path<u32>, Json(b): Json<TeleportBody>) -> R<PlayerState> {
    with(|st| {
        let (player, _) = player_of(account)?;
        let heading = heading_of(world::facing_of(player).unwrap_or([0.0, 1.0]));
        world::send_to(player, world::Out::Teleport(None, b.pos, heading));
        state_of(st, account)
    })
    .map(Json)
}

#[derive(Serialize)]
struct ObjectList {
    revision: u64,
    objects: Vec<SurveyObject>,
}

async fn objects(Path(pf): Path<u32>) -> R<ObjectList> {
    with(|st| {
        Ok(Json(ObjectList {
            revision: st.revision.get(&pf).copied().unwrap_or(0),
            objects: st
                .objects
                .values()
                .filter(|o| o.playfield == pf)
                .map(Object::view)
                .chain(st.walls.values().filter(|w| w.playfield == pf).map(Wall::view))
                .chain(crate::buildie::list(Some(pf)).iter().map(buildie_view))
                .collect(),
        }))
    })
}

#[derive(Serialize)]
struct NotesView {
    notes: Vec<Note>,
    scripts: Vec<Script>,
}

async fn notes(Path(pf): Path<u32>) -> R<NotesView> {
    with(|st| {
        let n = st.set.notes.get(&pf);
        Ok(Json(NotesView { notes: n.map(|n| n.notes.clone()).unwrap_or_default(), scripts: n.map(|n| n.scripts.clone()).unwrap_or_default() }))
    })
}

fn tidy_notes(st: &mut State, pf: u32) {
    if st.set.notes.get(&pf).is_some_and(|n| n.notes.is_empty() && n.scripts.is_empty()) {
        st.set.notes.remove(&pf);
    }
}

async fn save_note(Path((pf, id)): Path<(u32, String)>, headers: HeaderMap, Json(mut n): Json<Note>) -> R<Note> {
    with(|st| {
        n.id = id;
        n.author = Some(author_of(&headers)).filter(|a| !a.is_empty()).unwrap_or(n.author);
        n.updated = ids::now();
        let notes = &mut st.set.notes_mut(pf).notes;
        match notes.iter_mut().find(|x| x.id == n.id) {
            Some(x) => *x = n.clone(),
            None => notes.push(n.clone()),
        }
        save(st);
        bump(st, pf);
        Ok(n)
    })
    .map(Json)
}

async fn delete_note(Path((pf, id)): Path<(u32, String)>) -> Result<StatusCode, Error> {
    with(|st| {
        st.set.notes_mut(pf).notes.retain(|n| n.id != id);
        tidy_notes(st, pf);
        save(st);
        bump(st, pf);
        Ok(StatusCode::NO_CONTENT)
    })
}

async fn save_script(Path((pf, id)): Path<(u32, String)>, headers: HeaderMap, Json(mut s): Json<Script>) -> R<Script> {
    with(|st| {
        if s.waves.is_empty() {
            return Err(Error::Invalid("a script has at least one wave".into()));
        }
        s.id = id;
        s.author = Some(author_of(&headers)).filter(|a| !a.is_empty()).unwrap_or(s.author);
        s.updated = ids::now();
        let scripts = &mut st.set.notes_mut(pf).scripts;
        match scripts.iter_mut().find(|x| x.id == s.id) {
            Some(x) => *x = s.clone(),
            None => scripts.push(s.clone()),
        }
        save(st);
        sync_all(st);
        bump(st, pf);
        Ok(s)
    })
    .map(Json)
}

async fn delete_script(Path((pf, id)): Path<(u32, String)>) -> Result<StatusCode, Error> {
    with(|st| {
        if st.objects.values().any(|o| o.wave.as_ref().is_some_and(|w| w.script == id)) {
            return Err(Error::Invalid("delete or move the script's placements first".into()));
        }
        st.set.notes_mut(pf).scripts.retain(|s| s.id != id);
        tidy_notes(st, pf);
        save(st);
        sync_all(st);
        bump(st, pf);
        Ok(StatusCode::NO_CONTENT)
    })
}

/// A guess becomes a surveyed placement that replaces it (new id); others are returned as they are.
fn surveyed(st: &mut State, id: &str, author: &str) -> Result<String, Error> {
    let o = st.objects.get(id).ok_or_else(|| Error::NotFound(format!("no object {id}")))?;
    match o.category {
        Category::Surveyed => Ok(id.to_string()),
        Category::Guessed => {
            let mut o = st.objects.remove(id).expect("checked");
            o.replaces = Some(o.base_origin());
            o.id = ids::new_id("p", if author.is_empty() { "web" } else { author });
            o.category = Category::Surveyed;
            o.def.source = Some("survey".into());
            o.author = author.to_string();
            let new = o.id.clone();
            if let Some(n) = o.npc {
                st.by_npc.insert(n, new.clone());
            }
            for s in st.surveyors.values_mut() {
                if s.selected.as_deref() == Some(id) {
                    s.selected = Some(new.clone());
                }
                if s.hidden.remove(id) {
                    s.hidden.insert(new.clone());
                }
            }
            st.objects.insert(new.clone(), o);
            Ok(new)
        }
        c => Err(Error::Invalid(format!("{} objects can only be hidden", label(c)))),
    }
}

/// Stores the object's placement in the set (replacing the one with its id).
fn store(st: &mut State, id: &str) {
    let Some(o) = st.objects.get(id) else { return };
    let p = o.placement();
    let list = &mut st.set.placements_mut(o.playfield).placements;
    match list.iter_mut().find(|x| x.id == p.id) {
        Some(x) => *x = p,
        None => list.push(p),
    }
    save(st);
}

async fn edit(Path(id): Path<String>, headers: HeaderMap, Json(c): Json<Changes>) -> R<SurveyObject> {
    with(|st| {
        let author = author_of(&headers);
        if st.walls.contains_key(&id) {
            return edit_wall(st, &id, c, author);
        }
        let id = surveyed(st, &id, &author)?;
        let catalog = st.catalog.clone();
        let pf = st.objects[&id].playfield;
        if let Some(Some(w)) = &c.wave
            && !has_wave(&st.set, pf, w)
        {
            return Err(Error::Invalid("no such script wave in this playfield".into()));
        }
        let o = st.objects.get_mut(&id).expect("surveyed");
        if let Some(kind) = c.kind {
            // changed into another spawn: that look's spells, template, health
            let k = nkind(kind)?;
            let md = c.monster_data.flatten();
            let tpl = c.template.flatten();
            let mut def = template_of(&catalog, kind, md, tpl).unwrap_or_else(|| bare_def(k, md.unwrap_or(0), tpl.unwrap_or(0), o.def.level));
            def.kind = k;
            def.pos = o.def.pos;
            def.rot = o.def.rot;
            def.patrol = if walks(k) { o.def.patrol.clone() } else { Vec::new() };
            def.source = o.def.source.clone();
            o.def = def;
        }
        if let Some(p) = c.pos {
            o.def.pos = p;
        }
        if let Some(h) = c.heading {
            o.def.rot = Some(h);
        }
        if let Some(Some(level)) = c.level {
            let level = level.clamp(1, 100);
            o.def.health = health_at(&o.def, level);
            o.def.level = level;
        }
        if let Some(n) = c.name.filter(|n| !n.trim().is_empty()) {
            o.def.name = n;
        }
        if let Some(t) = c.comment {
            o.comment = t;
        }
        if let Some(w) = c.wave {
            o.wave = w;
        }
        if let Some(p) = c.patrol {
            o.def.patrol = if walks(o.def.kind) { p } else { Vec::new() };
        }
        if !author.is_empty() {
            o.author = author;
        }
        o.updated = ids::now();
        store(st, &id);
        respawn(st, &id);
        sync_all(st);
        mark_selecting(st, &id);
        bump(st, pf);
        Ok(st.objects[&id].view())
    })
    .map(Json)
}

async fn confirm(Path(id): Path<String>, headers: HeaderMap) -> R<SurveyObject> {
    with(|st| {
        if st.walls.contains_key(&id) {
            return Err(Error::Invalid("trigger walls are surveyed already".into()));
        }
        let author = author_of(&headers);
        let id = surveyed(st, &id, &author)?;
        if let Some(o) = st.objects.get_mut(&id) {
            o.updated = ids::now();
        }
        store(st, &id);
        // its name changes from [guess] to [surveyed]
        respawn(st, &id);
        sync_all(st);
        mark_selecting(st, &id);
        let pf = st.objects[&id].playfield;
        bump(st, pf);
        Ok(st.objects[&id].view())
    })
    .map(Json)
}

#[derive(Serialize)]
struct Removed {
    /// The record that keeps it deleted (a deleted guess's is a new placement): `forget` it to
    /// undo the delete.
    id: String,
}

async fn remove(Path(id): Path<String>, headers: HeaderMap) -> R<Removed> {
    with(|st| {
        if st.walls.contains_key(&id) {
            remove_wall(st, &id, &author_of(&headers));
            return Ok(Json(Removed { id }));
        }
        let o = st.objects.get(&id).ok_or_else(|| Error::NotFound(format!("no object {id}")))?;
        if !o.category.editable() {
            return Err(Error::Invalid(format!("{} objects can only be hidden", label(o.category))));
        }
        let author = author_of(&headers);
        let id = surveyed(st, &id, &author)?;
        let mut o = st.objects.remove(&id).expect("surveyed");
        // a removed placement: the guess it replaces stays away, and merges don't bring it back
        let mut p = o.placement();
        p.removed = true;
        p.updated = ids::now();
        if !author.is_empty() {
            p.author = author;
        }
        let list = &mut st.set.placements_mut(o.playfield).placements;
        match list.iter_mut().find(|x| x.id == p.id) {
            Some(x) => *x = p,
            None => list.push(p),
        }
        save(st);
        if let Some(n) = o.npc {
            st.by_npc.remove(&n);
        }
        despawn(&mut o);
        let viewers: Vec<Id32> = st.surveyors.iter().filter(|(_, s)| s.selected.as_deref() == Some(id.as_str())).map(|(v, _)| *v).collect();
        for v in viewers {
            if let Some(s) = st.surveyors.get_mut(&v) {
                s.selected = None;
            }
            mark(st, v);
        }
        bump(st, o.playfield);
        Ok(Json(Removed { id }))
    })
}

/// Nobody has `id` selected any more.
fn deselect(st: &mut State, id: &str) {
    let viewers: Vec<Id32> = st.surveyors.iter().filter(|(_, s)| s.selected.as_deref() == Some(id)).map(|(v, _)| *v).collect();
    for v in viewers {
        if let Some(s) = st.surveyors.get_mut(&v) {
            s.selected = None;
        }
        mark(st, v);
    }
}

/// The base spawn a placement's `origin` names (by id, else as `apply` finds a moved guess).
fn base_of(st: &State, pf: u32, origin: &Origin) -> Option<(String, SpawnDef)> {
    let defs = st.bases.get(&pf)?;
    let id_of = |d: &SpawnDef| ids::base_id(pf, skind(d.kind), nonzero(d.monster_data), nonzero(d.template), d.pos);
    defs.iter().find(|d| id_of(d) == origin.id).or_else(|| {
        defs.iter()
            .filter(|d| skind(d.kind) == origin.kind && nonzero(d.monster_data) == origin.monster_data && nonzero(d.template) == origin.template)
            .map(|d| (d, distance(d.pos, origin.pos)))
            .filter(|(_, dist)| *dist <= MATCH_DISTANCE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(d, _)| d)
    })
    .map(|d| (id_of(d), d.clone()))
}

/// Spawns the guess `origin` names again (its placement is gone): its view.
fn bring_back(st: &mut State, pf: u32, origin: &Origin) -> Option<SurveyObject> {
    let (id, def) = base_of(st, pf, origin)?;
    if st.objects.contains_key(&id) {
        return st.objects.get(&id).map(Object::view);
    }
    let mut o = Object {
        id: id.clone(),
        playfield: pf,
        category: Category::from_source(def.source.as_deref()),
        def,
        wave: None,
        replaces: None,
        comment: String::new(),
        author: String::new(),
        updated: 0,
        npc: None,
    };
    spawn(&st.surveyors, &st.set, &mut o);
    if let Some(n) = o.npc {
        st.by_npc.insert(n, id.clone());
    }
    let view = o.view();
    st.objects.insert(id, o);
    Some(view)
}

/// Drops a placement or trigger wall record entirely (no deletion record stays): undoing a
/// placement, or resetting an edited guess (the guess comes back, its view is the answer).
async fn forget(Path(id): Path<String>) -> R<Option<SurveyObject>> {
    with(|st| {
        if let Some(pf) = st.set.placements.iter().find(|(_, f)| f.triggers.iter().any(|t| t.id == id)).map(|(pf, _)| *pf) {
            st.walls.remove(&id); // despawns its pieces
            st.set.placements_mut(pf).triggers.retain(|t| t.id != id);
            save(st);
            deselect(st, &id);
            bump(st, pf);
            return Ok(Json(None));
        }
        let (pf, record) = st
            .set
            .placements
            .iter()
            .find_map(|(pf, f)| f.placements.iter().find(|p| p.id == id).map(|p| (*pf, p.clone())))
            .ok_or_else(|| Error::NotFound(format!("no placement {id}")))?;
        st.set.placements_mut(pf).placements.retain(|p| p.id != id);
        save(st);
        if let Some(mut o) = st.objects.remove(&id) {
            if let Some(n) = o.npc {
                st.by_npc.remove(&n);
            }
            despawn(&mut o);
        }
        deselect(st, &id);
        let guess = record.replaces.as_ref().and_then(|origin| bring_back(st, pf, origin));
        bump(st, pf);
        Ok(Json(guess))
    })
}

/// An object as a snapshot has it (the world editor's undo and redo).
#[derive(Deserialize)]
struct Snapshot {
    playfield: u32,
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    monster_data: Option<u32>,
    #[serde(default)]
    template: Option<u32>,
    #[serde(default)]
    level: Option<u32>,
    pos: [f32; 3],
    #[serde(default)]
    heading: f32,
    #[serde(default)]
    wave: Option<WaveRef>,
    #[serde(default)]
    patrol: Vec<[f32; 3]>,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    replaces: Option<Origin>,
    size: Option<[f32; 3]>,
    color: Option<String>,
    action: Option<WallAction>,
    #[serde(default)]
    once: Option<bool>,
}

/// Makes `id` exactly as the snapshot has it (spawned again; a guess it replaces goes away).
async fn put_object(Path(id): Path<String>, headers: HeaderMap, Json(b): Json<Snapshot>) -> R<SurveyObject> {
    with(|st| {
        let author = author_of(&headers);
        let pf = b.playfield;
        if b.kind == "trigger" {
            let def = TriggerWall {
                id: id.clone(),
                name: b.name,
                pos: b.pos,
                heading: b.heading,
                size: checked_size(b.size.unwrap_or(WALL_SIZE)),
                color: checked_color(b.color)?,
                action: b.action.unwrap_or(WallAction::Other),
                once: b.once.unwrap_or(false),
                comment: b.comment,
                removed: false,
                author,
                updated: ids::now(),
            };
            st.walls.remove(&id);
            store_wall(st, def.clone(), pf);
            let mut w = Wall { def, playfield: pf, pieces: Vec::new(), fired: HashSet::new() };
            spawn_wall(&st.surveyors, &mut w);
            let view = w.view();
            st.walls.insert(id.clone(), w);
            mark_selecting(st, &id);
            bump(st, pf);
            return Ok(view);
        }
        let kind: SKind = serde_json::from_value(serde_json::Value::String(b.kind.clone())).map_err(|_| Error::Invalid(format!("kind {}?", b.kind)))?;
        let p = Placement {
            id: id.clone(),
            kind,
            name: b.name,
            monster_data: b.monster_data,
            template: b.template,
            level: b.level,
            pos: b.pos,
            heading: b.heading,
            replaces: b.replaces,
            removed: false,
            wave: b.wave,
            patrol: b.patrol,
            comment: b.comment,
            author,
            updated: ids::now(),
        };
        // the guess it replaces goes away again
        let replaced = p.replaces.as_ref().and_then(|o| base_of(st, pf, o));
        if let Some((gid, _)) = &replaced
            && let Some(mut g) = st.objects.remove(gid)
        {
            if let Some(n) = g.npc {
                st.by_npc.remove(&n);
            }
            despawn(&mut g);
            deselect(st, gid);
        }
        let mut o = from_placement(&st.catalog, pf, &p, replaced.as_ref().map(|(_, d)| d)).ok_or_else(|| Error::Invalid("that kind can't be placed yet".into()))?;
        if let Some(mut old) = st.objects.remove(&id) {
            if let Some(n) = old.npc {
                st.by_npc.remove(&n);
            }
            despawn(&mut old);
        }
        o.author = p.author.clone();
        o.updated = p.updated;
        let list = &mut st.set.placements_mut(pf).placements;
        match list.iter_mut().find(|x| x.id == id) {
            Some(x) => *x = p,
            None => list.push(p),
        }
        save(st);
        spawn(&st.surveyors, &st.set, &mut o);
        if let Some(n) = o.npc {
            st.by_npc.insert(n, id.clone());
        }
        let view = o.view();
        st.objects.insert(id.clone(), o);
        sync_all(st);
        mark_selecting(st, &id);
        bump(st, pf);
        Ok(view)
    })
    .map(Json)
}

async fn set_info() -> R<SetInfo> {
    with(|st| Ok(Json(st.set.info.clone())))
}

async fn save_set_info(Json(info): Json<SetInfo>) -> R<SetInfo> {
    with(|st| {
        st.set.info = info.clone();
        save(st);
        Ok(Json(info))
    })
}

#[derive(Deserialize)]
struct ExportQuery {
    playfield: Option<u32>,
}

async fn export(Query(q): Query<ExportQuery>) -> R<Bundle> {
    with(|st| {
        let mut playfields = BTreeMap::new();
        let pfs: std::collections::BTreeSet<u32> = st.set.placements.keys().chain(st.set.notes.keys()).copied().filter(|pf| q.playfield.is_none_or(|x| x == *pf)).collect();
        for pf in pfs {
            let n = st.set.notes.get(&pf);
            playfields.insert(
                pf.to_string(),
                BundlePlayfield {
                    placements: st.set.placements.get(&pf).map(|f| f.placements.clone()).unwrap_or_default(),
                    notes: n.map(|n| n.notes.clone()).unwrap_or_default(),
                    scripts: n.map(|n| n.scripts.clone()).unwrap_or_default(),
                },
            );
        }
        Ok(Json(Bundle { format: sv::FORMAT, info: st.set.info.clone(), playfields }))
    })
}

#[derive(Deserialize)]
struct ImportBody {
    bundle: Bundle,
    options: MergeOptions,
    #[serde(default)]
    dry_run: bool,
}

async fn import(Json(b): Json<ImportBody>) -> R<merge::Report> {
    with(|st| {
        let mut theirs = Set { info: b.bundle.info, ..Default::default() };
        for (pf, content) in b.bundle.playfields {
            let pf: u32 = pf.parse().map_err(|_| Error::Invalid(format!("playfield {pf}?")))?;
            if !content.placements.is_empty() {
                theirs.placements_mut(pf).placements = content.placements;
            }
            if !content.notes.is_empty() || !content.scripts.is_empty() {
                let n = theirs.notes_mut(pf);
                n.notes = content.notes;
                n.scripts = content.scripts;
            }
        }
        let opts = merge::Options {
            tolerance: Tolerance { distance: b.options.distance.max(0.0), heading: b.options.heading.clamp(0.0, 180.0) },
            prefer: match b.options.prefer.as_deref() {
                Some("ours") => Prefer::Ours,
                Some("theirs") => Prefer::Theirs,
                _ => Prefer::Newer,
            },
            take: b.options.take.into_iter().collect(),
        };
        if b.dry_run {
            let mut copy = st.set.clone();
            return Ok(Json(merge::merge(&mut copy, &theirs, &opts)));
        }
        let report = merge::merge(&mut st.set, &theirs, &opts);
        save(st);
        // what changed is easiest seen by spawning the playfields again
        let pfs: Vec<u32> = theirs.placements.keys().chain(theirs.notes.keys()).copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        for pf in pfs {
            let ids: Vec<String> = st.objects.values().filter(|o| o.playfield == pf).map(|o| o.id.clone()).collect();
            for id in ids {
                if let Some(mut o) = st.objects.remove(&id) {
                    if let Some(n) = o.npc {
                        st.by_npc.remove(&n);
                    }
                    despawn(&mut o);
                }
            }
            st.walls.retain(|_, w| w.playfield != pf);
            build_playfield(st, pf);
            bump(st, pf);
        }
        sync_all(st);
        let viewers: Vec<Id32> = st.surveyors.keys().copied().collect();
        for v in viewers {
            mark(st, v);
        }
        info!(added = report.added.len(), near = report.near.len(), conflicts = report.conflicts.len(), "survey: import");
        Ok(Json(report))
    })
}

/// Shows `player` that wave and spawns its placements again, so they pop in as the trigger would.
fn preview(st: &mut State, player: Id32, pf: u32, wave: &WaveRef) {
    st.surveyors.entry(player).or_default().shown.insert(wave.script.clone(), wave.wave.clone());
    let ids: Vec<String> = st.objects.values().filter(|o| o.wave.as_ref() == Some(wave)).map(|o| o.id.clone()).collect();
    for id in &ids {
        respawn(st, id);
        mark_selecting(st, id);
    }
    sync_all(st);
    bump(st, pf);
}

// ------------------------------------------------------------------ trigger walls

/// Trigger wall looks: name, [resource type, visual], one piece's x range and height (RDB
/// bounds). The arena doors' closed (red) and open (green) visuals, and the shield and
/// forcefield walls of the Shadow Centurion Base and the Sorcerer's Keep.
const WALL_LOOKS: &[(&str, [u32; 2], [f32; 2], f32)] = &[
    ("red", [1010030, 155861], [-4.12, 4.12], 3.24),
    ("green", [1010030, 155858], [-4.12, 4.12], 3.24),
    ("shield", [1010030, 162353], [-2.5, 2.5], 8.0),
    ("low shield", [1010030, 351981], [-2.588, 2.588], 3.3),
    ("forcefield", [1010030, 389644], [-8.0, 0.0], 4.0),
];
/// Simple dynel instances of wall pieces (clear of RDB guids and gates.rs's beams).
static NEXT_PIECE: AtomicU32 = AtomicU32::new(0x00e0_0000);
/// How often players' moves are checked against the walls.
const WALL_TICK: Duration = Duration::from_millis(200);
/// A move longer than this between two checks is a teleport, not a walk through a wall.
const WALL_JUMP: f32 = 15.0;
/// A new wall: width, height, depth.
const WALL_SIZE: [f32; 3] = [6.0, 3.0, 1.0];

struct Wall {
    def: TriggerWall,
    playfield: u32,
    /// The pieces drawing it (dropping them despawns them).
    pieces: Vec<(Id32, world::Presence)>,
    /// Players a once-only wall went off for.
    fired: HashSet<Id32>,
}

impl Wall {
    fn ids(&self) -> impl Iterator<Item = Id32> + '_ {
        self.pieces.iter().map(|(id, _)| *id)
    }

    fn view(&self) -> SurveyObject {
        let d = &self.def;
        SurveyObject {
            id: d.id.clone(),
            kind: "trigger".into(),
            name: if d.name.is_empty() { "Trigger wall".into() } else { d.name.clone() },
            monster_data: None,
            template: None,
            level: None,
            pos: d.pos,
            heading: d.heading,
            category: Category::Surveyed,
            wave: None,
            patrol: Vec::new(),
            comment: d.comment.clone(),
            author: d.author.clone(),
            updated: d.updated,
            playfield: self.playfield,
            replaces: None,
            size: Some(d.size),
            color: Some(d.color.clone()),
            action: Some(d.action.clone()),
            once: Some(d.once),
        }
    }
}

fn look_of(color: &str) -> &'static (&'static str, [u32; 2], [f32; 2], f32) {
    WALL_LOOKS.iter().find(|l| l.0 == color).unwrap_or(&WALL_LOOKS[0])
}

/// Spawns the wall's pieces: its look scaled to its height, side by side across its width.
/// Surveyors who hide it don't get them.
fn spawn_wall(surveyors: &HashMap<Id32, Surveyor>, w: &mut Wall) {
    let (_, visual, [x0, x1], height) = *look_of(&w.def.color);
    let [width, h, _] = w.def.size;
    let scale = (h / height).max(0.05);
    let piece = (x1 - x0) * scale;
    let n = ((width / piece).ceil() as usize).clamp(1, 64);
    let step = if n > 1 { (width - piece) / (n - 1) as f32 } else { 0.0 };
    // the look's own middle (the forcefield's runs from -8 to 0)
    let offset = (x0 + x1) / 2.0 * scale;
    let (sin, cos) = w.def.heading.sin_cos();
    let half = w.def.heading / 2.0;
    let rot = [0.0, half.sin(), 0.0, half.cos()];
    let name = format!("[trigger] {}", if w.def.name.is_empty() { "wall" } else { &w.def.name });
    let hiding: Vec<Id32> = surveyors.iter().filter(|(_, s)| s.hidden.contains(&w.def.id)).map(|(v, _)| *v).collect();
    for i in 0..n {
        let across = if n > 1 { -width / 2.0 + piece / 2.0 + step * i as f32 } else { 0.0 } - offset;
        // across = the wall's own x axis, (cos h, -sin h) in x/z
        let pos = [w.def.pos[0] + across * cos, w.def.pos[1], w.def.pos[2] - across * sin];
        let id = Id32::new(crate::scenery::SIMPLE_DYNEL_TYPE, NEXT_PIECE.fetch_add(1, Ordering::Relaxed));
        {
            let mut hidden = HIDDEN.lock().unwrap();
            for v in &hiding {
                hidden.entry(*v).or_default().insert(id);
            }
        }
        let (outbox, mut inbox) = mpsc::unbounded_channel();
        tokio::spawn(async move { while inbox.recv().await.is_some() {} });
        let pf = w.playfield;
        let look = crate::scenery::Look { name: name.clone(), visual, bounds: [x0, 0.0, -0.1, x1, height, 0.1], scale };
        let presence = world::join(id, pf, Arc::new(Mutex::new(pos)), move || crate::scenery::visual_records(id, pf, pos, rot, &look), outbox);
        w.pieces.push((id, presence));
    }
}

/// Stores the wall in the set (replacing the one with its id).
fn store_wall(st: &mut State, def: TriggerWall, pf: u32) {
    let list = &mut st.set.placements_mut(pf).triggers;
    match list.iter_mut().find(|x| x.id == def.id) {
        Some(x) => *x = def,
        None => list.push(def),
    }
    save(st);
}

#[derive(Serialize)]
struct WallLook {
    color: &'static str,
}

/// The looks trigger walls can have (their `color`).
async fn wall_looks() -> R<Vec<WallLook>> {
    with(|_| Ok(Json(WALL_LOOKS.iter().map(|l| WallLook { color: l.0 }).collect())))
}

#[derive(Deserialize, Default)]
struct PlaceTriggerBody {
    #[serde(default)]
    name: String,
    size: Option<[f32; 3]>,
    color: Option<String>,
    action: Option<WallAction>,
    #[serde(default)]
    once: bool,
}

fn checked_size(size: [f32; 3]) -> [f32; 3] {
    size.map(|v| if v.is_finite() { v.clamp(0.1, 200.0) } else { 1.0 })
}

fn checked_color(color: Option<String>) -> Result<String, Error> {
    match color {
        None => Ok(WALL_LOOKS[0].0.into()),
        Some(c) if WALL_LOOKS.iter().any(|l| l.0 == c) => Ok(c),
        Some(c) => Err(Error::Invalid(format!("no wall look {c} (wall-looks lists them)"))),
    }
}

/// A trigger wall where the surveyor stands, across their way.
async fn place_trigger(Path(account): Path<u32>, headers: HeaderMap, Json(b): Json<PlaceTriggerBody>) -> R<SurveyObject> {
    with(|st| {
        let (player, pf) = player_of(account)?;
        let author = Some(author_of(&headers)).filter(|a| !a.is_empty()).unwrap_or_else(|| crate::sessions::players().into_iter().find(|p| p.account_id == account).map(|p| p.character_name).unwrap_or_default());
        let def = TriggerWall {
            id: ids::new_id("t", &author),
            name: b.name,
            pos: world::live_position(player).unwrap_or_default(),
            heading: heading_of(world::facing_of(player).unwrap_or([0.0, 1.0])),
            size: checked_size(b.size.unwrap_or(WALL_SIZE)),
            color: checked_color(b.color)?,
            action: b.action.unwrap_or(WallAction::Other),
            once: b.once,
            comment: String::new(),
            removed: false,
            author,
            updated: ids::now(),
        };
        store_wall(st, def.clone(), pf);
        let mut w = Wall { def, playfield: pf, pieces: Vec::new(), fired: HashSet::new() };
        spawn_wall(&st.surveyors, &mut w);
        let view = w.view();
        let id = w.def.id.clone();
        st.walls.insert(id.clone(), w);
        st.surveyors.entry(player).or_default().selected = Some(id);
        mark(st, player);
        bump(st, pf);
        Ok(view)
    })
    .map(Json)
}

fn edit_wall(st: &mut State, id: &str, c: Changes, author: String) -> Result<SurveyObject, Error> {
    let color = match c.color {
        Some(col) => Some(checked_color(Some(col))?),
        None => None,
    };
    let w = st.walls.get_mut(id).expect("checked");
    let d = &mut w.def;
    if let Some(p) = c.pos {
        d.pos = p;
    }
    if let Some(h) = c.heading {
        d.heading = h;
    }
    if let Some(size) = c.size {
        d.size = checked_size(size);
    }
    if let Some(col) = color {
        d.color = col;
    }
    if let Some(a) = c.action {
        d.action = a;
    }
    if let Some(o) = c.once {
        d.once = o;
        w.fired.clear();
    }
    if let Some(n) = c.name {
        d.name = n;
    }
    if let Some(t) = c.comment {
        d.comment = t;
    }
    if !author.is_empty() {
        d.author = author;
    }
    d.updated = ids::now();
    let (def, pf) = (d.clone(), w.playfield);
    // drawn again where and as it is now
    w.pieces.clear();
    spawn_wall(&st.surveyors, w);
    store_wall(st, def, pf);
    mark_selecting(st, id);
    bump(st, pf);
    Ok(st.walls[id].view())
}

fn remove_wall(st: &mut State, id: &str, author: &str) {
    let Some(w) = st.walls.remove(id) else { return };
    let mut def = w.def.clone();
    def.removed = true;
    def.updated = ids::now();
    if !author.is_empty() {
        def.author = author.into();
    }
    store_wall(st, def, w.playfield);
    drop(w); // despawns its pieces
    let viewers: Vec<Id32> = st.surveyors.iter().filter(|(_, s)| s.selected.as_deref() == Some(id)).map(|(v, _)| *v).collect();
    for v in viewers {
        if let Some(s) = st.surveyors.get_mut(&v) {
            s.selected = None;
        }
        mark(st, v);
    }
    let pf = st.set.placements.iter().find(|(_, f)| f.triggers.iter().any(|t| t.id == id)).map(|(pf, _)| *pf);
    if let Some(pf) = pf {
        bump(st, pf);
    }
}

/// Checks every player's moves against the trigger walls of their playfield.
async fn watch_walls() {
    let mut tick = tokio::time::interval(WALL_TICK);
    loop {
        tick.tick().await;
        // where the players are (outside the survey lock)
        let players: Vec<(Id32, u32, [f32; 3])> = crate::sessions::players()
            .into_iter()
            .filter_map(|p| {
                let id = Id32::new(crate::territory::CHARACTER_TYPE, p.character_id);
                let (pf, _) = world::whereabouts(id)?;
                Some((id, pf, world::live_position(id)?))
            })
            .collect();
        let mut guard = STATE.lock().unwrap();
        let Some(st) = guard.as_mut() else { continue };
        let mut fired = Vec::new();
        for (player, pf, pos) in players {
            let Some((last_pf, last)) = st.last_pos.insert(player, (pf, pos)) else { continue };
            if last_pf != pf || distance(last, pos) > WALL_JUMP || last == pos {
                continue;
            }
            for w in st.walls.values().filter(|w| w.playfield == pf) {
                if w.def.crossed(last, pos) && !(w.def.once && w.fired.contains(&player)) {
                    fired.push((w.def.id.clone(), player, pf));
                }
            }
        }
        for (id, player, pf) in fired {
            fire(st, &id, player, pf);
        }
    }
}

/// What crossing wall `id` does for `player` (in survey mode: shows it).
fn fire(st: &mut State, id: &str, player: Id32, pf: u32) {
    let Some(w) = st.walls.get_mut(id) else { return };
    w.fired.insert(player);
    let def = w.def.clone();
    info!(wall = %def.id, name = %def.name, player = player.instance, action = ?def.action, "survey: trigger wall crossed");
    let label = if def.name.is_empty() { "trigger".to_string() } else { format!("trigger {}", def.name) };
    let say = |text: String| crate::chat::system(player.instance, &format!("[{label}] {text}"));
    match &def.action {
        WallAction::Script { script, wave } => {
            let s = script.as_ref().and_then(|sid| scripts_of(&st.set, pf).iter().find(|s| &s.id == sid)).cloned();
            match s {
                Some(s) => {
                    let w = wave.as_ref().and_then(|wid| s.waves.iter().find(|x| &x.id == wid)).or(s.waves.first()).cloned();
                    if let Some(w) = w {
                        say(format!("starts {}: {}", if s.name.is_empty() { "a script" } else { &s.name }, w.name));
                        preview(st, player, pf, &WaveRef { script: s.id.clone(), wave: w.id.clone() });
                    }
                }
                None => say("starts a script (none chosen yet)".into()),
            }
        }
        WallAction::Discover { name, achievement } => {
            let ach = achievement.as_deref().and_then(|a| targets().achievements.iter().find(|x| x.id.to_string() == a || x.name == a));
            let toward = ach.map_or(String::new(), |a| {
                if a.locations.len() > 1 { format!(" (toward {}: one of {} locations)", a.name, a.locations.len()) } else { format!(" ({})", a.name) }
            });
            say(format!("discovered {}{toward}", name.as_deref().unwrap_or("a location")))
        }
        WallAction::Dialog { dialog } => {
            // by its first text (`knubot::starting_with`): !dialog's number for it
            let n = dialog.as_deref().and_then(|d| d.parse::<u32>().ok()).and_then(|first| crate::knubot::dialogues().iter().position(|x| x.first_text() == Some(first)));
            match (n, dialog.as_deref()) {
                (Some(i), _) => {
                    let d = &crate::knubot::dialogues()[i];
                    say(format!("opens {}'s dialogue", d.npc));
                    world::send_to(player, world::Out::Command(format!("!dialog {}", i + 1), None));
                }
                (None, Some(d)) => say(format!("opens the dialogue \"{d}\"")),
                (None, None) => say("opens a dialogue (none chosen yet)".into()),
            }
        }
        WallAction::Portal { window, to } => {
            let name = |pf: u32| crate::teleport::destination(pf).map_or_else(|| pf.to_string(), |d| d.name.clone());
            // a window that leads there, if only the destination is set
            let window = window.or_else(|| to.and_then(|t| crate::teleport::windows().into_iter().find(|w| w.1 == t).map(|w| w.0)));
            match (window, *to) {
                (Some(w), _) => {
                    let dest = to.or_else(|| crate::teleport::windows().into_iter().find(|x| x.0 == w).map(|x| x.1));
                    say(format!("opens a portal{}", dest.map_or(String::new(), |d| format!(" to {}", name(d)))));
                    world::send_to(player, world::Out::Command(format!("!window {w}"), None));
                }
                (None, Some(t)) => {
                    say(format!("a portal to {}", name(t)));
                    world::send_to(player, world::Out::Command(format!("!tp {t}"), None));
                }
                (None, None) => say("opens a portal (none chosen yet)".into()),
            }
        }
        WallAction::Message { text } => say(if text.is_empty() { "(a message)".into() } else { text.clone() }),
        WallAction::Other => say(if def.comment.is_empty() { "crossed".into() } else { def.comment.clone() }),
    }
}

// ------------------------------------------------------------------ trigger wall targets

#[derive(Serialize, Clone)]
struct TargetLocation {
    name: String,
    playfield: u32,
    playfield_name: Option<String>,
    /// where it is (an exploration area's middle), if known
    pos: Option<[f32; 3]>,
    /// achievement ids it counts toward
    achievements: Vec<u32>,
}

#[derive(Serialize, Clone)]
struct TargetAchievement {
    id: u32,
    name: String,
    /// the achievement's group ("Pirate Exploration")
    category: String,
    /// the locations it needs (all of them)
    locations: Vec<TargetLocationRef>,
}

#[derive(Serialize, Clone)]
struct TargetLocationRef {
    name: String,
    playfield: u32,
}

#[derive(Serialize, Clone)]
struct TargetDialog {
    /// its first text's id (what a wall's `dialog` stores)
    id: String,
    npc: String,
    playfield: u32,
    playfield_name: Option<String>,
    line: String,
}

#[derive(Serialize, Clone)]
struct TargetPortal {
    /// the teleport window it opens
    window: u32,
    /// where it leads
    to: u32,
    to_name: Option<String>,
    /// asks Yes/No (its window would be empty) instead of showing the window
    plain: bool,
    /// the playfields with a portal opening it
    playfields: Vec<u32>,
}

#[derive(Serialize)]
struct Targets {
    locations: Vec<TargetLocation>,
    achievements: Vec<TargetAchievement>,
    dialogs: Vec<TargetDialog>,
    portals: Vec<TargetPortal>,
}

/// What trigger walls can discover or open: the exploration areas and `find` achievements
/// (data/achievements.json; an `all` achievement whose parts are finds needs each of their
/// locations), the recorded dialogues and the portals' teleport windows.
fn targets() -> &'static Targets {
    static T: LazyLock<Targets> = LazyLock::new(|| {
        #[derive(Deserialize)]
        struct Ach {
            id: u32,
            name: String,
            kind: String,
            #[serde(default)]
            playfield: Option<u32>,
            #[serde(default)]
            find: Option<String>,
            #[serde(default)]
            children: Vec<u32>,
        }
        #[derive(Deserialize)]
        struct File {
            achievements: Vec<Ach>,
        }
        let file: File = serde_json::from_str(include_str!("../../../data/achievements.json")).expect("achievements.json");
        let pf_name = |pf: u32| crate::teleport::destination(pf).map(|d| d.name.clone());
        // "Pirate_Exploration_The Sirens" -> ("The Sirens", "Pirate Exploration")
        let split = |n: &str| match n.rsplit_once('_') {
            Some((cat, name)) => (name.to_string(), cat.replace('_', " ")),
            None => (n.to_string(), String::new()),
        };
        let finds: HashMap<u32, TargetLocationRef> = file
            .achievements
            .iter()
            .filter_map(|a| Some((a.id, TargetLocationRef { name: a.find.clone()?, playfield: a.playfield? })))
            .collect();
        let mut achievements = Vec::new();
        for a in &file.achievements {
            let locations: Vec<TargetLocationRef> = match a.kind.as_str() {
                "find" => finds.get(&a.id).cloned().into_iter().collect(),
                "all" if !a.children.is_empty() && a.children.iter().all(|c| finds.contains_key(c)) => a.children.iter().map(|c| finds[c].clone()).collect(),
                _ => continue,
            };
            if locations.is_empty() {
                continue;
            }
            let (name, category) = split(&a.name);
            achievements.push(TargetAchievement { id: a.id, name, category, locations });
        }
        // the exploration areas, and the found places that aren't areas
        let mut locations: Vec<TargetLocation> = crate::encounter::explore_areas()
            .into_iter()
            .map(|(pf, name, pos)| TargetLocation { name: name.into(), playfield: pf, playfield_name: pf_name(pf), pos: Some(pos), achievements: Vec::new() })
            .collect();
        for f in finds.values() {
            if !locations.iter().any(|l| l.name == f.name && l.playfield == f.playfield) {
                locations.push(TargetLocation { name: f.name.clone(), playfield: f.playfield, playfield_name: pf_name(f.playfield), pos: None, achievements: Vec::new() });
            }
        }
        for l in &mut locations {
            l.achievements = achievements.iter().filter(|a| a.locations.iter().any(|r| r.name == l.name && r.playfield == l.playfield)).map(|a| a.id).collect();
        }
        locations.sort_by(|a, b| (a.playfield, &a.name).cmp(&(b.playfield, &b.name)));
        let dialogs = crate::knubot::dialogues()
            .iter()
            .filter_map(|d| {
                Some(TargetDialog { id: d.first_text()?.to_string(), npc: d.npc.clone(), playfield: d.playfield, playfield_name: pf_name(d.playfield), line: d.first_line() })
            })
            .collect();
        let portals = crate::teleport::windows()
            .into_iter()
            .map(|(window, to, playfields)| TargetPortal { window, to, to_name: pf_name(to), plain: crate::teleport::plain_window(window), playfields })
            .collect();
        Targets { locations, achievements, dialogs, portals }
    });
    &T
}

async fn trigger_targets() -> R<&'static Targets> {
    with(|_| Ok(Json(targets())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_targets() {
        let t = targets();
        let town = t.locations.iter().find(|l| l.playfield == 4200 && l.name == "Achievement_PirateTown").expect("Hidden Pirate Town");
        assert!(town.achievements.contains(&724), "{:?}", town.achievements);
        let a = t.achievements.iter().find(|a| a.id == 724).unwrap();
        assert_eq!((a.name.as_str(), a.category.as_str()), ("Find the Hidden Pirate Town", "Pirate Exploration"));
        // some need several locations
        assert!(t.achievements.iter().any(|a| a.locations.len() > 1));
        let d = t.dialogs.iter().find(|d| d.npc == "Video Game Guy").unwrap();
        assert!(!d.line.is_empty() && !d.line.contains('{') && !d.line.contains('['), "{}", d.line);
        // Pirate World's portal asks Yes/No
        assert!(t.portals.iter().any(|p| p.window == 74 && p.to == 4600 && p.plain), "{:?}", t.portals.iter().map(|p| (p.window, p.to)).collect::<Vec<_>>());
        assert!(t.portals.iter().any(|p| !p.plain && !p.playfields.is_empty()));
    }
}
