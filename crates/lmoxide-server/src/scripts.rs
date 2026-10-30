//! Script waves on play servers: the world editor's scripts (docs/SURVEY.md "Notes, scripts and
//! waves") that are approved and active here (content.rs keeps their placements apart instead of
//! spawning them with the playfield). A script is started by a trigger wall (walls.rs), by a
//! player entering the area its first wave watches, or by `!script start`; it spawns its first
//! wave (or the one named), then its later waves follow their triggers:
//!
//! - `after {seconds}`: that long after the previous wave started (the first wave: after the
//!   script started);
//! - `wave_cleared {wave}`: every spawn of that wave is defeated;
//! - `health_below {placement, percent}`: that placement (none: the script's strongest spawn)
//!   drops below that much of its health (defeated counts);
//! - `enter_area {note}`: a player is in that note's area (none: the script's own area);
//! - none / `other`: never by itself (a wall or `!script start` naming the wave starts it).
//!
//! Wave spawns are ordinary NPCs and enemies (they fight, drop loot and count for achievements)
//! that don't respawn. A spawn with a `count` brings that many: the first on its spot, the others
//! within its point's `spread`. In a Lost Creations level's copy only the spawns of the copy's
//! variant come, each wave after its points showed the spawn particle (lost_creations.rs
//! `telegraph`), and the level hears of each wave cleared and of the script's end. A script runs once at a time per playfield; it is over when every spawn is
//! defeated and no waiting wave can still start, and can then be started again. When the
//! playfield empties and its NPCs reset (npc.rs), its running scripts end and their spawns go.
//! The rules chosen here are in docs/TUNABLES.md "Script waves".
//!
//! Locks: content.rs's `STATE`, then `STATE` here, then `npc::NPCS`.

use crate::instance::Pf;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use lmoxide_net::types::Id32;
use lmoxide_survey::{Area, Note, Placement, Script, Trigger, Wave};
use tracing::info;

use crate::{
    npc::{self, SpawnDef},
    world,
};

/// How often triggers are checked.
const TICK: Duration = Duration::from_millis(500);
/// A point area counts as a circle this wide (m).
const POINT_RADIUS: f32 = 5.0;

