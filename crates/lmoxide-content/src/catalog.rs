//! Client ids with names, for the world editor's pickers (the `x-lmoxide-client` catalogs):
//! from the client reference data in `data/` (looks, effect packages, stats) and from the
//! content records that carry client ids (items, spells, nanos, lore, playfields, texts). An
//! entry names the record it comes from, so the editor can link to it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::id::RecordKind;
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
    /// Its other names: a look's client name, every enemy, NPC, object and minifigure wearing
    /// it, what used it on live (searchable in the editor's pickers).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
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
            names: Vec::new(),
        });
    };
    // records of `kind` carrying the id at `pointer`
    let from_records =
        |kind: RecordKind,
         pointer: &str,
         add: &mut dyn FnMut(u64, String, Option<String>, Option<String>)| {
            for (k, e) in work
                .records()
                .filter(|(k, _)| k.id.kind == kind && k.feature == "live")
            {
                if let Some(id) = e.value.pointer(pointer).and_then(Value::as_u64) {
                    let detail = Some(kind.as_str().to_string());
                    add(id, name_of(&e.value), detail, Some(k.id.to_string()));
                }
            }
        };
    match name {
        "monster_data" => {
            // every name a look goes by: records first (the first one names it), then the
            // client's own name and what live used it for
            #[derive(Default)]
            struct Look {
                names: Vec<String>,
                kinds: Vec<String>,
                record: Option<String>,
                client: Option<String>,
            }
            fn named(looks: &mut BTreeMap<u64, Look>, id: u64, name: String, kind: String, record: Option<String>) {
                let l = looks.entry(id).or_default();
                if !name.is_empty() && !l.names.contains(&name) {
                    l.names.push(name);
                }
                if !kind.is_empty() && !l.kinds.contains(&kind) {
                    l.kinds.push(kind);
                }
                if l.record.is_none() {
                    l.record = record;
                }
            }
            let mut looks: BTreeMap<u64, Look> = BTreeMap::new();
            for kind in [RecordKind::Npc, RecordKind::Object, RecordKind::Minifig] {
                for (k, e) in work
                    .records()
                    .filter(|(k, _)| k.id.kind == kind && k.feature == "live")
                {
                    if let Some(id) = e.value.pointer("/render/monster_data").and_then(Value::as_u64) {
                        // an NPC record says enemy / npc / smashable
                        let what = e.value.get("kind").and_then(Value::as_str).unwrap_or(kind.as_str());
                        named(&mut looks, id, name_of(&e.value), what.to_string(), Some(k.id.to_string()));
                    }
                }
            }
            for m in read(&reference.join("monster_data.json"))?
                .get("monster_data")?
                .as_array()?
            {
                let Some(id) = m.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                for u in m.get("used_by").and_then(Value::as_array).into_iter().flatten() {
                    let kind = u.get("kind").and_then(Value::as_str).unwrap_or("");
                    named(&mut looks, id, name_of(u), kind.to_string(), None);
                }
                let client = name_of(m);
                looks.entry(id).or_default().client = Some(client).filter(|c| !c.is_empty());
            }
            for (id, l) in looks {
                let mut names = l.names.into_iter();
                let name = names.next().or_else(|| l.client.clone()).unwrap_or_else(|| format!("look {id}"));
                // "enemy · skeleton_axe · also Skeleton Warrior"
                let mut others: Vec<String> = names.collect();
                let client = l.client.filter(|c| *c != name);
                let mut detail = vec![if l.kinds.is_empty() { "look".to_string() } else { l.kinds.join("/") }];
                detail.extend(client.clone());
                if !others.is_empty() {
                    let shown: Vec<&str> = others.iter().take(4).map(String::as_str).collect();
                    detail.push(format!("also {}", shown.join(", ")));
                }
                others.extend(client);
                out.insert(id, Entry {
                    id,
                    name,
                    detail: Some(detail.join(" · ")),
                    record: l.record,
                    names: others,
                });
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
        "spell" => from_records(RecordKind::Skill, "/client/spell", &mut add),
        "nano" => from_records(RecordKind::Buff, "/nano", &mut add),
        "item" => {
            from_records(RecordKind::Item, "/template", &mut add);
            from_records(RecordKind::Minifig, "/template", &mut add);
            from_records(RecordKind::Object, "/render/template", &mut add);
        }
        "lore" => from_records(RecordKind::Achievement, "/lore", &mut add),
        "playfield" => from_records(RecordKind::World, "/playfield", &mut add),
        "window" => from_records(RecordKind::World, "/window", &mut add),
        "npc_template" => from_records(RecordKind::Npc, "/render/npc_template", &mut add),
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
