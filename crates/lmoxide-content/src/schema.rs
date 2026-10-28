//! What the JSON Schemas say beyond JSON's types, for editors and checks: which kinds of record
//! a reference may name (`x-lmoxide-ref`, `x-lmoxide-ref-key` for map keys), which client
//! catalog a number comes from (`x-lmoxide-client`), units (`x-lmoxide-unit`), positions and
//! headings (`x-lmoxide-position`, `x-lmoxide-rotation`). On an array or a map they apply to its
//! items or values.
//!
//! [`index`] describes every kind for the world editor (`schemas/index.json`); [`Walker`] finds
//! the references and client ids in a record by its schema.

use std::collections::HashMap;

use serde_json::{Value, json};

/// How the world editor shows a kind.
pub struct KindInfo {
    pub kind: &'static str,
    /// One of them ("NPC").
    pub title: &'static str,
    /// The folder's name ("NPCs").
    pub plural: &'static str,
    /// Kept in a world's folder (`worlds/<world>/...`).
    pub per_world: bool,
    /// Fields worth showing in lists besides the name.
    pub list: &'static [&'static str],
}

pub const KIND_INFO: &[KindInfo] = &[
    KindInfo {
        kind: "world",
        title: "World",
        plural: "Worlds",
        per_world: true,
        list: &["playfield", "kind", "region"],
    },
    KindInfo {
        kind: "spawn",
        title: "Spawn",
        plural: "Spawns",
        per_world: true,
        list: &["kind", "template", "wave"],
    },
    KindInfo {
        kind: "portal",
        title: "Portal",
        plural: "Portals",
        per_world: true,
        list: &["to"],
    },
    KindInfo {
        kind: "bouncer",
        title: "Bouncer",
        plural: "Bouncers",
        per_world: true,
        list: &["template", "then"],
    },
    KindInfo {
        kind: "area",
        title: "Area",
        plural: "Areas",
        per_world: true,
        list: &["role"],
    },
    KindInfo {
        kind: "trigger",
        title: "Trigger",
        plural: "Triggers",
        per_world: true,
        list: &["on", "once", "look"],
    },
    KindInfo {
        kind: "puzzle",
        title: "Puzzle",
        plural: "Puzzles",
        per_world: true,
        list: &[],
    },
    KindInfo {
        kind: "script",
        title: "Script",
        plural: "Scripts",
        per_world: true,
        list: &["area", "start", "status"],
    },
    KindInfo {
        kind: "region",
        title: "Region",
        plural: "Regions",
        per_world: false,
        list: &["worlds"],
    },
    KindInfo {
        kind: "npc",
        title: "NPC",
        plural: "NPCs",
        per_world: false,
        list: &["kind", "level", "faction"],
    },
    KindInfo {
        kind: "faction",
        title: "Faction",
        plural: "Factions",
        per_world: false,
        list: &["enemies"],
    },
    KindInfo {
        kind: "object",
        title: "Object",
        plural: "Objects",
        per_world: false,
        list: &["kind", "level"],
    },
    KindInfo {
        kind: "skill",
        title: "Skill",
        plural: "Skills",
        per_world: false,
        list: &["minifigure", "ai_category", "cooldown"],
    },
    KindInfo {
        kind: "buff",
        title: "Buff",
        plural: "Buffs",
        per_world: false,
        list: &["duration", "stacking"],
    },
    KindInfo {
        kind: "loot",
        title: "Loot table",
        plural: "Loot tables",
        per_world: false,
        list: &["rolls"],
    },
    KindInfo {
        kind: "item",
        title: "Item",
        plural: "Items",
        per_world: false,
        list: &["kind", "template"],
    },
    KindInfo {
        kind: "minifig",
        title: "Minifigure",
        plural: "Minifigures",
        per_world: false,
        list: &["series", "template"],
    },
    KindInfo {
        kind: "dialogue",
        title: "Dialogue",
        plural: "Dialogues",
        per_world: false,
        list: &["npc", "world"],
    },
    KindInfo {
        kind: "achievement",
        title: "Achievement",
        plural: "Achievements",
        per_world: false,
        list: &["title", "kind", "lore"],
    },
];

/// The files that aren't records, with their schema.
pub const ROOT_FILES: &[(&str, &str, &str)] = &[
    (
        "content",
        "content.json",
        "The tree's format version and where the client reference data is",
    ),
    (
        "tunables",
        "tunables.json",
        "Numbers the game runs on that aren't any one record's (regeneration, drop rates, levels ...)",
    ),
    ("texts", "texts.json", "Client texts by id (dialogue lines)"),
    ("features", "features.json", "The features records can belong to beyond live, feature and other: titles, comments, what each was copied from"),
];

