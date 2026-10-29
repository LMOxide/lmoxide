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
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use lmoxide_net::types::Id32;
use lmoxide_survey::{
    self as sv, Kind as SKind, Note, Origin, Placement, Script, SmashRule, TriggerWall, WallAction, WaveRef,
    Category,
    ids,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    npc::{self, Kind, SpawnDef},
    world,
};
use lmoxide_content::git;
use lmoxide_content::id::RecordKind;
use lmoxide_content::work::{Key, Work};

mod bouncers;
mod migrate;
mod records;

pub(crate) use records::wave_spawns;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether this is a survey server.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

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

#[derive(Clone)]
struct Object {
    /// Its record's id (`spawn/4200/scallywag-7`): what the API names it by.
    id: String,
    /// Its record (id and feature).
    key: Key,
    /// Its template (`npc/...`, `object/...`).
    template: Option<String>,
    playfield: u32,
    category: Category,
    /// As spawned, but with its plain name; `rot` is the heading.
    def: SpawnDef,
    wave: Option<WaveRef>,
    replaces: Option<Origin>,
    comment: String,
    /// lmoxide_survey::features (base objects: live)
    feature: String,
    tags: Vec<String>,
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
    /// A selected bouncer's landing point marker: its helper and the effect instance.
    landing: Option<(Id32, u32)>,
    /// Their marks on groups of objects ([`show_marks`]), the running mark effects (wearer,
    /// package, instance, a helper?) and the playfield and revision they show.
    marks: Vec<Mark>,
    mark_effects: Vec<(Id32, u32, u32, bool)>,
    marks_at: Option<(u32, u64)>,
}

struct State {
    dir: PathBuf,
    /// The base content of each playfield (what `npc::start` spawns elsewhere).
    bases: BTreeMap<u32, Vec<SpawnDef>>,
    objects: HashMap<String, Object>,
    by_npc: HashMap<Id32, String>,
    revision: HashMap<u32, u64>,
    surveyors: HashMap<Id32, Surveyor>,
    catalog: Vec<(Placeable, SpawnDef)>,
    /// Trigger walls by id.
    walls: HashMap<String, Wall>,
    /// Each world's notes and scripts, as the working copy has them (lmoxide_content::views).
    notes: Notes,
    /// The features records can belong to (the working copy's features.json).
    features: sv::features::FeaturesFile,
    /// Where each player was at the last wall check (playfield, position).
    last_pos: HashMap<Id32, (u32, [f32; 3])>,
}

/// Notes and scripts by playfield.
type Notes = BTreeMap<u32, sv::NotesFile>;

static STATE: LazyLock<Mutex<Option<State>>> = LazyLock::new(|| Mutex::new(None));
/// viewer -> the NPCs not sent to them.
static HIDDEN: LazyLock<Mutex<HashMap<Id32, HashSet<Id32>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Makes this a survey server. `dir`: the survey set of the editor before content records,
/// taken into the working copy once at start ([`migrate`]).
pub fn init(dir: PathBuf) {
    info!(set = %dir.display(), "survey mode");
    ENABLED.store(true, Ordering::Relaxed);
    *STATE.lock().unwrap() = Some(State {
        dir,
        bases: BTreeMap::new(),
        objects: HashMap::new(),
        by_npc: HashMap::new(),
        revision: HashMap::new(),
        surveyors: HashMap::new(),
        catalog: Vec::new(),
        walls: HashMap::new(),
        notes: Notes::new(),
        features: Default::default(),
        last_pos: HashMap::new(),
    });
}

/// Content records the world editor saved, created or deleted (crate::editor): the worlds whose
/// spawns changed spawn again; a changed template, skill or loot table, every world.
pub fn content_changed(work: &Work, keys: &[Key]) {
    // (with play content in the same server, the published content runs the quests)
    if keys.iter().any(|k| matches!(k.id.kind, RecordKind::Quest | RecordKind::Area)) && !crate::content::enabled() {
        crate::quests::set_from_work(work);
    }
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    let everywhere = keys.iter().any(|k| matches!(k.id.kind, RecordKind::Npc | RecordKind::Object | RecordKind::Skill));
    let pfs: Vec<u32> = if everywhere {
        st.objects.values().map(|o| o.playfield).chain(keys.iter().filter_map(|k| k.id.world)).collect()
    } else if keys.iter().any(|k| k.id.kind == RecordKind::Script) {
        // (its placements show or hide by wave)
        keys.iter().filter_map(|k| k.id.world).collect()
    } else {
        keys.iter().filter(|k| matches!(k.id.kind, RecordKind::Spawn | RecordKind::Area | RecordKind::Trigger | RecordKind::Bouncer)).filter_map(|k| k.id.world).collect()
    };
    if !pfs.is_empty() {
        rebuild(st, work, pfs);
    }
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
        SKind::Bouncer => Err(Error::Invalid("bouncers are jump pads, not characters".into())),
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
            feature: self.feature.clone(),
            tags: self.tags.clone(),
            author: self.author.clone(),
            updated: self.updated,
            playfield: self.playfield,
            replaces: self.replaces.clone(),
            size: None,
            radius: None,
            color: None,
            action: None,
            once: None,
            target: None,
            chain: None,
        }
    }
}

// ------------------------------------------------------------------ startup

/// Spawns the working copy's spawns of every world (and the set's walls and bouncers);
/// `all`, `npc::start`'s list, gives the catalog of looks.
pub fn start(all: Vec<(u32, SpawnDef)>) {
    let built = crate::editor::work(|work| {
        let mut guard = STATE.lock().unwrap();
        let Some(st) = guard.as_mut() else { return };
        for (pf, def) in all {
            st.bases.entry(pf).or_default().push(def);
        }
        st.catalog = catalog_of(&st.bases);
        migrate::run(&st.dir, work, &st.catalog, &[]);
        st.features = work.features();
        let playfields: std::collections::BTreeSet<u32> =
            work.records().filter(|(k, _)| matches!(k.id.kind, RecordKind::Spawn | RecordKind::Script | RecordKind::Area | RecordKind::Trigger | RecordKind::Bouncer)).filter_map(|(k, _)| k.id.world).collect();
        for pf in playfields {
            build_playfield(st, work, pf);
        }
        info!(objects = st.objects.len(), walls = st.walls.len(), "survey: content spawned");
    });
    if built.is_none() {
        warn!("survey: no content working copy (--content-dir): nothing spawned");
    }
    tokio::spawn(watch_walls());
}

/// Spawns playfield `pf`'s objects from the working copy, and the set's trigger walls.
fn build_playfield(st: &mut State, work: &Work, pf: u32) {
    let notes = lmoxide_content::views::notes_of(work, pf);
    if notes.notes.is_empty() && notes.scripts.is_empty() {
        st.notes.remove(&pf);
    } else {
        st.notes.insert(pf, notes);
    }
    for mut o in records::objects_of(work, pf) {
        spawn(&st.surveyors, &st.notes, &mut o);
        if let Some(n) = o.npc {
            st.by_npc.insert(n, o.id.clone());
        }
        st.objects.insert(o.id.clone(), o);
    }
    bouncers::apply(work, pf);
    for def in lmoxide_content::views::walls_of(work, pf) {
        let mut w = Wall { def, playfield: pf, pieces: Vec::new(), fired: HashSet::new() };
        spawn_wall(&st.surveyors, &mut w);
        st.walls.insert(w.def.id.clone(), w);
    }
}

/// Spawns `id` again as its record `key` now is (gone when the record is): its view.
fn replace_object(st: &mut State, work: &Work, id: &str, key: &Key) -> Option<SurveyObject> {
    if let Some(mut old) = st.objects.remove(id) {
        if let Some(n) = old.npc {
            st.by_npc.remove(&n);
        }
        despawn(&mut old);
    }
    let mut o = records::object_of(work, key)?;
    spawn(&st.surveyors, &st.notes, &mut o);
    if let Some(n) = o.npc {
        st.by_npc.insert(n, o.id.clone());
    }
    let view = o.view();
    st.objects.insert(o.id.clone(), o);
    Some(view)
}

/// The object `id` if editors may change it (guesses and surveyed ones).
fn editable(st: &State, id: &str) -> Result<Object, Error> {
    let o = st.objects.get(id).ok_or_else(|| Error::NotFound(format!("no object {id}")))?;
    if !o.category.editable() {
        return Err(Error::Invalid(format!("{} objects can only be hidden", label(o.category))));
    }
    Ok(o.clone())
}