/// A playfield's approved, active scripts, notes (their areas) and the placements of its waves
/// with what each spawns.
#[derive(Clone, Default)]
pub struct Playfield {
    pub scripts: Vec<Script>,
    pub notes: Vec<Note>,
    pub spawns: Vec<(Placement, SpawnDef)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum WaveState {
    /// Before the wave the script was started at: never spawns in this run.
    Skipped,
    Pending,
    Started(Instant),
}

struct Spawned {
    placement: String,
    wave: usize,
    id: Id32,
    /// its full health
    max: u32,
}

struct Run {
    started: Instant,
    waves: Vec<WaveState>,
    spawns: Vec<Spawned>,
    /// Waves started whose spawns are still to come (after the telegraph).
    coming: HashSet<usize>,
    /// Waves started and cleared.
    cleared: HashSet<usize>,
    /// why a wave doesn't start, logged once per run
    logged: HashSet<String>,
}

#[derive(Default)]
struct State {
    playfields: BTreeMap<u32, Playfield>,
    /// (playfield, script id) -> the running script
    /// the scripts running, per copy of a world
    runs: HashMap<(Pf, String), Run>,
    /// (player, copy, area key) inside an area at the last check
    inside: HashSet<(Id32, Pf, String)>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);
static WATCHING: AtomicBool = AtomicBool::new(false);

/// The approved, active scripts of every playfield (content.rs, whenever they may have changed).
/// A running script whose script or wave placements changed ends (its spawns go).
pub fn set(playfields: BTreeMap<u32, Playfield>) {
    let mut st = STATE.lock().unwrap();
    let State { playfields: old, runs, .. } = &mut *st;
    let of = |p: Option<&Playfield>, id: &str| -> (Option<Script>, Vec<Placement>) {
        let Some(p) = p else { return (None, Vec::new()) };
        let placements = p.spawns.iter().filter(|(x, _)| x.wave.as_ref().is_some_and(|w| w.script == id)).map(|(x, _)| x.clone()).collect();
        (p.scripts.iter().find(|s| s.id == id).cloned(), placements)
    };
    // (a preview's scripts stay as they were when it was made)
    let changed: Vec<(Pf, String)> = runs
        .keys()
        .filter(|(pf, id)| crate::preview::of(*pf).is_none() && of(old.get(&pf.model()), id) != of(playfields.get(&pf.model()), id))
        .cloned()
        .collect();
    for key in changed {
        if let Some(run) = runs.remove(&key) {
            info!(playfield = ?key.0, script = %key.1, "script: changed in the approved content; it ends, its spawns go");
            despawn(&run);
        }
    }
    let n: usize = playfields.values().map(|p| p.scripts.len()).sum();
    let summary: Vec<(u32, usize, usize)> = playfields.iter().filter(|(_, p)| !p.scripts.is_empty()).map(|(pf, p)| (*pf, p.scripts.len(), p.spawns.len())).collect();
    if summary != old.iter().filter(|(_, p)| !p.scripts.is_empty()).map(|(pf, p)| (*pf, p.scripts.len(), p.spawns.len())).collect::<Vec<_>>() {
        info!(scripts = n, playfields = ?summary.iter().map(|(pf, s, w)| format!("{pf:?}: {s} scripts, {w} wave placements")).collect::<Vec<_>>(), "world content: scripts");
    }
    *old = playfields;
    drop(st);
    if n > 0 && !WATCHING.swap(true, Ordering::SeqCst) {
        tokio::spawn(watch());
    }
}

/// Starts script `script` (id, name, part of the name or its number in `list`) on `pf`: wave
/// `wave` (id, name or number; none: the first). Running already: a wave named that is still
/// waiting starts now, else nothing happens. What happened, or why not.
pub fn start(pf: Pf, script: &str, wave: Option<&str>) -> Result<String, String> {
    let mut st = STATE.lock().unwrap();
    let State { playfields, runs, .. } = &mut *st;
    let preview = crate::preview::of(pf);
    let p = scripts_in(playfields, &preview, pf).filter(|p| !p.scripts.is_empty()).ok_or_else(|| format!("playfield {pf:?} has no approved, active scripts"))?;
    let s = find(&p.scripts, script).ok_or_else(|| format!("no script \"{script}\" on playfield {pf:?} (!script list)"))?;
    let i = match wave {
        Some(w) => wave_index(s, w).ok_or_else(|| format!("script {} has no wave \"{w}\"", s.name))?,
        None if s.waves.is_empty() => return Err(format!("script {} has no waves", s.name)),
        None => 0,
    };
    let key = (pf, s.id.clone());
    if let Some(run) = runs.get_mut(&key) {
        if wave.is_some() && run.waves[i] == WaveState::Pending {
            info!(playfield = ?pf, script = %s.name, wave = %wave_name(s, i), "script: running; the wave named starts now");
            let n = spawn_wave(run, pf, s, i, &p.spawns);
            return Ok(format!("{}: {} started ({n} spawns)", s.name, wave_name(s, i)));
        }
        info!(playfield = ?pf, script = %s.name, "script: running already; not started again");
        return Err(format!("{} is running already (!script stop ends it)", s.name));
    }
    let now = Instant::now();
    let mut run = Run {
        started: now,
        waves: (0..s.waves.len()).map(|j| if j < i { WaveState::Skipped } else { WaveState::Pending }).collect(),
        spawns: Vec::new(),
        coming: HashSet::new(),
        cleared: HashSet::new(),
        logged: HashSet::new(),
    };
    info!(playfield = ?pf, script = %s.name, id = %s.id, wave = %wave_name(s, i), "script starts");
    let msg = match (&s.waves[i].trigger, wave) {
        // the first wave's own delay still counts from the start
        (Some(Trigger::After { seconds }), None) => {
            info!(playfield = ?pf, script = %s.name, wave = %wave_name(s, i), seconds, "script: the first wave waits");
            format!("{} started: {} in {seconds} s", s.name, wave_name(s, i))
        }
        _ => {
            let n = spawn_wave(&mut run, pf, s, i, &p.spawns);
            format!("{} started: {} ({n} spawns)", s.name, wave_name(s, i))
        }
    };
    runs.insert(key, run);
    Ok(msg)
}

/// Ends the running scripts of `pf` (only `script`, if given); their spawns go. What ended.
pub fn stop(pf: Pf, script: Option<&str>) -> Vec<String> {
    let preview = crate::preview::of(pf);
    let mut st = STATE.lock().unwrap();
    let State { playfields, runs, .. } = &mut *st;
    let only = match script {
        Some(q) => match scripts_in(playfields, &preview, pf).and_then(|p| find(&p.scripts, q)) {
            Some(s) => Some(s.id.clone()),
            None => return vec![format!("no script \"{q}\" on playfield {pf:?}")],
        },
        None => None,
    };
    let keys: Vec<(Pf, String)> = runs.keys().filter(|(p, id)| *p == pf && only.as_ref().is_none_or(|o| o == id)).cloned().collect();
    if keys.is_empty() {
        return vec!["no script is running here".into()];
    }
    keys.into_iter()
        .filter_map(|key| {
            let run = runs.remove(&key)?;
            let name = script_name(scripts_in(playfields, &preview, pf), &key.1);
            info!(playfield = ?pf, script = %name, "script stopped; its spawns go");
            despawn(&run);
            Some(format!("{name} stopped ({} spawns removed)", run.spawns.len()))
        })
        .collect()
}

/// Playfield `pf` emptied and its NPCs were reset: its running scripts end, their spawns go.
pub fn playfield_emptied(pf: Pf) {
    let preview = crate::preview::of(pf);
    let mut st = STATE.lock().unwrap();
    let State { playfields, runs, inside } = &mut *st;
    inside.retain(|(_, p, _)| *p != pf);
    let keys: Vec<(Pf, String)> = runs.keys().filter(|(p, _)| *p == pf).cloned().collect();
    for key in keys {
        if let Some(run) = runs.remove(&key) {
            info!(playfield = ?pf, script = %script_name(scripts_in(playfields, &preview, pf), &key.1), "playfield empty: script reset, its spawns go");
            despawn(&run);
        }
    }
}

/// `!script list`: the scripts of `pf` with their waves and what each is doing.
pub fn list(pf: Pf) -> Vec<String> {
    let st = STATE.lock().unwrap();
    let preview = crate::preview::of(pf);
    let Some(p) = scripts_in(&st.playfields, &preview, pf).filter(|p| !p.scripts.is_empty()) else {
        let others: Vec<String> = st.playfields.iter().filter(|(_, p)| !p.scripts.is_empty()).map(|(pf, p)| format!("{pf:?} ({})", p.scripts.len())).collect();
        return vec![if others.is_empty() { "no approved, active scripts anywhere".into() } else { format!("no scripts here; scripts in playfields {}", others.join(", ")) }];
    };
    let mut out = Vec::new();
    for (n, s) in p.scripts.iter().enumerate() {
        let run = st.runs.get(&(pf, s.id.clone()));
        out.push(format!("{} {} [{}]: {}", n + 1, s.name, s.id, if run.is_some() { "running" } else { "idle" }));
        for (i, w) in s.waves.iter().enumerate() {
            let placements = p.spawns.iter().filter(|(x, _)| x.wave.as_ref().is_some_and(|r| r.script == s.id && r.wave == w.id)).count();
            let state = match run.map(|r| (r.waves[i], r)) {
                None => String::new(),
                Some((WaveState::Skipped, _)) => ", skipped".into(),
                Some((WaveState::Pending, _)) => ", waiting".into(),
                Some((WaveState::Started(_), r)) => {
                    let mine: Vec<&Spawned> = r.spawns.iter().filter(|x| x.wave == i).collect();
                    let alive = mine.iter().filter(|x| npc::health_of(x.id).is_some()).count();
                    format!(", spawned, {alive}/{} left", mine.len())
                }
            };
            out.push(format!("   wave {} {}: {}, {placements} placements{state}", i + 1, w.name, trigger_text(&w.trigger)));
        }
    }
    out
}

/// `!goto script n`: each script by its area's centre, else its first placement.
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    let st = STATE.lock().unwrap();
    let mut out = Vec::new();
    for (pf, p) in &st.playfields {
        for s in &p.scripts {
            let first = p.spawns.iter().find(|(x, _)| x.wave.as_ref().is_some_and(|w| w.script == s.id)).map(|(x, _)| x.pos);
            if let Some(pos) = s.area.as_ref().map(Area::center).or(first) {
                out.push((s.name.clone(), *pf, pos));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ running

async fn watch() {
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        // where the players are (outside the lock)
        let players: Vec<(Id32, Pf, [f32; 3])> = crate::sessions::players()
            .into_iter()
            .filter_map(|p| {
                let id = Id32::new(crate::territory::CHARACTER_TYPE, p.character_id);
                // (the world editor's copies run no play content)
                let (pf, _) = world::whereabouts(id).filter(|(pf, _)| !crate::instance::rules(*pf).editor)?;
                Some((id, pf, world::live_position(id)?))
            })
            .collect();
        check(&players);
    }
}

/// The scripts of copy `pf`: a preview's own (preview.rs, `preview` its `preview::of`), else its
/// world's.
fn scripts_in<'a>(playfields: &'a BTreeMap<u32, Playfield>, preview: &'a Option<Arc<crate::preview::Preview>>, pf: Pf) -> Option<&'a Playfield> {
    match preview {
        Some(p) => p.scripts.as_ref(),
        None => playfields.get(&pf.model()),
    }
}

/// One check: areas entered, waves whose trigger fired, scripts that are over.
fn check(players: &[(Id32, Pf, [f32; 3])]) {
    let mut st = STATE.lock().unwrap();
    let State { playfields, runs, inside } = &mut *st;
    // the copies players are in or scripts run in, with their previews
    let copies: std::collections::BTreeSet<Pf> = players.iter().map(|x| x.1).chain(runs.keys().map(|k| k.0)).collect();
    let previews: HashMap<Pf, Option<Arc<crate::preview::Preview>>> = copies.iter().map(|pf| (*pf, crate::preview::of(*pf))).collect();
    let def = |pf: Pf| scripts_in(playfields, &previews[&pf], pf);
    // who is in the areas the scripts watch
    let mut now_inside = HashSet::new();
    for &pf in &copies {
        let Some(p) = def(pf) else { continue };
        let here: Vec<&(Id32, Pf, [f32; 3])> = players.iter().filter(|x| x.1 == pf).collect();
        if here.is_empty() {
            continue;
        }
        for s in &p.scripts {
            for w in &s.waves {
                let Some(Trigger::EnterArea { note }) = &w.trigger else { continue };
                let Some((key, area)) = area_of(p, s, note.as_deref()) else { continue };
                for (player, copy, pos) in &here {
                    if contains(area, *pos) {
                        now_inside.insert((*player, *copy, key.clone()));
                    }
                }
            }
        }
    }
    let entered = |pf: Pf, key: &str| now_inside.iter().any(|(pl, p, k)| *p == pf && k == key && !inside.contains(&(*pl, pf, k.clone())));
    let occupied = |pf: Pf, key: &str| now_inside.iter().any(|(_, p, k)| *p == pf && k == key);

    // scripts started by entering their first wave's area (in the copy the player is in)
    let mut starts = Vec::new();
    for &pf in &copies {
        let Some(p) = def(pf) else { continue };
        {
            for s in &p.scripts {
                if runs.contains_key(&(pf, s.id.clone())) {
                    continue;
                }
                if let Some(Trigger::EnterArea { note }) = s.waves.first().and_then(|w| w.trigger.as_ref())
                    && let Some((key, _)) = area_of(p, s, note.as_deref())
                    && entered(pf, &key)
                {
                    starts.push((pf, s.id.clone()));
                }
            }
        }
    }

    // waves whose trigger fired, then the scripts that are over
    let now = Instant::now();
    let mut over = Vec::new();
    // (copy, script, a wave cleared / none: the script over with its spawns) for lost_creations.rs
    let mut events: Vec<(Pf, String, Option<usize>, Vec<Id32>)> = Vec::new();
    for ((pf, id), run) in runs.iter_mut() {
        let Some(p) = def(*pf) else { continue };
        let Some(s) = p.scripts.iter().find(|s| &s.id == id) else { continue };
        for i in 0..s.waves.len() {
            if run.waves[i] != WaveState::Pending {
                continue;
            }
            let fire = match &s.waves[i].trigger {
                Some(Trigger::After { seconds }) => after_ready(&run.waves, i, run.started, *seconds, now),
                Some(Trigger::WaveCleared { wave }) => match wave_index(s, wave) {
                    Some(j) => wave_down(run, j),
                    None => {
                        log_once(run, *pf, s, i, &format!("it waits for wave \"{wave}\", which this script doesn't have"));
                        false
                    }
                },
                Some(Trigger::HealthBelow { placement, percent }) => {
                    let target = match placement {
                        Some(pid) => run.spawns.iter().find(|x| &x.placement == pid),
                        None => run.spawns.iter().max_by_key(|x| x.max),
                    };
                    match target {
                        Some(t) => npc::health_of(t.id).is_none_or(|(h, max)| (h as f32) < percent / 100.0 * max as f32),
                        None => {
                            log_once(run, *pf, s, i, &format!("its placement {} isn't spawned (yet)", placement.as_deref().unwrap_or("(the strongest)")));
                            false
                        }
                    }
                }
                Some(Trigger::EnterArea { note }) => match area_of(p, s, note.as_deref()) {
                    Some((key, _)) => occupied(*pf, &key),
                    None => {
                        log_once(run, *pf, s, i, &format!("the area it waits for ({}) isn't there", note.as_deref().map_or("the script's own".into(), |n| format!("note {n}"))));
                        false
                    }
                },
                Some(Trigger::Other) | None => {
                    log_once(run, *pf, s, i, "it has no trigger the server runs (a wall or !script start naming it starts it)");
                    false
                }
            };
            if fire {
                info!(playfield = ?pf, script = %s.name, wave = %wave_name(s, i), trigger = %trigger_text(&s.waves[i].trigger), "script: wave triggered");
                spawn_wave(run, *pf, s, i, &p.spawns);
            }
        }
        for i in 0..s.waves.len() {
            if !run.cleared.contains(&i) && wave_down(run, i) {
                run.cleared.insert(i);
                info!(playfield = ?pf, script = %s.name, wave = %wave_name(s, i), "script: wave cleared");
                events.push((*pf, id.clone(), Some(i), Vec::new()));
            }
        }
        let all_down = run.coming.is_empty() && run.spawns.iter().all(|x| npc::health_of(x.id).is_none());
        if all_down && !(0..s.waves.len()).any(|i| can_still_start(run, p, s, i, s.waves.len())) {
            over.push((*pf, id.clone()));
        }
    }
    for key in over {
        if let Some(run) = runs.remove(&key) {
            info!(playfield = ?key.0, script = %script_name(def(key.0), &key.1)
, spawns = run.spawns.len(), "script over: every spawn defeated, no wave left to start");
            despawn(&run);
            let ids: Vec<Id32> = run.spawns.iter().map(|x| x.id).collect();
            events.push((key.0, key.1, None, ids));
        }
    }
    *inside = now_inside;
    drop(st);
    for (pf, id, wave, spawns) in events {
        match wave {
            Some(_) => crate::lost_creations::wave_cleared(pf, &id),
            None => crate::lost_creations::script_over(pf, &id, &spawns),
        }
    }
    for (pf, id) in starts {
        info!(playfield = ?pf, script = %id, "script: a player entered its first wave's area");
        if let Err(e) = start(pf, &id, None) {
            info!(playfield = ?pf, script = %id, "script not started: {e}");
        }
    }
}

/// Whether waiting wave `i` can still start by itself (else it doesn't keep the script running).
fn can_still_start(run: &Run, p: &Playfield, s: &Script, i: usize, depth: usize) -> bool {
    if run.waves[i] != WaveState::Pending || depth == 0 {
        return false;
    }
    match &s.waves[i].trigger {
        Some(Trigger::After { .. }) => true,
        Some(Trigger::EnterArea { note }) => area_of(p, s, note.as_deref()).is_some(),
        Some(Trigger::WaveCleared { wave }) => wave_index(s, wave).is_some_and(|j| can_still_start(run, p, s, j, depth - 1)),
        // its placement is in a wave still to come
        Some(Trigger::HealthBelow { placement: Some(pid), .. }) => p.spawns.iter().any(|(x, _)| {
            &x.id == pid
                && x.wave.as_ref().is_some_and(|w| w.script == s.id && wave_index(s, &w.wave).is_some_and(|j| j != i && can_still_start(run, p, s, j, depth - 1)))
        }),
        Some(Trigger::HealthBelow { placement: None, .. }) | Some(Trigger::Other) | None => false,
    }
}

/// `after {seconds}` of wave `i`: counted from the previous wave's start (from the script's start
/// for the first wave, or when the previous one was skipped); not while the previous one waits.
fn after_ready(waves: &[WaveState], i: usize, started: Instant, seconds: f32, now: Instant) -> bool {
    let from = match i.checked_sub(1).map(|j| waves[j]) {
        None | Some(WaveState::Skipped) => started,
        Some(WaveState::Started(t)) => t,
        Some(WaveState::Pending) => return false,
    };
    now.duration_since(from).as_secs_f32() >= seconds.max(0.0)
}

/// Whether started wave `i` is down: its spawns all came and are all defeated.
fn wave_down(run: &Run, i: usize) -> bool {
    matches!(run.waves[i], WaveState::Started(_)) && !run.coming.contains(&i) && run.spawns.iter().filter(|x| x.wave == i).all(|x| npc::health_of(x.id).is_none())
}

/// Wave `i`'s placements in copy `pf` (a Lost Creations level's: its variant's).
fn wave_placements<'a>(pf: Pf, s: &Script, i: usize, spawns: &'a [(Placement, SpawnDef)]) -> Vec<&'a (Placement, SpawnDef)> {
    let w = &s.waves[i];
    let variant = crate::lost_creations::variant(pf);
    spawns
        .iter()
        .filter(|(p, def)| {
            p.wave.as_ref().is_some_and(|r| r.script == s.id && r.wave == w.id) && (def.variants.is_empty() || variant.as_ref().is_some_and(|v| def.variants.contains(v)))
        })
        .collect()
}

/// Starts wave `i`: its placements spawn, at once or after the level's telegraph (the points
/// show the spawn particle first). How many spawn.
fn spawn_wave(run: &mut Run, pf: Pf, s: &Script, i: usize, spawns: &[(Placement, SpawnDef)]) -> usize {
    let started = Instant::now();
    run.waves[i] = WaveState::Started(started);
    let mine = wave_placements(pf, s, i, spawns);
    let n: usize = mine.iter().map(|(_, d)| d.count.max(1) as usize).sum();
    if n == 0 {
        info!(playfield = ?pf, script = %s.name, wave = %wave_name(s, i), "script: the wave has no placements here; nothing spawns");
        return 0;
    }
    let points: Vec<[f32; 3]> = mine.iter().map(|(p, _)| p.pos).collect();
    match crate::lost_creations::telegraph(pf, &points) {
        Some(delay) => {
            info!(playfield = ?pf, script = %s.name, wave = %wave_name(s, i), spawns = n, ?delay, "script: the wave's points light up");
            run.coming.insert(i);
            let (id, wave) = (s.id.clone(), i);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                arrive(pf, &id, wave, started);
            });
        }
        None => spawn_now(run, pf, s, i, spawns),
    }
    n
}

