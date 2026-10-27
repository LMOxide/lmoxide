//! Converting the data files the server read so far (`data/*.json`, the world editor's
//! published set) into the content tree. Run once per conversion: it writes every file anew,
//! so hand edits to the tree are lost when it runs again.

mod defs;
mod survey;
mod worlds;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::model::*;
use crate::record::Source;
use crate::tunables::{Manifest, Texts, Tunables};
use crate::{FORMAT, canon, slug};

/// Where the importer reads from.
pub struct Sources {
    /// `data/`.
    pub data: PathBuf,
    /// The world editor's published set (`var/survey-set/published`), if any.
    pub survey: Option<PathBuf>,
}

/// Everything read, before writing.
#[derive(Default)]
pub struct Tree {
    pub manifest: Option<Manifest>,
    pub tunables: Tunables,
    pub texts: Texts,
    pub worlds: Vec<World>,
    /// world id -> its folder name.
    pub world_dirs: BTreeMap<String, String>,
    /// world id -> records in its folder.
    pub spawns: BTreeMap<String, Vec<Spawn>>,
    pub portals: BTreeMap<String, Vec<Portal>>,
    pub bouncers: BTreeMap<String, Vec<Bouncer>>,
    pub areas: BTreeMap<String, Vec<Area>>,
    pub triggers: BTreeMap<String, Vec<Trigger>>,
    pub puzzles: BTreeMap<String, Vec<Puzzle>>,
    pub scripts: BTreeMap<String, Vec<Script>>,
    pub regions: Vec<Region>,
    pub npcs: Vec<Npc>,
    pub objects: Vec<Object>,
    pub skills: BTreeMap<u32, Skill>,
    pub buffs: Vec<Buff>,
    pub loot: Vec<LootTable>,
    pub items: BTreeMap<u32, Item>,
    pub minifigures: Vec<Minifigure>,
    pub dialogues: Vec<Dialogue>,
    pub achievements: Vec<Achievement>,
    /// What couldn't be placed, for the report.
    pub notes: Vec<String>,
}

/// Hands out readable ids: `<kind>/<slug of the name>`, `-2`, `-3` ... when taken.
#[derive(Default)]
pub struct Ids {
    taken: HashSet<String>,
}

impl Ids {
    pub fn new_id(&mut self, prefix: &str, name: &str) -> String {
        let base = format!("{prefix}/{}", slug(name));
        let mut id = base.clone();
        let mut n = 2;
        while !self.taken.insert(id.clone()) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// An id with a client number in it when the name alone is taken (`skill/aimed-shot-15351`).
    pub fn new_id_or_numbered(&mut self, prefix: &str, name: &str, number: u32) -> String {
        let base = format!("{prefix}/{}", slug(name));
        if self.taken.insert(base.clone()) {
            return base;
        }
        self.new_id(prefix, &format!("{name} {number}"))
    }
}

/// Shared state of an import: ids and the client-id -> record id maps.
#[derive(Default)]
pub struct Ctx {
    pub ids: Ids,
    /// playfield -> world id
    pub world: HashMap<u32, String>,
    /// client spell -> skill id
    pub skill: HashMap<u32, String>,
    /// client item template -> item id
    pub item: HashMap<u32, String>,
    /// minifigure template -> minifigure id
    pub minifig: HashMap<u32, String>,
    /// lore id -> achievement id
    pub achievement: HashMap<u32, String>,
    /// client ability (upgrade) -> buff id
    pub buff: HashMap<u32, String>,
    /// region name -> region id
    pub region: HashMap<String, String>,
    /// survey script id -> script id
    pub script: HashMap<String, String>,
    /// NPC and object templates made from placements
    pub templates: worlds::Templates,
}

pub fn read_json(path: &Path) -> Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Reads everything into a [`Tree`].
pub fn read(src: &Sources) -> Result<Tree> {
    let data = |name: &str| read_json(&src.data.join(name));
    let mut tree = Tree::default();
    let mut ctx = Ctx::default();
    tree.manifest = Some(Manifest {
        format: FORMAT,
        reference: "../data".into(),
        features: BTreeMap::new(),
    });

    worlds::worlds(&mut tree, &mut ctx, &data)?;
    defs::texts(&mut tree, &data)?;
    defs::skills(&mut tree, &mut ctx, &data)?;
    defs::items(&mut tree, &mut ctx, &data)?;
    defs::buffs(&mut tree, &mut ctx, &data)?;
    defs::minifigures(&mut tree, &mut ctx, &data)?;
    defs::loot_and_tunables(&mut tree, &mut ctx, &data)?;
    defs::dialogues(&mut tree, &mut ctx, &data)?;
    defs::achievements(&mut tree, &mut ctx, &data)?;
    worlds::world_records(&mut tree, &mut ctx, &data)?;
    if let Some(survey) = &src.survey {
        survey::survey(&mut tree, &mut ctx, survey, &data)?;
    }
    Ok(tree)
}

fn write_file<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, canon::to_string(value)?)
        .with_context(|| format!("writing {}", path.display()))
}