/// The catalog's spawn of that look (its spells, NPC template, health, ...), if any.
fn template_of(catalog: &[(Placeable, SpawnDef)], kind: SKind, monster_data: Option<u32>, template: Option<u32>) -> Option<SpawnDef> {
    catalog
        .iter()
        .find(|(p, _)| p.kind == kind && p.monster_data == monster_data && p.template == template)
        .or_else(|| catalog.iter().find(|(p, _)| p.kind == kind && p.monster_data == monster_data))
        .map(|(_, d)| d.clone())
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
fn spawn(surveyors: &HashMap<Id32, Surveyor>, notes: &Notes, o: &mut Object) {
    let def = o.spawn_def();
    let id = npc::new_id(&def);
    {
        let mut hidden = HIDDEN.lock().unwrap();
        for (viewer, s) in surveyors {
            if is_hidden_for(s, o, notes) {
                hidden.entry(*viewer).or_default().insert(id);
            }
        }
    }
    o.npc = Some(id);
    npc::spawn_as(id, def, crate::instance::Mode::WorldEditor.main(o.playfield));
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
    spawn(&st.surveyors, &st.notes, o);
    if let Some(new) = o.npc {
        st.by_npc.insert(new, id.to_string());
    }
}

fn bump(st: &mut State, pf: u32) {
    *st.revision.entry(pf).or_default() += 1;
}

fn scripts_of(notes: &Notes, pf: u32) -> &[Script] {
    notes.get(&pf).map_or(&[], |n| n.scripts.as_slice())
}

/// Whether playfield `pf` has that script wave.
fn has_wave(notes: &Notes, pf: u32, w: &WaveRef) -> bool {
    scripts_of(notes, pf).iter().any(|s| s.id == w.script && s.waves.iter().any(|x| x.id == w.wave))
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

fn is_hidden_for(s: &Surveyor, o: &Object, notes: &Notes) -> bool {
    if s.hidden.contains(&o.id) {
        return true;
    }
    let Some(w) = &o.wave else { return false };
    let Some(script) = scripts_of(notes, o.playfield).iter().find(|x| x.id == w.script) else { return false };
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
            all.insert(n, (o.playfield, is_hidden_for(s, o, &st.notes)));
        }
    }
    for w in st.walls.values() {
        for id in w.ids() {
            all.insert(id, (w.playfield, s.hidden.contains(&w.def.id)));
        }
    }
    for b in crate::buildie::list(None) {
        all.insert(b.id, (b.playfield.model(), s.hidden.contains(&buildie_id(&b))));
    }
    for l in crate::jumppad::list(None) {
        all.insert(l.dynel, (l.playfield, s.hidden.contains(&l.id)));
    }
    let want: HashSet<Id32> = all.iter().filter(|(_, (_, h))| *h).map(|(id, _)| *id).collect();
    let had = HIDDEN.lock().unwrap().insert(viewer, want.clone()).unwrap_or_default();
    let here = world::whereabouts(viewer).map(|(pf, _)| pf.model());
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
    }).or_else(|| buildie(selected.as_ref()?).map(|b| (b.id, Kind::Smashable, b.pos, b.playfield.model())))
    // (a bouncer: as a smashable, and its landing point marked the same way)
    .or_else(|| crate::jumppad::find(selected.as_ref()?).map(|l| (l.dynel, Kind::Smashable, l.pos, l.playfield)));
    let landing = selected.as_deref().and_then(crate::jumppad::find).and_then(|l| Some((l.playfield, l.target?)));
    let Some(s) = st.surveyors.get_mut(&viewer) else { return };
    let mut records = Vec::new();
    if let Some((wearer, instance, helper)) = s.marker.take() {
        records.push(crate::effects::stop_effect_package(wearer, SELECT_EFFECT, 0, instance));
        if helper {
            records.push(crate::game::despawn_record(wearer));
        }
    }
    if let Some((helper, instance)) = s.landing.take() {
        records.push(crate::effects::stop_effect_package(helper, SELECT_EFFECT, 0, instance));
        records.push(crate::game::despawn_record(helper));
    }
    if let Some((playfield, at)) = landing {
        let instance = crate::effects::new_instance();
        let (helper, spawn) = helper_at(playfield, at);
        records.extend(spawn);
        records.push(crate::effects::effect_package_hook(helper, SELECT_EFFECT, "", "", 0.0, 0, instance));
        s.landing = Some((helper, instance));
    }
    if let Some((npc, kind, pos, playfield)) = target {
        let instance = crate::effects::new_instance();
        let wearer = if kind == Kind::Smashable {
            let (helper, spawn) = helper_at(playfield, pos);
            records.extend(spawn);
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

/// An invisible character at `pos` (sent only to one viewer) to wear an effect for a simple
/// dynel (smashable, wall piece, buildie), which shows none itself: its id and spawn records.
fn helper_at(playfield: u32, pos: [f32; 3]) -> (Id32, Vec<bytes::Bytes>) {
    let helper = Id32::new(crate::territory::CHARACTER_TYPE, NEXT_HELPER.fetch_add(1, Ordering::Relaxed));
    let records = crate::game::remote_spawn_records(
        helper,
        &crate::game::CharSpec {
            name: "",
            playfield: crate::instance::Mode::WorldEditor.main(playfield),
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
    );
    (helper, records)
}

// ------------------------------------------------------------------ marks

/// Looks an editor can mark objects with (looping effect packages; the selection has the
/// champions' feet aura): the achievement auras and the champion aura icons.
const MARK_LOOKS: &[(&str, u32)] = &[
    ("achievement aura 1", 529173),
    ("achievement aura 2", 529182),
    ("achievement aura 3", 529183),
    ("achievement aura 4", 529186),
    ("nature icon", 586093),
    ("magic icon", 586094),
    ("tech icon", 586095),
    ("light icon", 586096),
    ("shadow icon", 586097),
];

/// What a mark picks: the objects one author changed last, or those of a feature (and what is
/// nested under it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkBy {
    Author,
    Feature,
    Tag,
}

/// An editor's own marker on a group of objects (never saved, like hiding).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub by: MarkBy,
    pub value: String,
    /// one of MARK_LOOKS
    pub look: String,
}

impl Mark {
    fn matches(&self, author: &str, feature: &str, tags: &[String]) -> bool {
        match self.by {
            MarkBy::Author => author.eq_ignore_ascii_case(&self.value),
            MarkBy::Feature => sv::features::within(feature, &self.value),
            MarkBy::Tag => tags.iter().any(|t| t.eq_ignore_ascii_case(self.value.trim_start_matches('#'))),
        }
    }
}

/// Shows `viewer` their marks on what they see in their playfield (again: after changes there
/// or a move to another playfield, see `watch_walls`).
fn show_marks(st: &mut State, viewer: Id32) {
    let Some(pf) = world::whereabouts(viewer).map(|(pf, _)| pf.model()) else { return };
    let revision = st.revision.get(&pf).copied().unwrap_or(0);
    let Some(s) = st.surveyors.get(&viewer) else { return };
    let mut records = Vec::new();
    let mut running = Vec::new();
    // (what to mark: the wearer if it is a character, else where a helper stands)
    let mut targets: Vec<(Option<Id32>, [f32; 3], u32)> = Vec::new();
    let look = |m: &Mark| MARK_LOOKS.iter().find(|l| l.0 == m.look).map(|l| l.1);
    if !s.marks.is_empty() {
        for o in st.objects.values().filter(|o| o.playfield == pf && !is_hidden_for(s, o, &st.notes)) {
            let Some(npc) = o.npc else { continue };
            if let Some(pkg) = s.marks.iter().find(|m| m.matches(&o.author, &o.feature, &o.tags)).and_then(look) {
                targets.push(((o.def.kind != Kind::Smashable).then_some(npc), o.def.pos, pkg));
            }
        }
        for w in st.walls.values().filter(|w| w.playfield == pf && !s.hidden.contains(&w.def.id)) {
            if let Some(pkg) = s.marks.iter().find(|m| m.matches(&w.def.author, &w.def.feature, &w.def.tags)).and_then(look) {
                targets.push((None, w.def.pos, pkg));
            }
        }
    }
    let s = st.surveyors.get_mut(&viewer).expect("checked");
    for (wearer, pkg, instance, helper) in s.mark_effects.drain(..) {
        records.push(crate::effects::stop_effect_package(wearer, pkg, 0, instance));
        if helper {
            records.push(crate::game::despawn_record(wearer));
        }
    }
    for (npc, pos, pkg) in targets {
        let wearer = match npc {
            Some(n) => n,
            None => {
                let (h, spawn) = helper_at(pf, pos);
                records.extend(spawn);
                h
            }
        };
        let instance = crate::effects::new_instance();
        records.push(crate::effects::effect_package_hook(wearer, pkg, "", "", 0.0, 0, instance));
        running.push((wearer, pkg, instance, npc.is_none()));
    }
    s.mark_effects = running;
    s.marks_at = Some((pf, revision));
    if !records.is_empty() {
        world::send_to(viewer, world::Out::Records(records));
    }
}

#[derive(Serialize)]
struct MarkLook {
    look: &'static str,
    effect: u32,
}

async fn mark_looks() -> R<Vec<MarkLook>> {
    with(|_| Ok(Json(MARK_LOOKS.iter().map(|l| MarkLook { look: l.0, effect: l.1 }).collect())))
}

/// An editor's marks (replacing the ones they had).
async fn set_marks(Path(account): Path<u32>, Json(marks): Json<Vec<Mark>>) -> R<PlayerState> {
    with(|st| {
        if let Some(m) = marks.iter().find(|m| !MARK_LOOKS.iter().any(|l| l.0 == m.look)) {
            return Err(Error::Invalid(format!("no mark look {} (mark-looks lists them)", m.look)));
        }
        let (player, _) = player_of(account)?;
        st.surveyors.entry(player).or_default().marks = marks;
        show_marks(st, player);
        state_of(st, account)
    })
    .map(Json)
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
    let Some(b) = crate::buildie::list(None).into_iter().find(|b| b.id == target) else {
        // a bouncer
        let Some(l) = crate::jumppad::by_dynel(target) else { return };
        let mut guard = STATE.lock().unwrap();
        let Some(st) = guard.as_mut() else { return };
        st.surveyors.entry(player).or_default().selected = Some(l.id.clone());
        mark(st, player);
        crate::chat::system(player.instance, &format!("selected [{}] {}", label(l.category), l.name));
        return;
    };
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
        feature: sv::features::live(),
        tags: Vec::new(),
        author: String::new(),
        updated: 0,
        playfield: b.playfield.model(),
        replaces: None,
        size: None,
        radius: None,
        color: None,
        action: None,
        once: None,
        target: None,
        chain: None,
    }
}

// ------------------------------------------------------------------ bouncers

/// Jump pads (jumppad.rs) as survey objects: base pads (a guessed landing point is a guess) and
/// the set's bouncer placements, which jumppad.rs applies (`crate::jumppad::set`).
fn bouncer_view(l: &crate::jumppad::Listed) -> SurveyObject {
    let p = l.placement.as_ref();
    SurveyObject {
        id: l.id.clone(),
        kind: SKind::Bouncer.as_str().into(),
        name: l.name.clone(),
        monster_data: None,
        template: Some(l.visual),
        level: None,
        pos: l.pos,
        heading: l.heading,
        category: l.category,
        wave: None,
        patrol: Vec::new(),
        comment: p.map(|p| p.comment.clone()).unwrap_or_default(),
        feature: p.map_or_else(sv::features::live, |p| p.feature.clone()),
        tags: p.map(|p| p.tags.clone()).unwrap_or_default(),
        author: p.map(|p| p.author.clone()).unwrap_or_default(),
        updated: p.map_or(0, |p| p.updated),
        playfield: l.playfield,
        replaces: p.and_then(|p| p.replaces.clone()),
        size: None,
        radius: None,
        color: None,
        action: None,
        once: None,
        target: Some(l.target),
        chain: (!l.chain.is_empty()).then(|| l.chain.clone()),
    }
}

/// The editor's fields of a bouncer to change (guesses and surveyed ones).
fn bouncer_placement(l: &crate::jumppad::Listed) -> Result<Placement, Error> {
    match (l.category, &l.placement) {
        (Category::Surveyed | Category::Guessed, Some(p)) => Ok(p.clone()),
        (c, _) => Err(Error::Invalid(format!("{} objects can only be hidden", label(c)))),
    }
}

/// Saves bouncer `p` (its record id) in playfield `pf`, or deletes it (`removed`), and puts the
/// world's pads in again. Its view (none when deleted).
fn store_bouncer(st: &mut State, work: &mut Work, pf: u32, p: Placement, author: &str) -> Result<Option<SurveyObject>, Error> {
    let key = Key::parse(&p.id, p.feature.clone()).map_err(Error::Invalid)?;
    if p.removed {
        let shown = work.versions(&p.id).into_iter().map(|(k, _)| k.clone()).find(|k| *k == key).or_else(|| work.versions(&p.id).into_iter().map(|(k, _)| k.clone()).next());
        if let Some(k) = shown {
            work.delete(&k, None, author, ids::now())?;
        }
    } else {
        let old = work.versions(&p.id).into_iter().map(|(k, _)| k.clone()).find(|k| *k != key);
        bouncers::store(work, &key, &p, author, ids::now())?;
        // moved to another feature: the old version goes
        if let Some(o) = old.filter(|o| o.feature != key.feature && !p.feature.is_empty()) {
            work.reset(&o, None)?;
        }
    }
    bouncers::apply(work, pf);
    sync_all(st);
    mark_selecting(st, &p.id);
    bump(st, pf);
    match crate::jumppad::find(&p.id) {
        Some(l) => Ok(Some(bouncer_view(&l))),
        None if p.removed => Ok(None),
        None => Err(Error::Invalid(format!("{}: saved, but its pad isn't in the world", p.id))),
    }
}

