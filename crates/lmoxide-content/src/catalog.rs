//! Client ids with names, for the world editor's pickers (the `x-lmoxide-client` catalogs):
//! from the client reference data in `data/` (looks, effect packages, stats) and from the
//! content records that carry client ids (items, spells, nanos, lore, playfields, texts). An
//! entry names the record it comes from, so the editor can link to it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::work::Work;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The content record it belongs to (`skill/...` for a spell).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
}

/// The catalogs there are.
pub const CATALOGS: &[&str] = &[
    "monster_data",
    "effect_package",
    "spell",
    "nano",
    "item",
    "text",
    "lore",
    "playfield",
    "window",
    "npc_template",
    "stat",
];

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn name_of(v: &Value) -> String {
    ["name", "title"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
        .unwrap_or("")
        .to_string()
}

/// One catalog (`None`: no such catalog). `reference` is the client data folder (`data/`).
pub fn catalog(name: &str, work: &Work, reference: &Path) -> Option<Vec<Entry>> {
    let mut out: BTreeMap<u64, Entry> = BTreeMap::new();
    let mut add = |id: u64, name: String, detail: Option<String>, record: Option<String>| {
        out.entry(id).or_insert(Entry {
            id,
            name,
            detail,
            record,
        });
    };
    // records carrying the id at `pointer`, of kinds starting with `prefix`
    let from_records =
        |prefix: &str,
         pointer: &str,
         add: &mut dyn FnMut(u64, String, Option<String>, Option<String>)| {
            for (k, e) in work
                .records()
                .filter(|(k, _)| k.id.starts_with(prefix) && k.feature == "live")
            {
                if let Some(id) = e.value.pointer(pointer).and_then(Value::as_u64) {
                    let detail = k.id.split_once('/').map(|(kind, _)| kind.to_string());
                    add(id, name_of(&e.value), detail, Some(k.id.clone()));
                }
            }
        };
    match name {
        "monster_data" => {
            from_records("npc/", "/render/monster_data", &mut add);
            from_records("object/", "/render/monster_data", &mut add);
            for m in read(&reference.join("monster_data.json"))?
                .get("monster_data")?
                .as_array()?
            {
                if let Some(id) = m.get("id").and_then(Value::as_u64) {
                    add(id, name_of(m), Some("look".into()), None);
                }
            }
        }
        "effect_package" => {
            for e in read(&reference.join("effect_catalog.json"))?
                .get("effects")?
                .as_array()?
            {
                let Some(id) = e.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                let parts: Vec<&str> = e
                    .get("timelines")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.get("name").and_then(Value::as_str))
                    .take(3)
                    .collect();
                let used: Option<String> = e
                    .get("used_by")
                    .and_then(Value::as_array)
                    .and_then(|u| u.first())
                    .map(name_of);
                add(
                    id,
                    if parts.is_empty() {
                        format!("effect {id}")
                    } else {
                        parts.join(", ")
                    },
                    used.map(|u| format!("used by {u}")),
                    None,
                );
            }
        }
        "spell" => from_records("skill/", "/client/spell", &mut add),
        "nano" => from_records("buff/", "/nano", &mut add),
        "item" => {
            from_records("item/", "/template", &mut add);
            from_records("minifig/", "/template", &mut add);
            from_records("object/", "/render/template", &mut add);
        }
        "lore" => from_records("achievement/", "/lore", &mut add),
        "playfield" => from_records("world/", "/playfield", &mut add),
        "window" => from_records("world/", "/window", &mut add),
        "npc_template" => from_records("npc/", "/render/npc_template", &mut add),
        "text" => {
            let texts = read(&work.dir().join("texts.json"))?;
            for (section, detail) in [("names", "name"), ("texts", "text")] {
                for (id, t) in texts
                    .get(section)
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    if let (Ok(id), Some(t)) = (id.parse(), t.as_str()) {
                        // dialogue lines start with client markup: `{...}` and `[Actor]`
                        let plain: String = t
                            .lines()
                            .filter(|l| !l.trim_start().starts_with('{'))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let plain = plain.split(']').next_back().unwrap_or(&plain).trim();
                        add(
                            id,
                            plain.chars().take(120).collect(),
                            Some(detail.into()),
                            None,
                        );
                    }
                }
            }
        }
        "stat" => {
            for (id, n) in read(&reference.join("stats.json"))?.as_object()? {
                if let (Ok(id), Some(n)) = (id.parse(), n.as_str()) {
                    add(id, n.to_string(), None, None);
                }
            }
        }
        _ => return None,
    }
    Some(out.into_values().collect())
}