/// The telegraph of wave `wave` (started at `started`) is over: its spawns come, if the run
/// still goes.
fn arrive(pf: Pf, script: &str, wave: usize, started: Instant) {
    let preview = crate::preview::of(pf);
    let mut st = STATE.lock().unwrap();
    let State { playfields, runs, .. } = &mut *st;
    let Some(run) = runs.get_mut(&(pf, script.to_string())) else { return };
    if run.waves.get(wave) != Some(&WaveState::Started(started)) || !run.coming.remove(&wave) {
        return;
    }
    let Some(p) = scripts_in(playfields, &preview, pf) else { return };
    let Some(s) = p.scripts.iter().find(|s| s.id == script) else { return };
    crate::lost_creations::telegraph_done(pf);
    spawn_now(run, pf, s, wave, &p.spawns);
}

fn spawn_now(run: &mut Run, pf: Pf, s: &Script, i: usize, spawns: &[(Placement, SpawnDef)]) {
    let mut n = 0;
    for (p, def) in wave_placements(pf, s, i, spawns) {
        for k in 0..def.count.max(1) {
            let mut def = def.clone();
            if k > 0 {
                def.pos = around(def.pos, def.spread.unwrap_or(crate::gamedata::get().tunables.lost_creations.spawn_spread));
            }
            let id = npc::new_id(&def);
            let max = def.health;
            npc::spawn_once(id, def, pf);
            run.spawns.push(Spawned { placement: p.id.clone(), wave: i, id, max });
            n += 1;
        }
    }
    info!(playfield = ?pf, script = %s.name, wave = %wave_name(s, i), spawned = n, "script: wave spawns");
}