/// `schemas/index.json`: every kind with its id prefix, where its records are, its title,
/// description (the record's doc comment) and list fields; the root files.
pub fn index() -> Value {
    let schemas: HashMap<&str, Value> = crate::schemas().into_iter().collect();
    let path_of = |k: &str| {
        crate::KINDS
            .iter()
            .find(|(kind, _)| *kind == k)
            .map(|(_, p)| *p)
    };
    let kinds: Vec<Value> = KIND_INFO
        .iter()
        .map(|k| {
            json!({
                "kind": k.kind,
                "prefix": format!("{}/", k.kind),
                "path": path_of(k.kind),
                "title": k.title,
                "plural": k.plural,
                "description": schemas[k.kind].get("description"),
                "per_world": k.per_world,
                "list": k.list,
                "schema": format!("{}.schema.json", k.kind),
            })
        })
        .collect();
    let files: Vec<Value> = ROOT_FILES
        .iter()
        .map(|(name, path, about)| json!({ "name": name, "path": path, "description": about, "schema": format!("{name}.schema.json") }))
        .collect();
    json!({ "format": crate::FORMAT, "kinds": kinds, "files": files })
}

/// A reference found in a record: where (a JSON pointer), what it names, which kinds fit.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundRef {
    pub path: String,
    pub id: String,
    pub kinds: Vec<String>,
}

/// A client id found in a record.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundClient {
    pub path: String,
    pub id: u64,
    pub catalog: String,
}

#[derive(Debug, Default)]
pub struct Found {
    pub refs: Vec<FoundRef>,
    pub client: Vec<FoundClient>,
}

/// Walks records along their kind's schema.
pub struct Walker {
    schemas: HashMap<String, Value>,
}

impl Default for Walker {
    fn default() -> Self {
        Self::new()
    }
}

impl Walker {
    pub fn new() -> Self {
        Walker {
            schemas: crate::schemas()
                .into_iter()
                .map(|(k, s)| (k.to_string(), s))
                .collect(),
        }
    }

    pub fn schema(&self, kind: &str) -> Option<&Value> {
        self.schemas.get(kind)
    }

    /// The references and client ids in `value`, a record (or part of one: spawn overrides) of
    /// `kind`.
    pub fn find(&self, kind: &str, value: &Value) -> Found {
        let mut found = Found::default();
        if let Some(root) = self.schemas.get(kind) {
            walk(root, root, value, &mut String::new(), &mut found);
        }
        found
    }
}

fn strings(v: Option<&Value>) -> Option<Vec<String>> {
    Some(
        v?.as_array()?
            .iter()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect(),
    )
}

/// The `$defs` entry a `$ref` points at (`#/$defs/Shape`).
fn deref<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(r) => r
            .strip_prefix('#')
            .and_then(|p| root.pointer(p))
            .map_or(schema, |s| deref(root, s)),
        None => schema,
    }
}

fn type_fits(schema: &Value, value: &Value) -> bool {
    let name = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_u64() || n.is_i64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    let ok = |t: &str| t == name || (t == "number" && name == "integer");
    match schema.get("type") {
        Some(Value::String(t)) => ok(t),
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).any(ok),
        _ => true,
    }
}

/// Could `value` be of this (dereferenced) schema? Enough to choose among `oneOf`/`anyOf`
/// branches: types, `const`, and an object's required keys (an externally tagged enum's
/// variant name).
fn fits(root: &Value, schema: &Value, value: &Value) -> bool {
    let schema = deref(root, schema);
    if let Some(c) = schema.get("const") {
        return c == value;
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        return e.contains(value);
    }
    if let Some(branches) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        return branches.iter().any(|b| fits(root, b, value));
    }
    if !type_fits(schema, value) {
        return false;
    }
    match (value, schema.get("required").and_then(Value::as_array)) {
        (Value::Object(o), Some(req)) => req
            .iter()
            .filter_map(Value::as_str)
            .all(|k| o.contains_key(k)),
        _ => true,
    }
}

