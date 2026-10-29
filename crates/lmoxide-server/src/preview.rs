//! Previews (docs/SERVER-ARCHITECTURE.md "Content versions and previews"): a world editor plays a
//! world as the editor's working copy has it now (its unpublished changes too), with gameplay on
//! (enemies move and fight, loot drops, trigger walls and scripts run), in a copy of their own.
//! The working copy's spawns, trigger walls, scripts and jump pads replace the published ones
//! there; definitions (templates, skills, loot tables) stay the published content's. The copy
//! is dropped once it has been empty a while (worlds.rs).

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use lmoxide_content::views;
use lmoxide_net::types::Id32;
use lmoxide_survey::TriggerWall;
use tracing::{info, warn};

use crate::{
    instance::{Mode, Pf},
    npc::SpawnDef,
};

/// What a preview copy runs instead of the published content.
pub struct Preview {
    /// Who asked for it.
    pub owner: Id32,
    pub residents: Vec<SpawnDef>,
    pub walls: Vec<TriggerWall>,
    pub scripts: Option<crate::scripts::Playfield>,
}

static PREVIEWS: LazyLock<Mutex<HashMap<Pf, Arc<Preview>>>> = LazyLock::new(Default::default);

/// The preview copy `pf` is, if it is one.
pub fn of(pf: Pf) -> Option<Arc<Preview>> {
    PREVIEWS.lock().unwrap().get(&pf).cloned()
}

/// A new preview copy of playfield `model` for `owner`, from the working copy as it is now, and
/// how many errors the working copy has (what has one is left out). Slow (it loads the working
/// copy): not on a game connection's task.
pub fn start(owner: Id32, model: u32) -> Result<(Pf, usize), String> {
    let dir = crate::editor::work(|w| w.dir().to_path_buf()).ok_or("this server has no world editor working copy")?;
    let (content, errors) = crate::gamedata::load_version(&dir);
    for e in errors.iter().take(5) {
        warn!(?owner, "preview: the working copy: {e}");
    }
    let residents: Vec<SpawnDef> =
        crate::npc::resident_list(crate::spawns::table_of(&content)).into_iter().filter(|(pf, _)| *pf == model).map(|(_, d)| d).collect();
    let (walls, scripts) = crate::editor::work(|work| {
        let walls = views::walls_of(work, model);
        let notes = views::notes_of(work, model);
        let spawns = crate::survey::wave_spawns(work, model);
        let scripts = (!notes.scripts.is_empty() || !spawns.is_empty())
            .then(|| crate::scripts::Playfield { scripts: notes.scripts, notes: notes.notes, spawns });
        (walls, scripts)
    })
    .unwrap_or_default();
    // gameplay on: the maxed mode's rules
    let pf = crate::instance::new_copy(model, Mode::Maxed).ok_or("every copy of that world is in use")?;
    info!(?owner, playfield = ?pf, residents = residents.len(), walls = walls.len(), scripts = scripts.is_some(), errors = errors.len(), "preview made");
    PREVIEWS.lock().unwrap().insert(pf, Arc::new(Preview { owner, residents, walls, scripts }));
    Ok((pf, errors.len()))
}

/// Forgets preview copy `pf` (dropped, worlds.rs).
pub fn end(pf: Pf) {
    PREVIEWS.lock().unwrap().remove(&pf);
}
