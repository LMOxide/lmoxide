//! Three-way merge of a record edited by two people at once, field by field: what only one side
//! changed is taken; what both changed differently is a conflict. Objects merge per field (all
//! the way down), anything else (numbers, strings, lists) is one value.

use serde_json::{Map, Value};

/// Fields every save sets anew; never conflicts.
const BOOKKEEPING: &[&str] = &["rev", "author", "updated"];

/// A field both sides changed differently. `path` is a JSON pointer (`/stats/health`); a side
/// that removed the field has `null`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Conflict {
    pub path: String,
    pub base: Value,
    pub ours: Value,
    pub theirs: Value,
}

/// Merges `ours` (an editor's version, edited from `base`) with `theirs` (what was saved since).
pub fn merge3(base: &Value, ours: &Value, theirs: &Value) -> Result<Value, Vec<Conflict>> {
    let mut conflicts = Vec::new();
    let out = merge_at(
        Some(base),
        Some(ours),
        Some(theirs),
        &mut String::new(),
        &mut conflicts,
    );
    if conflicts.is_empty() {
        Ok(out.unwrap_or(Value::Null))
    } else {
        Err(conflicts)
    }
}

fn merge_at(
    base: Option<&Value>,
    ours: Option<&Value>,
    theirs: Option<&Value>,
    path: &mut String,
    conflicts: &mut Vec<Conflict>,
) -> Option<Value> {
    if ours == theirs {
        return ours.cloned();
    }
    if ours == base {
        return theirs.cloned();
    }
    if theirs == base {
        return ours.cloned();
    }
    if let (Some(Value::Object(o)), Some(Value::Object(t))) = (ours, theirs) {
        let empty = Map::new();
        let b = base.and_then(Value::as_object).unwrap_or(&empty);
        let mut out = Map::new();
        // ours' field order, then fields only theirs has
        let keys = o
            .keys()
            .chain(t.keys().filter(|k| !o.contains_key(*k)))
            .chain(
                b.keys()
                    .filter(|k| !o.contains_key(*k) && !t.contains_key(*k)),
            );
        for k in keys.cloned().collect::<Vec<_>>() {
            if path.is_empty() && BOOKKEEPING.contains(&k.as_str()) {
                if let Some(v) = o.get(&k).or(t.get(&k)) {
                    out.insert(k, v.clone());
                }
                continue;
            }
            let len = path.len();
            path.push('/');
            path.push_str(&k.replace('~', "~0").replace('/', "~1"));
            if let Some(v) = merge_at(b.get(&k), o.get(&k), t.get(&k), path, conflicts) {
                out.insert(k, v);
            }
            path.truncate(len);
        }
        return Some(Value::Object(out));
    }
    conflicts.push(Conflict {
        path: path.clone(),
        base: base.cloned().unwrap_or(Value::Null),
        ours: ours.cloned().unwrap_or(Value::Null),
        theirs: theirs.cloned().unwrap_or(Value::Null),
    });
    ours.cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn takes_each_sides_changes() {
        let base =
            json!({ "id": "npc/a", "level": 3, "stats": { "health": 10, "power": 2 }, "rev": 4 });
        let ours =
            json!({ "id": "npc/a", "level": 5, "stats": { "health": 10, "power": 2 }, "rev": 4 });
        let theirs = json!({ "id": "npc/a", "level": 3, "stats": { "health": 12, "power": 2 }, "tags": ["x"], "rev": 5 });
        assert_eq!(
            merge3(&base, &ours, &theirs).unwrap(),
            json!({ "id": "npc/a", "level": 5, "stats": { "health": 12, "power": 2 }, "tags": ["x"], "rev": 4 })
        );
    }

    #[test]
    fn removals_merge_too() {
        let base = json!({ "a": 1, "b": 2 });
        assert_eq!(
            merge3(
                &base,
                &json!({ "a": 1 }),
                &json!({ "a": 1, "b": 2, "c": 3 })
            )
            .unwrap(),
            json!({ "a": 1, "c": 3 })
        );
    }

    #[test]
    fn same_field_changed_differently_conflicts() {
        let base = json!({ "pos": [1, 2, 3], "stats": { "health": 10 } });
        let ours = json!({ "pos": [1, 2, 4], "stats": { "health": 11 } });
        let theirs = json!({ "pos": [9, 2, 3], "stats": { "health": 11 } });
        let c = merge3(&base, &ours, &theirs).unwrap_err();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].path, "/pos");
        assert_eq!(
            (c[0].ours.clone(), c[0].theirs.clone()),
            (json!([1, 2, 4]), json!([9, 2, 3]))
        );
        // one side removed it, the other changed it
        let c = merge3(&json!({ "a": 1 }), &json!({}), &json!({ "a": 2 })).unwrap_err();
        assert_eq!((c[0].path.as_str(), c[0].ours.clone()), ("/a", Value::Null));
    }
}