/// A random spot `within` m of `pos` (at least half that far, so they don't stand on each
/// other), at its height.
fn around(pos: [f32; 3], within: f32) -> [f32; 3] {
    let angle = crate::loot::random_below(3600) as f32 / 3600.0 * std::f32::consts::TAU;
    let d = within * (0.5 + crate::loot::random_below(1000) as f32 / 2000.0);
    [pos[0] + d * angle.sin(), pos[1], pos[2] + d * angle.cos()]
}

fn despawn(run: &Run) {
    for x in &run.spawns {
        npc::despawn(x.id);
    }
}

fn log_once(run: &mut Run, pf: Pf, s: &Script, i: usize, why: &str) {
    if run.logged.insert(s.waves[i].id.clone()) {
        info!(playfield = ?pf, script = %s.name, wave = %wave_name(s, i), "script: the wave doesn't start by itself: {why}");
    }
}

// ------------------------------------------------------------------ lookups

/// A script by id, name (no case), number in the list, or part of its name.
fn find<'a>(scripts: &'a [Script], q: &str) -> Option<&'a Script> {
    let l = q.to_ascii_lowercase();
    scripts
        .iter()
        .find(|s| s.id == q)
        .or_else(|| scripts.iter().find(|s| s.name.eq_ignore_ascii_case(q)))
        .or_else(|| q.parse::<usize>().ok().and_then(|n| scripts.get(n.wrapping_sub(1))))
        .or_else(|| scripts.iter().find(|s| !l.is_empty() && s.name.to_ascii_lowercase().contains(&l)))
}