fn walk(root: &Value, schema: &Value, value: &Value, path: &mut String, found: &mut Found) {
    let schema = deref(root, schema);
    if let Some(kinds) = strings(schema.get("x-lmoxide-ref")) {
        refs_in(value, &kinds, path, found);
    }
    if let Some(catalog) = schema.get("x-lmoxide-client").and_then(Value::as_str) {
        client_in(value, catalog, path, found);
    }
    if let (Some(kinds), Value::Object(o)) = (strings(schema.get("x-lmoxide-ref-key")), value) {
        for k in o.keys() {
            found.refs.push(FoundRef {
                path: format!("{path}/{}", escape(k)),
                id: k.clone(),
                kinds: kinds.clone(),
            });
        }
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = schema.get(key).and_then(Value::as_array) {
            for b in branches
                .iter()
                .filter(|b| key == "allOf" || fits(root, b, value))
            {
                walk(root, b, value, path, found);
                if key == "oneOf" {
                    break;
                }
            }
        }
    }
    match value {
        Value::Object(o) => {
            let props = schema.get("properties").and_then(Value::as_object);
            let more = schema.get("additionalProperties").filter(|a| a.is_object());
            for (k, v) in o {
                if let Some(s) = props.and_then(|p| p.get(k)).or(more) {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&escape(k));
                    walk(root, s, v, path, found);
                    path.truncate(len);
                }
            }
        }
        Value::Array(a) => {
            let items = schema.get("items");
            let prefix = schema.get("prefixItems").and_then(Value::as_array);
            for (i, v) in a.iter().enumerate() {
                if let Some(s) = prefix.and_then(|p| p.get(i)).or(items) {
                    let len = path.len();
                    path.push_str(&format!("/{i}"));
                    walk(root, s, v, path, found);
                    path.truncate(len);
                }
            }
        }
        _ => {}
    }
}

/// Every string in `value` (it, its items, its values) is a reference.
fn refs_in(value: &Value, kinds: &[String], path: &str, found: &mut Found) {
    match value {
        Value::String(s) => found.refs.push(FoundRef {
            path: path.to_string(),
            id: s.clone(),
            kinds: kinds.to_vec(),
        }),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .for_each(|(i, v)| refs_in(v, kinds, &format!("{path}/{i}"), found)),
        Value::Object(o) => o
            .iter()
            .for_each(|(k, v)| refs_in(v, kinds, &format!("{path}/{}", escape(k)), found)),
        _ => {}
    }
}

fn client_in(value: &Value, catalog: &str, path: &str, found: &mut Found) {
    match value {
        Value::Number(n) => {
            if let Some(id) = n.as_u64() {
                found.client.push(FoundClient {
                    path: path.to_string(),
                    id,
                    catalog: catalog.to_string(),
                });
            }
        }
        Value::Array(a) => a
            .iter()
            .enumerate()
            .for_each(|(i, v)| client_in(v, catalog, &format!("{path}/{i}"), found)),
        _ => {}
    }
}

/// A JSON pointer's escaping of one key.
fn escape(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_refs_and_client_ids() {
        let w = Walker::new();
        let spawn = json!({
            "id": "spawn/4200/a", "kind": "enemy", "template": "npc/pirate", "pos": [1, 2, 3],
            "wave": { "script": "script/4200/x", "wave": "1" },
            "client": { "item_template": 77 }
        });
        let f = w.find("spawn", &spawn);
        let refs: Vec<_> = f
            .refs
            .iter()
            .map(|r| (r.path.as_str(), r.id.as_str(), r.kinds.join(",")))
            .collect();
        assert!(
            refs.contains(&("/template", "npc/pirate", "npc,object".into())),
            "{refs:?}"
        );
        assert!(
            refs.contains(&("/wave/script", "script/4200/x", "script".into())),
            "{refs:?}"
        );
        assert_eq!(
            f.client,
            vec![FoundClient {
                path: "/client/item_template".into(),
                id: 77,
                catalog: "item".into()
            }]
        );
    }

    #[test]
    fn walks_tagged_enums_and_map_keys() {
        let w = Walker::new();
        let trigger = json!({
            "id": "trigger/4200/t",
            "actions": [
                { "apply_buff": { "buff": "buff/burning", "target": { "placement": "spawn/4200/a" } } },
                { "effect": { "effect_package": 162435 } },
                { "give_item": { "item": "item/goblet" } }
            ]
        });
        let f = w.find("trigger", &trigger);
        let refs: Vec<_> = f
            .refs
            .iter()
            .map(|r| (r.path.as_str(), r.kinds.join(",")))
            .collect();
        assert_eq!(
            refs,
            [
                ("/actions/0/apply_buff/buff", "buff".to_string()),
                (
                    "/actions/0/apply_buff/target/placement",
                    "spawn,bouncer,trigger".to_string()
                ),
                ("/actions/2/give_item/item", "item".to_string()),
            ]
        );
        assert_eq!(f.client[0].catalog, "effect_package");
        let world = json!({ "id": "world/x", "playfield": 4200, "arrival_from": { "world/y": { "pos": [0, 0, 0] } } });
        let f = w.find("world", &world);
        assert_eq!(f.refs[0].id, "world/y");
        assert_eq!(f.refs[0].path, "/arrival_from/world~1y");
        assert_eq!(f.client[0].catalog, "playfield");
    }
}