/// One file per record: `<dir>/<slug part of the id>.json`.
fn write_each<T: Serialize + crate::record::Record>(
    out: &Path,
    dir: &str,
    records: impl IntoIterator<Item = T>,
    count: &mut usize,
) -> Result<()> {
    for r in records {
        let name = r
            .id()
            .split_once('/')
            .map_or(r.id(), |(_, n)| n)
            .replace('/', "-");
        write_file(&out.join(dir).join(format!("{name}.json")), &r)?;
        *count += 1;
    }
    Ok(())
}

/// Writes the tree into `out` (emptied first).
pub fn write(tree: Tree, out: &Path) -> Result<usize> {
    if out.exists() {
        std::fs::remove_dir_all(out).with_context(|| format!("emptying {}", out.display()))?;
    }
    let mut n = 0;
    write_file(&out.join("content.json"), &tree.manifest)?;
    write_file(&out.join("tunables.json"), &tree.tunables)?;
    write_file(&out.join("texts.json"), &tree.texts)?;
    for w in tree.worlds {
        let dir = out.join("worlds").join(&tree.world_dirs[&w.id]);
        let id = w.id.clone();
        write_file(&dir.join("world.json"), &w)?;
        n += 1;
        macro_rules! section {
            ($field:ident, $file:literal) => {
                if let Some(records) = tree.$field.get(&id).filter(|r| !r.is_empty()) {
                    n += records.len();
                    let map: BTreeMap<&str, _> =
                        [(stringify!($field), records)].into_iter().collect();
                    write_file(&dir.join($file), &map)?;
                }
            };
        }
        section!(spawns, "spawns.json");
        section!(portals, "portals.json");
        section!(bouncers, "bouncers.json");
        section!(areas, "areas.json");
        section!(triggers, "triggers.json");
        section!(puzzles, "puzzles.json");
        if let Some(scripts) = tree.scripts.get(&id) {
            for s in scripts {
                let name = s.id.rsplit('/').next().unwrap_or(&s.id);
                write_file(&dir.join("scripts").join(format!("{name}.json")), s)?;
                n += 1;
            }
        }
    }
    write_each(out, "regions", tree.regions, &mut n)?;
    write_each(out, "npcs", tree.npcs, &mut n)?;
    write_each(out, "objects", tree.objects, &mut n)?;
    write_each(out, "skills", tree.skills.into_values(), &mut n)?;
    write_each(out, "buffs", tree.buffs, &mut n)?;
    write_each(out, "loot", tree.loot, &mut n)?;
    write_each(out, "items", tree.items.into_values(), &mut n)?;
    write_each(out, "minifigures", tree.minifigures, &mut n)?;
    write_each(out, "dialogues", tree.dialogues, &mut n)?;
    write_each(out, "achievements", tree.achievements, &mut n)?;
    for (kind, schema) in crate::schemas() {
        write_file(
            &out.join("schemas").join(format!("{kind}.schema.json")),
            &schema,
        )?;
    }
    Ok(n)
}

// Helpers for reading the loosely shaped data files.

pub(crate) fn u32_of(v: &Value) -> Option<u32> {
    v.as_u64().map(|n| n as u32)
}

pub(crate) fn f32_of(v: &Value) -> Option<f32> {
    v.as_f64().map(|n| n as f32)
}

pub(crate) fn str_of(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

pub(crate) fn vec3(v: &Value) -> Option<crate::Vec3> {
    let a = v.as_array()?;
    Some([f32_of(a.first()?)?, f32_of(a.get(1)?)?, f32_of(a.get(2)?)?])
}

pub(crate) fn rot(v: &Value) -> Option<crate::Rot> {
    match v {
        Value::Number(n) => Some(crate::Rot::Heading(n.as_f64()? as f32)),
        Value::Array(a) if a.len() == 4 => {
            let q: Vec<f32> = a.iter().filter_map(f32_of).collect();
            (q.len() == 4).then(|| crate::Rot::Quat([q[0], q[1], q[2], q[3]]))
        }
        _ => None,
    }
}

pub(crate) fn source_of(v: &Value) -> Option<Source> {
    Some(match v.as_str()? {
        "live" => Source::Live,
        "demo" => Source::Demo,
        "proposed" => Source::Proposed,
        "surveyed" => Source::Surveyed,
        _ => Source::Client,
    })
}

/// `v` read as a `T`; `None` when it's null. Anything else that doesn't fit stops the import
/// (nothing is dropped quietly).
pub(crate) fn strict<T: serde::de::DeserializeOwned>(v: &Value) -> Option<T> {
    if v.is_null() {
        return None;
    }
    match serde_json::from_value(v.clone()) {
        Ok(t) => Some(t),
        Err(e) => panic!(
            "import: can't read {v} as {}: {e}",
            std::any::type_name::<T>()
        ),
    }
}

/// The object's members, without `_comment`.
pub(crate) fn entries(v: &Value) -> impl Iterator<Item = (&String, &Value)> {
    v.as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| !k.starts_with('_'))
}