/// Edits a bouncer (a guessed one becomes a surveyed placement replacing it).
fn edit_bouncer(st: &mut State, work: &mut Work, l: &crate::jumppad::Listed, c: Changes, author: &str) -> Result<SurveyObject, Error> {
    if c.kind.is_some_and(|k| k != SKind::Bouncer) {
        return Err(Error::Invalid("a bouncer can't be changed into another kind".into()));
    }
    let feature = c.feature.clone().map(|f| checked_feature(&st.features, Some(f))).transpose()?;
    let mut p = bouncer_placement(l)?;
    if let Some(Some(t)) = c.template {
        p.template = Some(t);
    }
    if let Some(pos) = c.pos {
        p.pos = pos;
    }
    if let Some(h) = c.heading {
        p.heading = h;
    }
    if let Some(t) = c.target {
        p.target = t;
    }
    if let Some(n) = c.name.filter(|n| !n.trim().is_empty()) {
        p.name = n;
    }
    if let Some(t) = c.comment {
        p.comment = t;
    }
    if let Some(f) = feature {
        p.feature = f;
    }
    if let Some(t) = c.tags {
        p.tags = sv::tidy_tags(t);
    }
    if !author.is_empty() {
        p.author = author.to_string();
    }
    p.updated = ids::now();
    store_bouncer(st, work, l.playfield, p, author)?.ok_or_else(|| Error::NotFound(format!("{} is gone", l.id)))
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
    /// lmoxide_survey::features
    feature: String,
    /// labels like "approximate", "todo" (lmoxide_survey::tidy_tags)
    tags: Vec<String>,
    author: String,
    updated: u64,
    playfield: u32,
    /// The guess it replaces (confirmed or edited), which Reset brings back.
    #[serde(skip_serializing_if = "Option::is_none")]
    replaces: Option<Origin>,
    /// Trigger walls: width, height, depth; look; what crossing does; only once.
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<[f32; 3]>,
    /// Trigger radii: the radius (m)
    #[serde(skip_serializing_if = "Option::is_none")]
    radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<WallAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    once: Option<bool>,
    /// Bouncers: where it throws you (null: not set yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<Option<[f32; 3]>>,
    /// Bouncers whose landing point is on pads that throw on: the hops after `target`.
    #[serde(skip_serializing_if = "Option::is_none")]
    chain: Option<Vec<[f32; 3]>>,
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
    /// their marks on groups of objects
    marks: Vec<Mark>,
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
    /// lmoxide_survey::features: moves it to another feature
    feature: Option<String>,
    /// replaces its tags
    tags: Option<Vec<String>>,
    /// Trigger walls
    size: Option<[f32; 3]>,
    /// a trigger radius (m); null: a wall
    #[serde(default, deserialize_with = "some")]
    radius: Option<Option<f32>>,
    color: Option<String>,
    action: Option<WallAction>,
    once: Option<bool>,
    /// Bouncers: where it throws you (null: not set)
    #[serde(default, deserialize_with = "some")]
    target: Option<Option<[f32; 3]>>,
}

/// A field that may be given as null (`Some(None)`) or left out (`None`).
fn some<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
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

/// Runs `f` on the working copy and the survey state (in that lock order: crate::editor).
fn with_work<T>(f: impl FnOnce(&mut Work, &mut State) -> Result<T, Error>) -> Result<T, Error> {
    crate::editor::work(|work| with(|st| f(work, st))).unwrap_or(Err(Error::Off))
}

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
    Ok((id, pf.model()))
}