/// A wave of `s` by id, name (no case) or number.
fn wave_index(s: &Script, q: &str) -> Option<usize> {
    s.waves
        .iter()
        .position(|w| w.id == q)
        .or_else(|| s.waves.iter().position(|w| w.name.eq_ignore_ascii_case(q)))
        .or_else(|| q.parse::<usize>().ok().filter(|n| (1..=s.waves.len()).contains(n)).map(|n| n - 1))
}

fn wave_name(s: &Script, i: usize) -> String {
    let w: &Wave = &s.waves[i];
    if w.name.is_empty() { format!("wave {}", i + 1) } else { w.name.clone() }
}

fn script_name(p: Option<&Playfield>, id: &str) -> String {
    p.and_then(|p| p.scripts.iter().find(|s| s.id == id)).map_or_else(|| id.to_string(), |s| s.name.clone())
}

fn trigger_text(t: &Option<Trigger>) -> String {
    match t {
        None => "no trigger".into(),
        Some(Trigger::After { seconds }) => format!("after {seconds} s"),
        Some(Trigger::HealthBelow { placement, percent }) => format!("{} below {percent}%", placement.as_deref().unwrap_or("the strongest spawn")),
        Some(Trigger::EnterArea { note }) => format!("entering {}", note.as_deref().unwrap_or("the script's area")),
        Some(Trigger::WaveCleared { wave }) => format!("wave {wave} cleared"),
        Some(Trigger::Other) => "other".into(),
    }
}

