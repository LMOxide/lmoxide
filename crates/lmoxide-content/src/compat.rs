//! Content in the shapes of the data files it replaced, for code that still reads those
//! shapes (the server's achievement tree, the world editor's achievement lists, the API).

use serde_json::{Value, json};

use crate::Content;

/// `achievements.json`: `achievements` (the client's lore tree, by lore id) and `playfields`
/// (the lore tree's names for playfields and areas).
pub fn achievements_file(c: &Content) -> Value {
    let lore = |id: &Option<String>| id.as_ref().and_then(|i| c.achievements.get(i)).and_then(|a| a.lore);
    let mut list: Vec<(u32, Value)> = c
        .achievements
        .values()
        .filter_map(|a| {
            let id = a.lore?;
            let mut v = serde_json::Map::new();
            v.insert("id".into(), json!(id));
            v.insert("parent".into(), json!(lore(&a.parent).or(a.parent_lore).unwrap_or(id)));
            let mut put = |k: &str, x: Value| {
                if !x.is_null() {
                    v.insert(k.into(), x);
                }
            };
            put("name", json!(a.client_name));
            put("title", json!(a.title));
            put("text", json!(a.text));
            put("points", json!(a.points));
            put("kind", json!(a.kind));
            put("playfield", json!(a.world.as_ref().and_then(|w| c.worlds.get(w)).map(|w| w.playfield)));
            put("find", json!(a.find));
            put("place", json!(a.place));
            put("stars", json!(a.stars));
            put("target", a.target.clone().unwrap_or(Value::Null));
            if !a.children.is_empty() {
                put("children", json!(a.children.iter().filter_map(|ch| c.achievements.get(ch)?.lore).collect::<Vec<_>>()));
            }
            put("counter", json!(a.counter));
            put("minifigure", json!(a.minifigure.as_ref().and_then(|m| c.minifigures.get(m)).map(|m| m.template)));
            put("n", json!(a.n));
            put("counts", a.counts.clone().unwrap_or(Value::Null));
            put("series", json!(a.series));
            if a.invisible {
                put("invisible", json!(true));
            }
            Some((id, Value::Object(v)))
        })
        .collect();
    list.sort_by_key(|(id, _)| *id);
    let mut playfields = serde_json::Map::new();
    for w in c.worlds.values() {
        playfields.insert(w.playfield.to_string(), json!(w.display_name.as_ref().unwrap_or(&w.name)));
    }
    if let Some(Value::Object(names)) = c.tunables.more.get("achievements.area_names") {
        playfields.extend(names.clone());
    }
    json!({ "achievements": list.into_iter().map(|(_, v)| v).collect::<Vec<_>>(), "playfields": playfields })
}

/// `dialogues.json`: the client's texts, answers, text keys and names (texts.json) and the
/// dialogues (content/dialogues), by their `order`.
pub fn dialogues_file(c: &Content) -> Value {
    let mut list: Vec<((u32, usize), Value)> = c
        .dialogues
        .iter()
        .map(|(id, d)| {
            let mut v = serde_json::Map::new();
            v.insert("texts".into(), json!(d.texts));
            let mut put = |k: &str, x: Value| {
                if !x.is_null() {
                    v.insert(k.into(), x);
                }
            };
            put("playfield", json!(d.world.as_ref().and_then(|w| c.worlds.get(w)).map(|w| w.playfield)));
            put("npc_name", json!(d.npc_name));
            put("npc", json!(d.npc));
            put("source", serde_json::to_value(d.source).unwrap_or(Value::Null));
            if d.seen_live {
                put("seen_live", json!(true));
            }
            put("start", d.start.clone().unwrap_or(Value::Null));
            put("pos", json!(d.pos));
            put("follows", d.follows.clone().unwrap_or(Value::Null));
            ((d.order, c.order.get(id).copied().unwrap_or(usize::MAX)), Value::Object(v))
        })
        .collect();
    list.sort_by_key(|(i, _)| *i);
    let t = &c.texts;
    json!({
        "texts": t.texts, "answers": t.answers, "keys": t.keys, "names": t.names,
        "dialogues": list.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
    })
}