/// Who made the request (lmoxide-api's actor: "account:<name>" or "key:<name>"): the name.
pub(crate) fn author_of(headers: &HeaderMap) -> String {
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
        .route("/v1/survey/tags", get(tags))
        .route("/v1/survey/mark-looks", get(mark_looks))
        .route("/v1/survey/players/{account}/marks", put(set_marks))
        .route("/v1/survey/trigger-targets", get(trigger_targets))
        .route("/v1/survey/smash-achievements", get(smash_achievements))
        .route("/v1/survey/smash-achievements/{key}", put(save_smash_rule).delete(delete_smash_rule))
        .route("/v1/survey/achievement-rules", get(achievement_rules))
        .route("/v1/survey/enemy-names", get(enemy_names))
        .route("/v1/survey/players/{account}/select", post(select))
        .route("/v1/survey/players/{account}/hide", post(hide))
        .route("/v1/survey/players/{account}/unhide-all", post(unhide_all))
        .route("/v1/survey/players/{account}/shown-wave", post(shown_wave_route))
        .route("/v1/survey/players/{account}/preview-wave", post(preview_wave))
        .route("/v1/survey/players/{account}/teleport", post(teleport))
        .route("/v1/survey/playfields", get(playfields))
        .route("/v1/survey/playfields/{pf}/objects", get(objects).post(create_object))
        .route("/v1/survey/playfields/{pf}/notes", get(notes))
        .route("/v1/survey/playfields/{pf}/notes/{id}", put(save_note).delete(delete_note))
        .route("/v1/survey/playfields/{pf}/scripts/{id}", put(save_script).delete(delete_script))
        .route("/v1/survey/objects/{id}", axum::routing::patch(edit).delete(remove).put(put_object))
        .route("/v1/survey/objects/{id}/confirm", post(confirm))
        .route("/v1/survey/objects/{id}/forget", post(forget))
        .route("/v1/survey/features", get(features).post(create_feature))
        .route("/v1/survey/features/copy", post(copy_feature))
        .route("/v1/survey/unpublished", get(unpublished))
        .route("/v1/survey/publish", post(publish))
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
    with(|st| {
        let bouncers = crate::jumppad::looks().into_iter().map(|l| Placeable { kind: SKind::Bouncer, name: l.name, monster_data: None, template: Some(l.visual), level: None, used: Some(l.used) });
        Ok(Json(st.catalog.iter().map(|(p, _)| p.clone()).chain(bouncers).collect()))
    })
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
        marks: s.marks.clone(),
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
    /// The feature it belongs to (default live).
    #[serde(default)]
    feature: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

async fn place(Path(account): Path<u32>, headers: HeaderMap, Json(b): Json<PlaceBody>) -> R<SurveyObject> {
    with_work(|work, st| {
        let (player, pf) = player_of(account)?;
        let pos = world::live_position(player).unwrap_or_default();
        let heading = heading_of(world::facing_of(player).unwrap_or([0.0, 1.0]));
        let author = Some(author_of(&headers)).filter(|a| !a.is_empty()).unwrap_or_else(|| crate::sessions::players().into_iter().find(|p| p.account_id == account).map(|p| p.character_name).unwrap_or_default());
        if b.what.kind == SKind::Bouncer {
            // at the surveyor; where it throws you is set later (`target`)
            if b.what.template.is_none() {
                return Err(Error::Invalid("a bouncer needs its look (template: the pad's visual id, from the catalog)".into()));
            }
            let p = Placement {
                id: work.new_id("bouncer", if b.what.name.is_empty() { "bouncer" } else { &b.what.name }, Some(pf)),
                kind: SKind::Bouncer,
                name: b.what.name.clone(),
                monster_data: None,
                template: b.what.template,
                level: None,
                pos,
                heading,
                target: None,
                replaces: None,
                removed: false,
                wave: None,
                patrol: Vec::new(),
                comment: String::new(),
                tags: sv::tidy_tags(&b.tags),
                feature: checked_feature(&st.features, b.feature)?,
                author: author.clone(),
                updated: ids::now(),
            };
            let id = p.id.clone();
            st.surveyors.entry(player).or_default().selected = Some(id.clone());
            return store_bouncer(st, work, pf, p, &author)?.ok_or_else(|| Error::Invalid(format!("{id} wasn't placed")));
        }
        nkind(b.what.kind)?;
        if b.wave.as_ref().is_some_and(|w| !has_wave(&st.notes, pf, w)) {
            return Err(Error::Invalid("no such script wave in this playfield".into()));
        }
        let feature = checked_feature(&st.features, b.feature)?;
        let now = ids::now();
        let objects: Vec<&Object> = st.objects.values().collect();
        let template = records::template_for(work, &objects, &b.what, &author, now)?;
        let key = records::new_key(work, pf, &b.what.name, &feature);
        let mut o = records::new_object(key, template, &b.what, template_of(&st.catalog, b.what.kind, b.what.monster_data, b.what.template))?;
        o.playfield = pf;
        o.def.pos = pos;
        o.def.rot = Some(heading);
        o.wave = b.wave.clone();
        o.tags = sv::tidy_tags(&b.tags);
        records::store(work, &o, &author, now)?;
        let id = o.id.clone();
        let s = st.surveyors.entry(player).or_default();
        s.selected = Some(id.clone());
        if let Some(w) = &b.wave {
            // placing into a wave shows that wave
            s.shown.insert(w.script.clone(), w.wave.clone());
        }
        let view = replace_object(st, work, &id, &o.key).ok_or_else(|| Error::Invalid(format!("{id} was saved but can't be spawned")))?;
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
            let objects = st.objects.values().filter(|o| o.playfield == pf && surveyor.is_none_or(|s| !is_hidden_for(s, o, &st.notes))).map(|o| (&o.id, o.def.pos));
            let walls = st.walls.values().filter(|w| w.playfield == pf && surveyor.is_none_or(|s| !s.hidden.contains(&w.def.id))).map(|w| (w.def.id.clone(), w.def.pos));
            let buildies = crate::buildie::list(Some(crate::instance::Mode::WorldEditor.main(pf))).into_iter().map(|b| (buildie_id(&b), b.pos)).filter(|(id, _)| surveyor.is_none_or(|s| !s.hidden.contains(id)));
            let bouncers = crate::jumppad::list(Some(pf)).into_iter().map(|l| (l.id, l.pos)).filter(|(id, _)| surveyor.is_none_or(|s| !s.hidden.contains(id)));
            objects
                .map(|(id, p)| (id.clone(), p))
                .chain(walls)
                .chain(buildies)
                .chain(bouncers)
                .min_by(|a, b| distance(a.1, here).total_cmp(&distance(b.1, here)))
                .map(|(id, _)| id)
        } else {
            if let Some(id) = b.id.as_ref().filter(|id| !st.objects.contains_key(*id) && !st.walls.contains_key(*id) && buildie(id).is_none() && crate::jumppad::find(id).is_none()) {
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
                .chain(crate::buildie::list(Some(crate::instance::Mode::WorldEditor.main(pf))).iter().map(buildie_view))
                .chain(crate::jumppad::list(Some(pf)).iter().map(bouncer_view))
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
        let n = st.notes.get(&pf);
        Ok(Json(NotesView { notes: n.map(|n| n.notes.clone()).unwrap_or_default(), scripts: n.map(|n| n.scripts.clone()).unwrap_or_default() }))
    })
}

/// Saves a note as an area of the world (role note; a new one gets its id from its reference).
async fn save_note(Path((pf, id)): Path<(u32, String)>, headers: HeaderMap, Json(mut n): Json<Note>) -> R<Note> {
    with_work(|work, st| {
        n.id = id;
        n.feature = checked_feature(&st.features, Some(n.feature))?;
        n.tags = sv::tidy_tags(&n.tags);
        let saved = lmoxide_content::views::put_note(work, pf, &n, &author_of(&headers), ids::now())?;
        refresh_notes(st, work, pf);
        bump(st, pf);
        Ok(saved)
    })
    .map(Json)
}

/// Reads playfield `pf`'s notes and scripts again from the working copy.
fn refresh_notes(st: &mut State, work: &Work, pf: u32) {
    let notes = lmoxide_content::views::notes_of(work, pf);
    st.notes.insert(pf, notes);
}

/// The record a note or script id names (any feature).
fn record_key(work: &Work, id: &str, kind: RecordKind) -> Result<Key, Error> {
    work.versions(id).into_iter().map(|(k, _)| k.clone()).find(|k| k.id.kind == kind).ok_or_else(|| Error::NotFound(format!("no {kind} {id}")))
}

async fn delete_note(Path((pf, id)): Path<(u32, String)>, headers: HeaderMap) -> Result<StatusCode, Error> {
    with_work(|work, st| {
        let key = record_key(work, &id, RecordKind::Area)?;
        work.delete(&key, None, &author_of(&headers), ids::now())?;
        refresh_notes(st, work, pf);
        bump(st, pf);
        Ok(StatusCode::NO_CONTENT)
    })
}

/// Saves a script as its record (a new one gets its id from its name).
async fn save_script(Path((pf, id)): Path<(u32, String)>, headers: HeaderMap, Json(mut s): Json<Script>) -> R<Script> {
    with_work(|work, st| {
        if s.waves.is_empty() {
            return Err(Error::Invalid("a script has at least one wave".into()));
        }
        s.id = id;
        s.feature = checked_feature(&st.features, Some(s.feature))?;
        s.tags = sv::tidy_tags(&s.tags);
        for w in &mut s.waves {
            w.tags = sv::tidy_tags(&w.tags);
        }
        let saved = lmoxide_content::views::put_script(work, pf, &s, &author_of(&headers), ids::now())?;
        refresh_notes(st, work, pf);
        sync_all(st);
        bump(st, pf);
        Ok(saved)
    })
    .map(Json)
}

async fn delete_script(Path((pf, id)): Path<(u32, String)>, headers: HeaderMap) -> Result<StatusCode, Error> {
    with_work(|work, st| {
        if st.objects.values().any(|o| o.wave.as_ref().is_some_and(|w| w.script == id)) {
            return Err(Error::Invalid("delete or move the script's placements first".into()));
        }
        let key = record_key(work, &id, RecordKind::Script)?;
        let area = work.get(&key).and_then(|e| e.value.get("area")).and_then(|a| a.as_str()).map(str::to_string);
        work.delete(&key, None, &author_of(&headers), ids::now())?;
        // its own area too, when nothing else uses it
        if let Some(a) = area.and_then(|a| record_key(work, &a, RecordKind::Area).ok())
            && work.used_by(&a.id.to_string()).is_empty()
            && work.get(&a).is_some_and(|e| e.value.get("note").is_none())
        {
            work.delete(&a, None, &author_of(&headers), ids::now())?;
        }
        refresh_notes(st, work, pf);
        sync_all(st);
        bump(st, pf);
        Ok(StatusCode::NO_CONTENT)
    })
}

async fn edit(Path(id): Path<String>, headers: HeaderMap, Json(c): Json<Changes>) -> R<SurveyObject> {
    with_work(|work, st| {
        let author = author_of(&headers);
        if st.walls.contains_key(&id) {
            return edit_wall(st, work, &id, c, author);
        }
        if let Some(l) = crate::jumppad::find(&id) {
            return edit_bouncer(st, work, &l, c, &author);
        }
        let feature = c.feature.clone().map(|f| checked_feature(&st.features, Some(f))).transpose()?;
        let mut o = editable(st, &id)?;
        let pf = o.playfield;
        if let Some(Some(w)) = &c.wave
            && !has_wave(&st.notes, pf, w)
        {
            return Err(Error::Invalid("no such script wave in this playfield".into()));
        }
        let now = ids::now();
        if let Some(kind) = c.kind {
            // changed into another spawn: that look's template (its spells, health, ...)
            let what = Placeable { kind, name: o.def.name.clone(), monster_data: c.monster_data.flatten(), template: c.template.flatten(), level: Some(o.def.level), used: None };
            let objects: Vec<&Object> = st.objects.values().collect();
            o.template = Some(records::template_for(work, &objects, &what, &author, now)?);
            o.def.kind = nkind(kind)?;
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
        if let Some(t) = c.tags {
            o.tags = sv::tidy_tags(t);
        }
        if let Some(w) = c.wave {
            o.wave = w;
        }
        if let Some(p) = c.patrol {
            o.def.patrol = if walks(o.def.kind) { p } else { Vec::new() };
        }
        let old = o.key.clone();
        if let Some(f) = feature {
            // moved to another feature: its version there, the old one gone
            o.feature = f.clone();
            o.key.feature = f;
        }
        records::store(work, &o, &author, now)?;
        if o.key != old {
            work.reset(&old, None)?;
        }
        let view = replace_object(st, work, &id, &o.key).ok_or_else(|| Error::Invalid(format!("{id} was saved but can't be spawned (its template?)")))?;
        sync_all(st);
        mark_selecting(st, &id);
        bump(st, pf);
        Ok(view)
    })
    .map(Json)
}

async fn confirm(Path(id): Path<String>, headers: HeaderMap) -> R<SurveyObject> {
    with_work(|work, st| {
        if st.walls.contains_key(&id) {
            return Err(Error::Invalid("trigger walls are surveyed already".into()));
        }
        let author = author_of(&headers);
        if let Some(l) = crate::jumppad::find(&id) {
            return edit_bouncer(st, work, &l, Changes { tags: Some(l.placement.as_ref().map(|p| p.tags.iter().cloned().chain(["verified".to_string()]).collect()).unwrap_or_default()), ..Changes::default() }, &author);
        }
        // confirmed: surveyed, and tagged so
        let mut o = editable(st, &id)?;
        if !o.tags.iter().any(|t| t == "verified") {
            o.tags.push("verified".into());
        }
        records::store(work, &o, &author, ids::now())?;
        // its name changes from [guess] to [surveyed]
        let view = replace_object(st, work, &id, &o.key).ok_or_else(|| Error::Invalid(format!("{id} can't be spawned")))?;
        sync_all(st);
        mark_selecting(st, &id);
        bump(st, o.playfield);
        Ok(view)
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
    with_work(|work, st| {
        if st.walls.contains_key(&id) {
            remove_wall(st, work, &id, &author_of(&headers))?;
            return Ok(Json(Removed { id }));
        }
        if let Some(l) = crate::jumppad::find(&id) {
            let mut p = bouncer_placement(&l)?;
            p.removed = true;
            store_bouncer(st, work, l.playfield, p, &author_of(&headers))?;
            deselect(st, &id);
            return Ok(Json(Removed { id }));
        }
        let o = editable(st, &id)?;
        // (refused while other records name it: a bouncer that needs it built, a wave's health trigger)
        work.delete(&o.key, None, &author_of(&headers), ids::now())?;
        if let Some(mut o) = st.objects.remove(&id) {
            if let Some(n) = o.npc {
                st.by_npc.remove(&n);
            }
            despawn(&mut o);
        }
        deselect(st, &id);
        bump(st, o.playfield);
        // (forget it to bring it back: its committed version)
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

/// Drops a placement or trigger wall record entirely (no deletion record stays): undoing a
/// placement, or resetting an edited guess (the guess comes back, its view is the answer).
async fn forget(Path(id): Path<String>) -> R<Option<SurveyObject>> {
    with_work(|work, st| {
        // a trigger wall: back to its committed version (none: gone)
        if let Ok(key) = Key::parse(&id, st.walls.get(&id).map_or_else(|| "live".to_string(), |w| w.def.feature.clone()))
            && key.id.kind == RecordKind::Trigger
        {
            let pf = key.id.world.unwrap_or(0);
            git::restore(work, &key)?;
            st.walls.remove(&id); // despawns its pieces
            deselect(st, &id);
            let l = lmoxide_content::views::Lookups::new(work);
            let back = lmoxide_content::views::wall_view(work, &key, &l).map(|def| {
                let mut w = Wall { def, playfield: pf, pieces: Vec::new(), fired: HashSet::new() };
                spawn_wall(&st.surveyors, &mut w);
                let view = w.view();
                st.walls.insert(id.clone(), w);
                view
            });
            bump(st, pf);
            return Ok(Json(back));
        }
        // a bouncer: back to its committed version
        if let Ok(key) = Key::parse(&id, crate::jumppad::find(&id).and_then(|l| l.placement).map_or_else(|| "live".to_string(), |p| p.feature))
            && key.id.kind == RecordKind::Bouncer
        {
            let pf = key.id.world.unwrap_or(0);
            git::restore(work, &key)?;
            bouncers::apply(work, pf);
            deselect(st, &id);
            sync_all(st);
            bump(st, pf);
            return Ok(Json(crate::jumppad::find(&id).map(|l| bouncer_view(&l))));
        }
        // a spawn: back to its committed version (a new one goes, a deleted one comes back)
        let key = st.objects.get(&id).map(|o| o.key.clone()).or_else(|| Key::parse(&id, "live").ok()).filter(|k| k.id.kind == RecordKind::Spawn).ok_or_else(|| Error::NotFound(format!("no object {id}")))?;
        let pf = key.id.world.unwrap_or(0);
        git::restore(work, &key)?;
        deselect(st, &id);
        let back = match work.get(&key) {
            Some(_) => replace_object(st, work, &id, &key),
            None => {
                if let Some(mut o) = st.objects.remove(&id) {
                    if let Some(n) = o.npc {
                        st.by_npc.remove(&n);
                    }
                    despawn(&mut o);
                }
                None
            }
        };
        sync_all(st);
        bump(st, pf);
        Ok(Json(back))
    })
}

/// An object as a snapshot has it (the world editor's undo and redo, copy and paste).
#[derive(Deserialize)]
struct Snapshot {
    /// (pasting: the path's playfield instead)
    #[serde(default)]
    playfield: Option<u32>,
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
    #[serde(default = "sv::features::live")]
    feature: String,
    #[serde(default)]
    tags: Vec<String>,
    size: Option<[f32; 3]>,
    #[serde(default)]
    radius: Option<f32>,
    color: Option<String>,
    action: Option<WallAction>,
    #[serde(default)]
    once: Option<bool>,
    /// Bouncers: where it throws you
    #[serde(default)]
    target: Option<[f32; 3]>,
}

/// Makes `id` exactly as the snapshot has it (spawned again; a guess it replaces goes away).
async fn put_object(Path(id): Path<String>, headers: HeaderMap, Json(b): Json<Snapshot>) -> R<SurveyObject> {
    with_work(|work, st| {
        let pf = b.playfield.ok_or_else(|| Error::Invalid("playfield?".into()))?;
        put_snapshot(st, work, id, pf, b, author_of(&headers))
    })
    .map(Json)
}

/// A new record in playfield `pf` from a snapshot (the world editor's paste): a new id, the
/// feature and wave checked, never replacing a guess; nobody's selection changes.
async fn create_object(Path(pf): Path<u32>, headers: HeaderMap, Json(mut b): Json<Snapshot>) -> R<SurveyObject> {
    with_work(|work, st| {
        let author = author_of(&headers);
        b.feature = checked_feature(&st.features, Some(b.feature))?;
        if b.wave.as_ref().is_some_and(|w| !has_wave(&st.notes, pf, w)) {
            return Err(Error::Invalid("no such script wave in this playfield".into()));
        }
        b.replaces = None;
        let id = if b.kind == "trigger" {
            ids::new_id("t", &author)
        } else if b.kind == "bouncer" {
            work.new_id("bouncer", if b.name.is_empty() { "bouncer" } else { &b.name }, Some(pf))
        } else {
            records::new_key(work, pf, &b.name, &b.feature).id.to_string()
        };
        put_snapshot(st, work, id, pf, b, author)
    })
    .map(Json)
}

/// Makes `id` in playfield `pf` exactly as the snapshot has it, saved and spawned (again).
fn put_snapshot(st: &mut State, work: &mut Work, id: String, pf: u32, b: Snapshot, author: String) -> Result<SurveyObject, Error> {
    if b.kind == "trigger" {
        let def = TriggerWall {
            id: id.clone(),
            name: b.name,
            pos: b.pos,
            heading: b.heading,
            size: checked_size(b.size.unwrap_or(WALL_SIZE)),
            radius: checked_radius(b.radius),
            color: checked_color(b.color, b.radius.is_some())?,
            action: b.action.unwrap_or(WallAction::Other),
            once: b.once.unwrap_or(false),
            comment: b.comment,
            removed: false,
            tags: sv::tidy_tags(&b.tags),
            feature: b.feature,
            author: author.clone(),
            updated: ids::now(),
        };
        st.walls.remove(&id);
        let def = store_wall(work, &def, pf, &author)?;
        let id = def.id.clone();
        let mut w = Wall { def, playfield: pf, pieces: Vec::new(), fired: HashSet::new() };
        spawn_wall(&st.surveyors, &mut w);
        let view = w.view();
        st.walls.insert(id.clone(), w);
        mark_selecting(st, &id);
        bump(st, pf);
        return Ok(view);
    }
    let kind: SKind = serde_json::from_value(serde_json::Value::String(b.kind.clone())).map_err(|_| Error::Invalid(format!("kind {}?", b.kind)))?;
    if kind == SKind::Bouncer {
        if b.template.is_none() {
            return Err(Error::Invalid("a bouncer needs its look (template: the pad's visual id, from the catalog)".into()));
        }
        let p = Placement {
            id: id.clone(),
            kind,
            name: b.name,
            monster_data: None,
            template: b.template,
            level: None,
            pos: b.pos,
            heading: b.heading,
            target: b.target,
            replaces: b.replaces,
            removed: false,
            wave: None,
            patrol: Vec::new(),
            comment: b.comment,
            tags: sv::tidy_tags(&b.tags),
            feature: b.feature,
            author: author.clone(),
            updated: ids::now(),
        };
        return store_bouncer(st, work, pf, p, &author)?.ok_or_else(|| Error::Invalid(format!("{id} wasn't saved")));
    }
    let key = Key::parse(&id, b.feature.clone()).map_err(Error::Invalid)?;
    let what = Placeable { kind, name: b.name.clone(), monster_data: b.monster_data, template: b.template, level: b.level, used: None };
    let now = ids::now();
    let objects: Vec<&Object> = st.objects.values().collect();
    let template = records::template_for(work, &objects, &what, &author, now)?;
    let mut o = records::new_object(key, template, &what, template_of(&st.catalog, kind, b.monster_data, b.template))?;
    o.playfield = pf;
    o.def.pos = b.pos;
    o.def.rot = Some(b.heading);
    o.wave = b.wave;
    o.def.patrol = if walks(o.def.kind) { b.patrol } else { Vec::new() };
    o.comment = b.comment;
    o.tags = sv::tidy_tags(&b.tags);
    records::store(work, &o, &author, now)?;
    let view = replace_object(st, work, &id, &o.key).ok_or_else(|| Error::Invalid("that kind can't be placed yet".into()))?;
    sync_all(st);
    mark_selecting(st, &id);
    bump(st, pf);
    Ok(view)
}

/// Spawns these playfields again from the working copy and the set (what changed is easiest
/// seen that way).
fn rebuild(st: &mut State, work: &Work, pfs: impl IntoIterator<Item = u32>) {
    for pf in pfs.into_iter().collect::<std::collections::BTreeSet<_>>() {
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
        build_playfield(st, work, pf);
        bump(st, pf);
    }
    sync_all(st);
    let viewers: Vec<Id32> = st.surveyors.keys().copied().collect();
    for v in viewers {
        mark(st, v);
    }
}

// ------------------------------------------------------------------ features

#[derive(Serialize)]
struct FeatureView {
    id: String,
    title: String,
    /// "New content > Christmas Event > 2026"
    path: String,
    category: String,
    comment: String,
    copied_from: Option<String>,
    author: String,
    updated: u64,
}

#[derive(Serialize)]
struct FeaturesView {
    /// the features' first level: live (mimic live), feature (new content), other
    categories: Vec<FeatureView>,
    features: Vec<FeatureView>,
}

fn features_view(f: &sv::features::FeaturesFile) -> FeaturesView {
    let cat = |(id, title): &(&str, &str)| FeatureView {
        id: id.to_string(),
        title: title.to_string(),
        path: title.to_string(),
        category: id.to_string(),
        comment: String::new(),
        copied_from: None,
        author: String::new(),
        updated: 0,
    };
    FeaturesView {
        categories: sv::features::CATEGORIES.iter().map(cat).collect(),
        features: f
            .features
            .iter()
            .map(|x| FeatureView {
                id: x.id.clone(),
                title: x.title.clone(),
                path: f.path_title(&x.id),
                category: sv::features::category(&x.id).to_string(),
                comment: x.comment.clone(),
                copied_from: x.copied_from.clone(),
                author: x.author.clone(),
                updated: x.updated,
            })
            .collect(),
    }
}

async fn features() -> R<FeaturesView> {
    with(|st| Ok(Json(features_view(&st.features))))
}

#[derive(Deserialize)]
struct NewFeature {
    /// a category (`live`, `feature`, `other`) or a feature to nest under
    parent: String,
    title: String,
    #[serde(default)]
    comment: String,
}

fn new_feature(features: &sv::features::FeaturesFile, parent: &str, title: &str, comment: String, author: String) -> Result<sv::features::Feature, Error> {
    if !features.known(parent) {
        return Err(Error::Invalid(format!("no feature {parent}")));
    }
    let slug = sv::features::slug(title);
    if slug.is_empty() {
        return Err(Error::Invalid("a feature needs a title with letters or digits".into()));
    }
    Ok(sv::features::Feature { id: format!("{parent}/{slug}"), title: title.trim().into(), comment, copied_from: None, author, updated: ids::now() })
}

async fn create_feature(headers: HeaderMap, Json(b): Json<NewFeature>) -> R<FeaturesView> {
    with_work(|work, st| {
        let f = new_feature(&st.features, &b.parent, &b.title, b.comment, author_of(&headers))?;
        let mut features = work.features();
        features.add(f).map_err(|e| Error::Invalid(e.to_string()))?;
        work.save_features(&features)?;
        st.features = features;
        Ok(Json(features_view(&st.features)))
    })
}

#[derive(Deserialize)]
struct CopyFeature {
    from: String,
    parent: String,
    title: String,
    #[serde(default)]
    comment: String,
}

#[derive(Serialize)]
struct Copied {
    id: String,
    records: usize,
    features: FeaturesView,
}

/// Copies a feature with everything in it (each record gets a version in the new feature), so
/// next year's event starts as this year's (the copies are spawned too).
async fn copy_feature(headers: HeaderMap, Json(b): Json<CopyFeature>) -> R<Copied> {
    with_work(|work, st| {
        let author = author_of(&headers);
        let mut f = new_feature(&st.features, &b.parent, &b.title, b.comment, author.clone())?;
        if sv::features::within(&f.id, &b.from) {
            return Err(Error::Invalid("a feature can't be copied into itself".into()));
        }
        if !st.features.known(&b.from) {
            return Err(Error::Invalid(format!("no feature {}", b.from)));
        }
        f.copied_from = Some(b.from.clone());
        let id = f.id.clone();
        let mut features = work.features();
        // the features nested under it come along
        let nested: Vec<sv::features::Feature> = features.features.iter().filter(|x| sv::features::within(&x.id, &b.from) && x.id != b.from).cloned().collect();
        features.add(f).map_err(|e| Error::Invalid(e.to_string()))?;
        for n in nested {
            let nid = format!("{id}{}", &n.id[b.from.len()..]);
            if features.get(&nid).is_none() {
                features.add(sv::features::Feature { id: nid, copied_from: Some(n.id.clone()), author: author.clone(), updated: ids::now(), ..n }).map_err(|e| Error::Invalid(e.to_string()))?;
            }
        }
        work.save_features(&features)?;
        st.features = features;
        let records = work.copy_feature(&b.from, &id, &author, ids::now())?;
        if records > 0 {
            let pfs: Vec<u32> = st.objects.values().map(|o| o.playfield).collect();
            rebuild(st, work, pfs);
        }
        info!(from = %b.from, to = %id, records, "survey: feature copied");
        Ok(Json(Copied { id, records, features: features_view(&st.features) }))
    })
}

// ------------------------------------------------------------------ publishing

/// One editor's changes to one feature in one playfield that aren't published yet (the working
/// copy's records that differ from its last commit and aren't in the publish queue).
#[derive(Serialize)]
struct Unpublished {
    /// 0: definitions (NPCs, achievements, ...), not a world's.
    playfield: u32,
    playfield_name: String,
    feature: String,
    feature_path: String,
    author: String,
    /// The caller's own changes.
    mine: bool,
    /// "2 added, 1 changed".
    summary: String,
    items: Vec<UnpublishedItem>,
}

#[derive(Serialize)]
struct UnpublishedItem {
    /// placement, trigger, note, script, smash_rule, or record (a definition)
    kind: &'static str,
    id: String,
    name: String,
    /// added, changed, removed
    op: &'static str,
    /// The fields that changed.
    details: Vec<String>,
    updated: u64,
    tags: Vec<String>,
}

/// A playfield's name ("Definitions" for records of no world, else its number).
fn playfield_name(pf: u32) -> String {
    if pf == 0 {
        return "Definitions".into();
    }
    crate::teleport::destination(pf).map_or_else(|| pf.to_string(), |d| d.name.clone())
}

/// The working copy's records changed since its last commit and not published (queued) yet.
fn unpublished_changes(work: &Work) -> Result<Vec<git::Change>, Error> {
    let queue = lmoxide_content::queue::Queue::new(&crate::editor::queue_dir(work));
    Ok(git::changes(work).map_err(|e| Error::Invalid(format!("{e:#}")))?.into_iter().filter(|c| !queue.has(&c.key, c.after.as_ref())).collect())
}

/// The unpublished changes by world, feature and author (the caller's first).
fn unpublished_of(st: &State, work: &Work, me: &str) -> Result<Vec<Unpublished>, Error> {
    let mut groups: BTreeMap<(u32, String, String), Vec<UnpublishedItem>> = BTreeMap::new();
    for c in unpublished_changes(work)? {
        let v = c.after.as_ref().or(c.before.as_ref()).cloned().unwrap_or_default();
        let kind = match c.key.id.kind {
            RecordKind::Spawn | RecordKind::Bouncer => "placement",
            RecordKind::Trigger => "trigger",
            RecordKind::Area if v.get("note").is_some() => "note",
            RecordKind::Script => "script",
            RecordKind::Achievement if v.get("rule").is_some() => "smash_rule",
            _ => "record",
        };
        let fields = |x: &Option<serde_json::Value>| x.as_ref().and_then(|x| x.as_object()).cloned().unwrap_or_default();
        let (before, after) = (fields(&c.before), fields(&c.after));
        let details: Vec<String> = if c.change == "changed" {
            before.keys().chain(after.keys()).filter(|k| !["rev", "author", "updated"].contains(&k.as_str()) && before.get(*k) != after.get(*k)).cloned().collect::<std::collections::BTreeSet<_>>().into_iter().collect()
        } else {
            Vec::new()
        };
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let name = [str_of("name"), str_of("title"), str_of("client_name")].into_iter().find(|n| !n.is_empty()).unwrap_or_else(|| c.key.id.to_string());
        groups.entry((c.key.id.world.unwrap_or(0), c.key.feature.clone(), c.author.clone())).or_default().push(UnpublishedItem {
            kind,
            id: c.key.id.to_string(),
            name,
            op: c.change,
            details,
            updated: v.get("updated").and_then(|u| u.as_u64()).unwrap_or(0),
            tags: v.get("tags").and_then(|t| t.as_array()).into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect(),
        });
    }
    let mut list: Vec<Unpublished> = groups
        .into_iter()
        .map(|((pf, feature, author), items)| {
            let n = |op: &str| items.iter().filter(|i| i.op == op).count();
            let summary = [("added", n("added")), ("changed", n("changed")), ("removed", n("removed"))].into_iter().filter(|(_, n)| *n > 0).map(|(op, n)| format!("{n} {op}")).collect::<Vec<_>>().join(", ");
            Unpublished { playfield: pf, playfield_name: playfield_name(pf), feature_path: st.features.path_title(&feature), feature, mine: author == me, author, summary, items }
        })
        .collect();
    list.sort_by_key(|u| !u.mine);
    Ok(list)
}

async fn unpublished(headers: HeaderMap) -> R<Vec<Unpublished>> {
    let me = author_of(&headers);
    with_work(|work, st| Ok(Json(unpublished_of(st, work, &me)?)))
}

#[derive(Deserialize)]
struct PublishRequest {
    /// Only this playfield's changes.
    playfield: Option<u32>,
    /// Only this feature's changes.
    feature: Option<String>,
    /// Whose changes: default the caller's; `*`: everyone's.
    author: Option<String>,
    /// Only these records.
    ids: Option<Vec<String>>,
}

#[derive(Serialize)]
struct Published {
    records: usize,
    /// What is still unpublished.
    unpublished: Vec<Unpublished>,
}

/// Publishes changes: they become one entry of the publish queue (var/content-queue), for an
/// admin to approve; they stay in the working copy as they are.
async fn publish(headers: HeaderMap, Json(r): Json<PublishRequest>) -> R<Published> {
    let me = author_of(&headers);
    let author = r.author.clone().unwrap_or(me.clone());
    with_work(|work, st| {
        let chosen: Vec<git::Change> = unpublished_changes(work)?
            .into_iter()
            .filter(|c| r.playfield.is_none_or(|p| c.key.id.world.unwrap_or(0) == p) && r.feature.as_ref().is_none_or(|f| &c.key.feature == f) && (author == "*" || c.author == author))
            .filter(|c| r.ids.as_ref().is_none_or(|ids| ids.contains(&c.key.id.to_string())))
            .collect();
        let records = chosen.len();
        if records > 0 {
            let worlds: std::collections::BTreeSet<String> = chosen.iter().map(|c| playfield_name(c.key.id.world.unwrap_or(0))).collect();
            let features: std::collections::BTreeSet<&str> = chosen.iter().map(|c| c.key.feature.as_str()).collect();
            let summary = format!("{}: {records} records (feature {})", worlds.into_iter().collect::<Vec<_>>().join(", "), features.into_iter().collect::<Vec<_>>().join(", "));
            let entry = lmoxide_content::queue::Entry { seq: 0, created: ids::now(), by: me.clone(), summary, status: "queued".into(), reason: None, records: chosen.into_iter().map(Into::into).collect() };
            let e = lmoxide_content::queue::Queue::new(&crate::editor::queue_dir(work)).add(entry).map_err(|e| Error::Invalid(format!("not queued: {e}")))?;
            info!(by = %me, author = %author, records, seq = e.seq, "survey: published (queued)");
        }
        Ok(Json(Published { records, unpublished: unpublished_of(st, work, &me)? }))
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
    // (not the red arena gate, 155861: it is solid, so the wall can't be walked through; walls
    // saved as red show green)
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
            feature: d.feature.clone(),
            tags: d.tags.clone(),
            author: d.author.clone(),
            updated: d.updated,
            playfield: self.playfield,
            replaces: None,
            size: Some(d.size),
            radius: d.radius,
            color: Some(d.color.clone()),
            action: Some(d.action.clone()),
            once: Some(d.once),
            target: None,
            chain: None,
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
    // the look's own middle (the forcefield's runs from -8 to 0)
    let offset = (x0 + x1) / 2.0 * scale;
    // each piece: its middle and heading (across = the piece's own x axis, (cos h, -sin h) in x/z)
    let places: Vec<([f32; 2], f32)> = match w.def.radius {
        // a trigger radius: a ring of pieces around the circle, each along it
        Some(r) => {
            let n = ((std::f32::consts::TAU * r / piece).ceil() as usize).clamp(3, 48);
            (0..n)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / n as f32;
                    ([w.def.pos[0] + r * a.sin(), w.def.pos[2] + r * a.cos()], a)
                })
                .collect()
        }
        None => {
            let n = ((width / piece).ceil() as usize).clamp(1, 64);
            let step = if n > 1 { (width - piece) / (n - 1) as f32 } else { 0.0 };
            let (sin, cos) = w.def.heading.sin_cos();
            (0..n)
                .map(|i| {
                    let across = if n > 1 { -width / 2.0 + piece / 2.0 + step * i as f32 } else { 0.0 };
                    ([w.def.pos[0] + across * cos, w.def.pos[2] - across * sin], w.def.heading)
                })
                .collect()
        }
    };
    let what = if w.def.radius.is_some() { "radius" } else { "wall" };
    let name = format!("[trigger] {}", if w.def.name.is_empty() { what } else { &w.def.name });
    let hiding: Vec<Id32> = surveyors.iter().filter(|(_, s)| s.hidden.contains(&w.def.id)).map(|(v, _)| *v).collect();
    // the looks are one-sided (seen from behind they vanish): each piece twice, back to back
    let places = places.into_iter().flat_map(|(at, h)| [(at, h), (at, h + std::f32::consts::PI)]);
    for ([x, z], heading) in places {
        let (sin, cos) = heading.sin_cos();
        let half = heading / 2.0;
        let rot = [0.0, half.sin(), 0.0, half.cos()];
        let pos = [x - offset * cos, w.def.pos[1], z + offset * sin];
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
        let presence = world::join(id, crate::instance::Mode::WorldEditor.main(pf), Arc::new(Mutex::new(pos)), move || crate::scenery::visual_records(id, crate::instance::Mode::WorldEditor.main(pf), pos, rot, &look), outbox);
        w.pieces.push((id, presence));
    }
}

/// Saves a wall as its trigger record: as saved (a new one with its record id).
fn store_wall(work: &mut Work, def: &TriggerWall, pf: u32, author: &str) -> Result<TriggerWall, Error> {
    Ok(lmoxide_content::views::put_wall(work, pf, def, author, ids::now())?)
}

#[derive(Serialize)]
struct WallLook {
    color: &'static str,
    /// `wall` or `radius`: which shape it draws
    shape: &'static str,
}

/// The looks trigger walls and radii can have (their `color`).
async fn wall_looks() -> R<Vec<WallLook>> {
    // a trigger radius is a ring of the same pieces
    let walls = WALL_LOOKS.iter().map(|l| WallLook { color: l.0, shape: "wall" });
    let circles = WALL_LOOKS.iter().map(|l| WallLook { color: l.0, shape: "radius" });
    with(|_| Ok(Json(walls.chain(circles).collect())))
}

#[derive(Serialize, Debug, PartialEq)]
struct TagUse {
    tag: String,
    /// records of the working set carrying it
    count: usize,
    /// one of the tags every editor is offered
    suggested: bool,
}

/// Every tag on a record of the working copy (and on script waves), most used first, with the
/// suggested ones.
fn tag_uses(work: &Work) -> Vec<TagUse> {
    let mut counts: BTreeMap<String, usize> = sv::SUGGESTED_TAGS.iter().map(|t| (t.to_string(), 0)).collect();
    let strings = |v: Option<&serde_json::Value>| v.and_then(|t| t.as_array()).into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect::<Vec<_>>();
    for (_, e) in work.records() {
        for t in strings(e.value.get("tags")) {
            *counts.entry(t).or_default() += 1;
        }
        for w in e.value.get("waves").and_then(|w| w.as_array()).into_iter().flatten() {
            for t in strings(w.get("tags")) {
                *counts.entry(t).or_default() += 1;
            }
        }
    }
    let mut out: Vec<TagUse> = counts.into_iter().map(|(t, count)| TagUse { suggested: sv::SUGGESTED_TAGS.contains(&t.as_str()), tag: t, count }).collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    out
}

async fn tags() -> R<Vec<TagUse>> {
    with_work(|work, _| Ok(Json(tag_uses(work))))
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
    /// The feature it belongs to (default live).
    #[serde(default)]
    feature: Option<String>,
    /// A trigger radius (m) instead of a wall.
    #[serde(default)]
    radius: Option<f32>,
    #[serde(default)]
    tags: Vec<String>,
}

fn checked_feature(features: &sv::features::FeaturesFile, feature: Option<String>) -> Result<String, Error> {
    match feature {
        None => Ok(sv::features::live()),
        Some(f) if features.known(&f) => Ok(f),
        Some(f) => Err(Error::Invalid(format!("no feature {f} (create it first)"))),
    }
}

fn checked_size(size: [f32; 3]) -> [f32; 3] {
    size.map(|v| if v.is_finite() { v.clamp(0.1, 200.0) } else { 1.0 })
}

/// A look for a wall or a trigger radius (a ring of the same pieces).
fn checked_color(color: Option<String>, _circle: bool) -> Result<String, Error> {
    match color {
        None => Ok(WALL_LOOKS[0].0.into()),
        // the red arena gate is gone (solid): its walls become green; (the pressure plates an
        // early trigger radius had too)
        Some(c) if c == "red" || c.ends_with(" plate") => Ok(WALL_LOOKS[0].0.into()),
        Some(c) if WALL_LOOKS.iter().any(|l| l.0 == c) => Ok(c),
        Some(c) => Err(Error::Invalid(format!("no wall look {c} (wall-looks lists them)"))),
    }
}

/// A trigger radius: at least half a metre, at most 100.
fn checked_radius(r: Option<f32>) -> Option<f32> {
    r.map(|r| if r.is_finite() { r.clamp(0.5, 100.0) } else { 5.0 })
}

/// A trigger wall where the surveyor stands, across their way.
async fn place_trigger(Path(account): Path<u32>, headers: HeaderMap, Json(b): Json<PlaceTriggerBody>) -> R<SurveyObject> {
    with_work(|work, st| {
        let (player, pf) = player_of(account)?;
        let author = Some(author_of(&headers)).filter(|a| !a.is_empty()).unwrap_or_else(|| crate::sessions::players().into_iter().find(|p| p.account_id == account).map(|p| p.character_name).unwrap_or_default());
        let def = TriggerWall {
            id: ids::new_id("t", &author),
            name: b.name,
            pos: world::live_position(player).unwrap_or_default(),
            heading: heading_of(world::facing_of(player).unwrap_or([0.0, 1.0])),
            size: checked_size(b.size.unwrap_or(WALL_SIZE)),
            radius: checked_radius(b.radius),
            color: checked_color(b.color, b.radius.is_some())?,
            action: b.action.unwrap_or(WallAction::Other),
            once: b.once,
            comment: String::new(),
            removed: false,
            tags: sv::tidy_tags(&b.tags),
            feature: checked_feature(&st.features, b.feature)?,
            author: author.clone(),
            updated: ids::now(),
        };
        let def = store_wall(work, &def, pf, &author)?;
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

fn edit_wall(st: &mut State, work: &mut Work, id: &str, c: Changes, author: String) -> Result<SurveyObject, Error> {
    let circle = c.radius.map_or_else(|| st.walls[id].def.radius.is_some(), |r| r.is_some());
    // (a change of shape keeps a look of that shape)
    let color = checked_color(Some(c.color.clone().unwrap_or_else(|| st.walls[id].def.color.clone())), circle)?;
    let w = st.walls.get_mut(id).expect("checked");
    let d = &mut w.def;
    if let Some(r) = c.radius {
        d.radius = checked_radius(r);
    }
    d.color = color;
    if let Some(p) = c.pos {
        d.pos = p;
    }
    if let Some(h) = c.heading {
        d.heading = h;
    }
    if let Some(size) = c.size {
        d.size = checked_size(size);
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
    if let Some(f) = c.feature {
        d.feature = checked_feature(&st.features, Some(f))?;
    }
    if let Some(t) = c.tags {
        d.tags = sv::tidy_tags(t);
    }
    if let Some(t) = c.comment {
        d.comment = t;
    }
    let (def, pf) = (d.clone(), w.playfield);
    let saved = store_wall(work, &def, pf, &author)?;
    let w = st.walls.get_mut(id).expect("checked");
    w.def = saved;
    // drawn again where and as it is now
    w.pieces.clear();
    spawn_wall(&st.surveyors, w);
    mark_selecting(st, id);
    bump(st, pf);
    Ok(st.walls[id].view())
}

/// Deletes a wall's trigger record (refused while other records name it).
fn remove_wall(st: &mut State, work: &mut Work, id: &str, author: &str) -> Result<(), Error> {
    let Some(w) = st.walls.get(id) else { return Ok(()) };
    let pf = w.playfield;
    let key = Key::parse(id, w.def.feature.clone()).map_err(Error::Invalid)?;
    work.delete(&key, None, author, ids::now())?;
    st.walls.remove(id); // despawns its pieces
    deselect(st, id);
    bump(st, pf);
    Ok(())
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
                let pf = world::whereabouts(id)?.0.model();
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
        // marks follow changes and moves to another playfield
        let stale: Vec<Id32> = st
            .surveyors
            .iter()
            .filter(|(_, s)| !s.marks.is_empty() || !s.mark_effects.is_empty())
            .filter(|(v, s)| {
                let pf = st.last_pos.get(v).map(|(pf, _)| *pf);
                pf.is_some_and(|pf| s.marks_at != Some((pf, st.revision.get(&pf).copied().unwrap_or(0))))
            })
            .map(|(v, _)| *v)
            .collect();
        for v in stale {
            show_marks(st, v);
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
            let s = script.as_ref().and_then(|sid| scripts_of(&st.notes, pf).iter().find(|s| &s.id == sid)).cloned();
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
        WallAction::Collect { drop } => {
            let what = drop.as_deref().map(|k| match smash_target(k) {
                Ok(a) => a.title,
                Err(_) => k.strip_prefix(sv::DROP_PREFIX).unwrap_or(k).to_string(),
            });
            say(format!("collects {}", what.unwrap_or_else(|| "something (none chosen yet)".into())))
        }
        WallAction::Other => say(if def.comment.is_empty() { "crossed".into() } else { def.comment.clone() }),
    }
}

// ------------------------------------------------------------------ trigger wall targets

#[derive(Serialize, Clone)]
struct TargetLocation {
    /// the bounded area's (or world object's) name, what a wall stores ("Achievement_Volcano")
    name: String,
    /// what players call it ("Volcano Summit", from its achievements' texts), if known
    title: Option<String>,
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
    /// the client's internal name, its group cut off ("Lava Pier")
    name: String,
    /// the achievement's internal group ("Pirate Exploration")
    category: String,
    /// as the game shows it: its title ("Fiery Explorer"), text ("Find the Volcano Summit in
    /// Dinosaur Rise.") and the title of the achievement it is part of ("Paleontologist")
    title: Option<String>,
    text: Option<String>,
    group: Option<String>,
    /// the locations it needs (all of them)
    locations: Vec<TargetLocationRef>,
}

#[derive(Serialize, Clone)]
struct TargetLocationRef {
    name: String,
    playfield: u32,
    title: Option<String>,
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
/// (content/achievements; an `all` achievement whose parts are finds needs each of their
/// locations), the recorded dialogues and the portals' teleport windows.
fn targets() -> &'static Targets {
    static T: LazyLock<Targets> = LazyLock::new(|| {
        #[derive(Deserialize)]
        struct Ach {
            id: u32,
            #[serde(default)]
            parent: u32,
            name: String,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            text: Option<String>,
            #[serde(default)]
            place: Option<String>,
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
        let file: File = serde_json::from_value(lmoxide_content::compat::achievements_file(crate::gamedata::get())).expect("achievements");
        let pf_name = |pf: u32| crate::teleport::destination(pf).map(|d| d.name.clone());
        // "Pirate_Exploration_The Sirens" -> ("The Sirens", "Pirate Exploration")
        let split = |n: &str| match n.rsplit_once('_') {
            Some((cat, name)) => (name.to_string(), cat.replace('_', " ")),
            None => (n.to_string(), String::new()),
        };
        let finds: HashMap<u32, TargetLocationRef> = file
            .achievements
            .iter()
            .filter_map(|a| Some((a.id, TargetLocationRef { name: a.find.clone()?, playfield: a.playfield?, title: a.place.clone() })))
            .collect();
        let titles: HashMap<u32, &str> = file.achievements.iter().filter_map(|a| Some((a.id, a.title.as_deref()?))).collect();
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
            achievements.push(TargetAchievement {
                id: a.id,
                name,
                category,
                title: a.title.clone(),
                text: a.text.clone(),
                group: titles.get(&a.parent).map(|t| t.to_string()),
                locations,
            });
        }
        // the exploration areas, and the found places that aren't areas
        let mut locations: Vec<TargetLocation> = crate::encounter::explore_areas()
            .into_iter()
            .map(|(pf, name, pos)| TargetLocation { name: name.into(), title: None, playfield: pf, playfield_name: pf_name(pf), pos: Some(pos), achievements: Vec::new() })
            .collect();
        for f in finds.values() {
            if !locations.iter().any(|l| l.name == f.name && l.playfield == f.playfield) {
                locations.push(TargetLocation { name: f.name.clone(), title: None, playfield: f.playfield, playfield_name: pf_name(f.playfield), pos: None, achievements: Vec::new() });
            }
        }
        for l in &mut locations {
            l.achievements = achievements.iter().filter(|a| a.locations.iter().any(|r| r.name == l.name && r.playfield == l.playfield)).map(|a| a.id).collect();
            l.title = finds.values().find(|r| r.name == l.name && r.playfield == l.playfield).and_then(|r| r.title.clone());
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

/// The locations (playfield, name) achievement `id` (or internal name) needs, if it is a `find`
/// achievement or all of finds (trigger walls on play servers, walls.rs).
pub fn achievement_locations(id: &str) -> Option<Vec<(u32, String)>> {
    let a = targets().achievements.iter().find(|a| a.id.to_string() == id || a.name == id)?;
    Some(a.locations.iter().map(|l| (l.playfield, l.name.clone())).collect())
}

async fn trigger_targets() -> R<&'static Targets> {
    with(|_| Ok(Json(targets())))
}

// ------------------------------------------------------------------ playfields

#[derive(Serialize)]
struct PlayfieldView {
    id: u32,
    name: String,
    /// base content objects (live, demo, client and guessed spawns)
    base: u32,
    /// the set's placements (deletions of guesses left out)
    placements: u32,
    triggers: u32,
    scripts: u32,
    notes: u32,
}

/// The client's playfield names (the content tree's worlds).
fn playfield_names() -> &'static BTreeMap<u32, String> {
    static NAMES: LazyLock<BTreeMap<u32, String>> = LazyLock::new(crate::world_data::playfield_names);
    &NAMES
}

/// Every playfield the client names or the set has records for, with how much is in it.
async fn playfields() -> R<Vec<PlayfieldView>> {
    with(|st| {
        let names = playfield_names();
        let ids: std::collections::BTreeSet<u32> = names.keys().chain(st.bases.keys()).chain(st.notes.keys()).copied().collect();
        let count = |n: usize| n.min(u32::MAX as usize) as u32;
        Ok(Json(
            ids.into_iter()
                .map(|id| {
                    let n = st.notes.get(&id);
                    let surveyed = st.objects.values().filter(|o| o.playfield == id && o.category == Category::Surveyed).count();
                    PlayfieldView {
                        id,
                        name: names.get(&id).cloned().or_else(|| crate::teleport::destination(id).map(|d| d.name.clone())).unwrap_or_default(),
                        base: count(st.bases.get(&id).map_or(0, Vec::len)),
                        placements: count(surveyed),
                        triggers: count(st.walls.values().filter(|w| w.playfield == id).count()),
                        scripts: count(n.map_or(0, |n| n.scripts.len())),
                        notes: count(n.map_or(0, |n| n.notes.len())),
                    }
                })
                .collect(),
        ))
    })
}

// ------------------------------------------------------------------ smash achievement rules

/// A smash achievement (or a discover location, kind `find`) with its rule.
#[derive(Serialize)]
struct SmashEntry {
    #[serde(flatten)]
    achievement: crate::achievements::SmashAchievement,
    rule: Option<SmashRule>,
}

/// What `key` names: a smash achievement, or a discover location (`find:<playfield>:<name>`,
/// as trigger-targets names it).
fn smash_target(key: &str) -> Result<crate::achievements::SmashAchievement, Error> {
    if let Some(rest) = key.strip_prefix("find:") {
        let (pf, name) = rest.split_once(':').ok_or_else(|| Error::NotFound(format!("no location {key}")))?;
        let l = targets()
            .locations
            .iter()
            .find(|l| l.playfield.to_string() == pf && l.name == name)
            .ok_or_else(|| Error::NotFound(format!("no location {name} in playfield {pf}")))?;
        return Ok(crate::achievements::SmashAchievement {
            key: key.into(),
            ids: l.achievements.clone(),
            title: l.title.clone().unwrap_or_else(|| l.name.clone()),
            text: String::new(),
            group: l.playfield_name.clone(),
            kind: "find",
            what: None,
            matching: Some(l.name.clone()),
            n: 0,
            minifigures: 0,
        });
    }
    // an editor's own list of enemies, or a mission item enemies drop
    for (prefix, kind) in [(sv::CATEGORY_PREFIX, "category"), (sv::DROP_PREFIX, "drop")] {
        if let Some(name) = key.strip_prefix(prefix) {
            if name.trim().is_empty() {
                return Err(Error::Invalid(format!("{prefix}<name>: the name?")));
            }
            return Ok(crate::achievements::SmashAchievement {
                key: key.into(),
                ids: Vec::new(),
                title: name.into(),
                text: String::new(),
                group: None,
                kind,
                what: None,
                matching: None,
                n: 0,
                minifigures: 0,
            });
        }
    }
    crate::achievements::smash_achievements()
        .iter()
        .chain(crate::achievements::collect_achievements())
        .find(|a| a.key == key)
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("no smash achievement {key}")))
}

/// Every smash achievement (counters of kills or smashes, defeats) with its rule.
async fn smash_achievements() -> R<Vec<SmashEntry>> {
    with_work(|work, _| {
        let rules: HashMap<String, SmashRule> = lmoxide_content::views::rules(work).into_iter().map(|r| (r.id.clone(), r)).collect();
        Ok(Json(
            crate::achievements::smash_achievements()
                .iter()
                .chain(crate::achievements::collect_achievements())
                .map(|a| SmashEntry { rule: rules.get(&a.key).cloned(), achievement: a.clone() })
                .collect(),
        ))
    })
}

/// Every smash rule, discover locations' too.
async fn achievement_rules() -> R<Vec<SmashRule>> {
    with_work(|work, _| Ok(Json(lmoxide_content::views::rules(work))))
}

#[derive(Deserialize)]
struct SmashRuleBody {
    #[serde(default)]
    enemies: Vec<String>,
    /// Drops and collectibles: each enemy gives one only once (per player).
    #[serde(default)]
    once: bool,
    /// Drops: the item template.
    #[serde(default)]
    item: Option<u32>,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    feature: Option<String>,
}

/// Sets which enemies count toward a smash achievement (a discover location's rule has only
/// tags and a comment).
async fn save_smash_rule(Path(key): Path<String>, headers: HeaderMap, Json(b): Json<SmashRuleBody>) -> R<SmashEntry> {
    let achievement = smash_target(&key)?;
    with_work(|work, st| {
        let mut enemies: Vec<String> = Vec::new();
        for e in b.enemies.iter().map(|e| e.trim()).filter(|e| !e.is_empty()) {
            if !enemies.iter().any(|x| x.eq_ignore_ascii_case(e)) {
                enemies.push(e.to_string());
            }
        }
        if achievement.kind == "find" && !enemies.is_empty() {
            return Err(Error::Invalid("a discover location's rule has no enemies".into()));
        }
        let rule = SmashRule {
            id: key.clone(),
            enemies,
            once: b.once,
            item: b.item,
            comment: b.comment,
            tags: sv::tidy_tags(&b.tags),
            feature: checked_feature(&st.features, b.feature)?,
            author: author_of(&headers),
            updated: ids::now(),
        };
        let rule = lmoxide_content::views::put_rule(work, &rule, &author_of(&headers), ids::now())?;
        Ok(Json(SmashEntry { achievement, rule: Some(rule) }))
    })
}

/// An enemy or smashable name a defeat can carry (what smash rules and categories list).
#[derive(Serialize, Debug, Clone)]
struct EnemyName {
    name: String,
    /// enemy, smashable, boss or champion
    kind: &'static str,
    playfields: Vec<u32>,
    /// live, demo, client (the base content and catalog) or achievement (only achievements name it)
    source: &'static str,
}

/// Whether a spawn is a boss or a champion, by its health over a regular enemy's of its level
/// (npc.rs `BOSS_RANK` 7, `CHAMPION_RANK` 4).
fn enemy_kind(def: &SpawnDef) -> &'static str {
    match def.kind {
        Kind::Smashable => "smashable",
        _ => match def.health / npc::regular_life(def.level).max(1) {
            7.. => "boss",
            4.. => "champion",
            _ => "enemy",
        },
    }
}

fn enemy_names_of(bases: &BTreeMap<u32, Vec<SpawnDef>>, catalog: &[(Placeable, SpawnDef)]) -> Vec<EnemyName> {
    let mut out: BTreeMap<String, EnemyName> = BTreeMap::new();
    let mut add = |name: &str, kind: &'static str, playfields: &[u32], source: &'static str| {
        let name = name.trim();
        if name.is_empty() || name == "?" {
            return;
        }
        let e = out.entry(name.to_string()).or_insert_with(|| EnemyName { name: name.to_string(), kind, playfields: Vec::new(), source });
        for pf in playfields {
            if !e.playfields.contains(pf) {
                e.playfields.push(*pf);
            }
        }
        // the bigger kind wins (a boss somewhere is a boss)
        let rank = |k: &str| ["smashable", "enemy", "champion", "boss"].iter().position(|x| *x == k).unwrap_or(0);
        if rank(kind) > rank(e.kind) {
            e.kind = kind;
        }
        let order = |s: &str| ["live", "demo", "client", "achievement"].iter().position(|x| *x == s).unwrap_or(9);
        if order(source) < order(e.source) {
            e.source = source;
        }
    };
    let source_of = |d: &SpawnDef| match d.source.as_deref() {
        Some("live") => "live",
        Some("demo") => "demo",
        _ => "client",
    };
    for (pf, defs) in bases {
        for d in defs.iter().filter(|d| matches!(d.kind, Kind::Enemy | Kind::Smashable)) {
            add(&d.name, enemy_kind(d), &[*pf], source_of(d));
        }
    }
    for (p, d) in catalog.iter().filter(|(p, _)| matches!(p.kind, SKind::Enemy | SKind::Smashable)) {
        add(&p.name, enemy_kind(d), &[], source_of(d));
    }
    for (name, kind, pfs) in crate::achievements::achievement_enemies() {
        add(&name, kind, &pfs, "achievement");
    }
    for e in out.values_mut() {
        e.playfields.sort();
    }
    out.into_values().collect()
}

/// Every enemy and smashable name a defeat can carry: the base content of every playfield, the
/// catalog, the encounter champions and the defeat and target achievements' targets.
async fn enemy_names() -> R<Vec<EnemyName>> {
    with(|st| Ok(Json(enemy_names_of(&st.bases, &st.catalog))))
}

async fn delete_smash_rule(Path(key): Path<String>, headers: HeaderMap) -> Result<StatusCode, Error> {
    with_work(|work, _| {
        lmoxide_content::views::delete_rule(work, &key, &author_of(&headers), ids::now())?;
        Ok(StatusCode::NO_CONTENT)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_counted() {
        // a working copy with a tagged note and a script whose wave is tagged
        let dir = std::env::temp_dir().join(format!("lmoxide-survey-tags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("worlds/1200-hub/scripts")).unwrap();
        let put = |rel: &str, v: serde_json::Value| std::fs::write(dir.join(rel), serde_json::to_string(&v).unwrap()).unwrap();
        put("content.json", serde_json::json!({ "format": 1, "reference": "../data" }));
        put("worlds/1200-hub/world.json", serde_json::json!({ "id": "world/hub", "playfield": 1200 }));
        put("worlds/1200-hub/areas.json", serde_json::json!({ "areas": [{ "id": "area/1200/n", "role": "note", "note": {}, "tags": ["todo", "rough"] }] }));
        put("worlds/1200-hub/scripts/s.json", serde_json::json!({ "id": "script/1200/s", "waves": [{ "id": "w", "tags": ["rough"] }] }));
        let work = Work::open(&dir).unwrap();
        let uses = tag_uses(&work);
        assert_eq!(uses[0..2].iter().map(|u| (u.tag.as_str(), u.count, u.suggested)).collect::<Vec<_>>(), [("rough", 2, false), ("todo", 1, true)]);
        assert!(uses.iter().any(|u| u.tag == "approximate" && u.count == 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn smash_targets_and_playfield_names() {
        assert_eq!(playfield_names().get(&1100).map(String::as_str), Some("PvP - Builder Wars"));
        let town = smash_target("find:4200:Achievement_PirateTown").unwrap();
        assert_eq!((town.kind, town.title.as_str()), ("find", "Hidden Pirate Town"));
        assert!(town.ids.contains(&724));
        assert!(matches!(smash_target("find:4200:Nowhere"), Err(Error::NotFound(_))));
        assert_eq!(smash_target("2163").unwrap().title, "Novice Monster Smasher");
        assert!(matches!(smash_target("1"), Err(Error::NotFound(_))));
    }

    #[test]
    fn enemy_names_and_categories() {
        let names = enemy_names_of(&BTreeMap::new(), &[]);
        let uggluk = names.iter().find(|n| n.name == "Uggluk Gatekeeper").expect("Uggluk Gatekeeper");
        assert_eq!((uggluk.kind, uggluk.source), ("champion", "achievement"));
        assert!(!uggluk.playfields.is_empty(), "the side dungeons he appears in");
        assert!(names.iter().any(|n| n.name == "Skram Gatecrasher"), "a defeat target");
        assert!(!names.iter().any(|n| n.name == "Siege Command Room"), "places aren't enemies");
        let cat = smash_target("category:My skeletons").unwrap();
        assert_eq!((cat.kind, cat.title.as_str(), cat.n, cat.ids.len()), ("category", "My skeletons", 0, 0));
        assert_eq!(smash_target("drop:Mushroom").unwrap().kind, "drop");
        assert!(matches!(smash_target("category: "), Err(Error::Invalid(_))));
        let collar = smash_target("2431").unwrap();
        assert_eq!((collar.kind, collar.n), ("collect", 3));
    }

    #[test]
    fn trigger_targets() {
        let t = targets();
        let town = t.locations.iter().find(|l| l.playfield == 4200 && l.name == "Achievement_PirateTown").expect("Hidden Pirate Town");
        assert!(town.achievements.contains(&724), "{:?}", town.achievements);
        let a = t.achievements.iter().find(|a| a.id == 724).unwrap();
        assert_eq!((a.name.as_str(), a.category.as_str()), ("Find the Hidden Pirate Town", "Pirate Exploration"));
        assert_eq!(town.title.as_deref(), Some("Hidden Pirate Town"));
        // named as the game shows them
        let fiery = t.achievements.iter().find(|a| a.id == 1613).unwrap();
        assert_eq!((fiery.title.as_deref(), fiery.group.as_deref()), (Some("Fiery Explorer"), Some("Paleontologist")));
        assert_eq!(fiery.text.as_deref(), Some("Find the Volcano Summit in Dinosaur Rise."));
        let summit = t.locations.iter().find(|l| l.playfield == 6200 && l.name == "Achievement_Volcano").unwrap();
        assert_eq!((summit.title.as_deref(), summit.playfield_name.as_deref()), (Some("Volcano Summit"), Some("Dinosaur Rise")));
        // some need several locations
        assert!(t.achievements.iter().any(|a| a.locations.len() > 1));
        let d = t.dialogs.iter().find(|d| d.npc == "Video Game Guy").unwrap();
        assert!(!d.line.is_empty() && !d.line.contains('{') && !d.line.contains('['), "{}", d.line);
        // Pirate World's portal asks Yes/No
        assert!(t.portals.iter().any(|p| p.window == 74 && p.to == 4600 && p.plain), "{:?}", t.portals.iter().map(|p| (p.window, p.to)).collect::<Vec<_>>());
        assert!(t.portals.iter().any(|p| !p.plain && !p.playfields.is_empty()));
    }
}