/// The area an `enter_area` trigger watches, with a key: note `note`'s, else the script's own.
fn area_of<'a>(p: &'a Playfield, s: &'a Script, note: Option<&str>) -> Option<(String, &'a Area)> {
    match note {
        Some(n) => p.notes.iter().find(|x| x.id == n).map(|x| (x.id.clone(), &x.area)),
        None => s.area.as_ref().map(|a| (format!("script:{}", s.id), a)),
    }
}

/// Whether `pos` is in `area`, seen from above (heights don't count).
fn contains(area: &Area, pos: [f32; 3]) -> bool {
    let d = |c: [f32; 3]| ((c[0] - pos[0]).powi(2) + (c[2] - pos[2]).powi(2)).sqrt();
    match area {
        Area::Point(c) => d(*c) <= POINT_RADIUS,
        Area::Circle { center, radius } => d(*center) <= *radius,
        Area::Polygon { points } if points.len() < 3 => false,
        Area::Polygon { points } => {
            // even-odd rule on (x, z)
            let (x, z) = (pos[0], pos[2]);
            let mut inside = false;
            let mut j = points.len() - 1;
            for i in 0..points.len() {
                let (xi, zi, xj, zj) = (points[i][0], points[i][2], points[j][0], points[j][2]);
                if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
                    inside = !inside;
                }
                j = i;
            }
            inside
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(waves: &[(&str, &str)]) -> Script {
        serde_json::from_value(serde_json::json!({
            "id": "s-1", "name": "Spawn Alien UFO",
            "waves": waves.iter().map(|(id, name)| serde_json::json!({"id": id, "name": name})).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn lookups() {
        let s = script(&[("w-a", "wave 1"), ("w-b", "Boss")]);
        let all = vec![s.clone()];
        assert_eq!(find(&all, "s-1").map(|s| &s.id), Some(&s.id));
        assert!(find(&all, "spawn alien ufo").is_some());
        assert!(find(&all, "ufo").is_some());
        assert!(find(&all, "1").is_some());
        assert!(find(&all, "2").is_none());
        assert_eq!(wave_index(&s, "w-b"), Some(1));
        assert_eq!(wave_index(&s, "boss"), Some(1));
        assert_eq!(wave_index(&s, "1"), Some(0));
        assert_eq!(wave_index(&s, "3"), None);
    }

    #[test]
    fn areas() {
        assert!(contains(&Area::Point([0.0, 0.0, 0.0]), [3.0, 50.0, 3.0]));
        assert!(!contains(&Area::Point([0.0, 0.0, 0.0]), [4.0, 0.0, 4.0]));
        assert!(contains(&Area::Circle { center: [10.0, 0.0, 10.0], radius: 2.0 }, [11.0, 0.0, 11.0]));
        let square = Area::Polygon { points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0], [0.0, 0.0, 10.0]] };
        assert!(contains(&square, [5.0, 0.0, 5.0]));
        assert!(!contains(&square, [15.0, 0.0, 5.0]));
    }

    #[test]
    fn extra_spawns_land_near_their_point() {
        for _ in 0..200 {
            let p = around([10.0, 5.0, 20.0], 1.5);
            let d = (p[0] - 10.0).hypot(p[2] - 20.0);
            assert!((0.749..=1.501).contains(&d), "{d}");
            assert_eq!(p[1], 5.0);
        }
    }

    #[test]
    fn after_counts_from_the_previous_wave() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(10);
        let waves = [WaveState::Started(t1), WaveState::Pending, WaveState::Pending];
        // the first wave from the script's start
        assert!(after_ready(&[WaveState::Pending], 0, t0, 5.0, t0 + Duration::from_secs(5)));
        assert!(!after_ready(&[WaveState::Pending], 0, t0, 5.0, t0 + Duration::from_secs(4)));
        // the second from the first's start
        assert!(!after_ready(&waves, 1, t0, 5.0, t1 + Duration::from_secs(4)));
        assert!(after_ready(&waves, 1, t0, 5.0, t1 + Duration::from_secs(5)));
        // the third waits while the second does
        assert!(!after_ready(&waves, 2, t0, 0.0, t1 + Duration::from_secs(100)));
        // after a skipped wave: from the script's start
        assert!(after_ready(&[WaveState::Skipped, WaveState::Pending], 1, t0, 5.0, t0 + Duration::from_secs(5)));
    }
}
