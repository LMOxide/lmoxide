//! The repository's content tree loads without problems and is in its canonical form (run
//! `cargo run -p lmoxide-content -- fmt` after editing by hand).

use std::path::Path;

use lmoxide_content::{Options, load, load::format_tree};

fn tree() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"))
}

#[test]
fn content_loads_without_problems() {
    let (content, problems) = load(tree(), &Options::default());
    let list: Vec<String> = problems.iter().take(20).map(|p| p.to_string()).collect();
    assert!(
        problems.is_empty(),
        "{} problems, first ones:\n{}",
        problems.len(),
        list.join("\n")
    );
    assert!(
        content.worlds.len() > 200
            && content.spawns.len() > 5000
            && content.minifigures.len() >= 133
    );
}

#[test]
fn content_is_formatted() {
    let changed = format_tree(tree(), true).expect("format check");
    assert!(
        changed.is_empty(),
        "not in canonical form (cargo run -p lmoxide-content -- fmt): {changed:?}"
    );
}

/// The achievement tree as achievements.json had it (the server, world editor and API read it in
/// that shape): the same entries, and the same playfield names but for Kingdom (5200), whose
/// display name is its teleport destination's ("Medieval Kingdom", as the achievements took it),
/// and the test asset playfield 1.
#[test]
fn achievements_same_as_the_old_file() {
    let (content, _) = load(tree(), &Options::default());
    let new = lmoxide_content::compat::achievements_file(&content);
    let old: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tree().join("../data/archive/achievements.json")).unwrap()).unwrap();
    let (o, n) = (old["achievements"].as_array().unwrap(), new["achievements"].as_array().unwrap());
    assert_eq!(o.len(), n.len());
    for (a, b) in o.iter().zip(n) {
        assert_eq!(a, b);
    }
    let mut names = old["playfields"].clone();
    names["5200"] = "Medieval Kingdom".into();
    // (playfield 1, a test asset the client's text table doesn't name, gets its playfield name)
    names["1"] = "xx_Asset_Smoke_Test".into();
    assert_eq!(names, new["playfields"]);
}

/// The dialogues as dialogues.json had them (knubot.rs reads that shape).
#[test]
fn dialogues_same_as_the_old_file() {
    let (content, _) = load(tree(), &Options::default());
    let new = lmoxide_content::compat::dialogues_file(&content);
    let old: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(tree().join("../data/archive/dialogues.json")).unwrap()).unwrap();
    for k in ["texts", "answers", "keys", "names"] {
        assert_eq!(old[k], new[k], "{k}");
    }
    // positions as the server reads them (f32)
    let norm = |d: &serde_json::Value| {
        let mut d = d.clone();
        if let Some(p) = d["pos"].as_array() {
            d["pos"] = serde_json::json!(p.iter().map(|x| format!("{}", x.as_f64().unwrap() as f32)).collect::<Vec<_>>());
        }
        d.to_string()
    };
    let a: Vec<String> = old["dialogues"].as_array().unwrap().iter().map(norm).collect();
    let b: Vec<String> = new["dialogues"].as_array().unwrap().iter().map(norm).collect();
    assert_eq!(a, b);
}

/// Every record of the tree as written (before features and `extends`), with its file.
fn raw_records() -> Vec<(std::path::PathBuf, serde_json::Value)> {
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') || p.ends_with("schemas") {
                continue;
            }
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "json") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(tree(), &mut files);
    let mut out = Vec::new();
    for f in files {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        match v {
            serde_json::Value::Object(ref o) if o.contains_key("id") => out.push((f, v)),
            serde_json::Value::Object(o) if f.parent() != Some(tree()) => {
                for (_, records) in o {
                    out.extend(records.as_array().unwrap().iter().map(|r| (f.clone(), r.clone())));
                }
            }
            _ => {}
        }
    }
    out
}

/// The schemas (what editors and the world editor's forms go by) accept every record and root
/// file, so they can't drift from what the loader reads.
#[test]
fn records_fit_their_schemas() {
    let schemas: std::collections::HashMap<&str, serde_json::Value> = lmoxide_content::schemas().into_iter().collect();
    let validators: std::collections::HashMap<&str, jsonschema::Validator> =
        schemas.iter().map(|(k, s)| (*k, jsonschema::validator_for(s).expect(k))).collect();
    let mut bad = Vec::new();
    let records = raw_records();
    assert!(records.len() > 10_000);
    for (file, r) in &records {
        let id = r["id"].as_str().unwrap();
        let kind = lmoxide_content::kind_of(id).expect(id);
        for e in validators[kind].iter_errors(r).take(1) {
            bad.push(format!("{} [{id}] {}: {e}", file.display(), e.instance_path()));
        }
    }
    for (name, file, _) in lmoxide_content::schema::ROOT_FILES {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(tree().join(file)).unwrap()).unwrap();
        for e in validators[name].iter_errors(&v).take(1) {
            bad.push(format!("{file} {}: {e}", e.instance_path()));
        }
    }
    assert!(bad.is_empty(), "{} records don't fit their schema:\n{}", bad.len(), bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
}

/// Every id the loader takes for a reference (a string that starts with a kind's prefix) is in a
/// field whose schema says which kinds it may name (`x-lmoxide-ref`), so the world editor offers
/// a picker for it and finds where a record is used.
#[test]
fn every_reference_is_annotated() {
    let walker = lmoxide_content::schema::Walker::new();
    // prose, and fields that hold keys or names rather than references
    const NOT_REFS: &[&str] = &["id", "name", "comment", "text", "title", "about", "rivals_about", "reference", "display_name", "prompt", "client_name"];
    fn ids(v: &serde_json::Value, path: &str, out: &mut Vec<(String, String)>) {
        match v {
            serde_json::Value::String(s) if lmoxide_content::kind_of(s).is_some() && !s.contains(' ') => out.push((path.to_string(), s.clone())),
            serde_json::Value::Array(a) => a.iter().enumerate().for_each(|(i, v)| ids(v, &format!("{path}/{i}"), out)),
            serde_json::Value::Object(o) => o
                .iter()
                .filter(|(k, _)| !NOT_REFS.contains(&k.as_str()))
                .for_each(|(k, v)| ids(v, &format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")), out)),
            _ => {}
        }
    }
    let mut missing = std::collections::BTreeMap::<String, (usize, String)>::new();
    for (_, r) in raw_records() {
        let id = r["id"].as_str().unwrap();
        let kind = lmoxide_content::kind_of(id).unwrap();
        let mut found: Vec<String> = walker.find(kind, &r).refs.into_iter().map(|f| f.path).collect();
        if let (Some(o), Some(t)) = (r.get("overrides"), r.get("template").and_then(|t| t.as_str()).and_then(lmoxide_content::kind_of)) {
            found.extend(walker.find(t, o).refs.into_iter().map(|f| format!("/overrides{}", f.path)));
        }
        let mut all = Vec::new();
        ids(&r, "", &mut all);
        for (path, value) in all {
            if !found.contains(&path) {
                // by field: the path without indexes
                let field: String = path.split('/').filter(|p| p.parse::<usize>().is_err()).collect::<Vec<_>>().join("/");
                let e = missing.entry(format!("{kind}: {field}")).or_insert((0, format!("{id}: {value}")));
                e.0 += 1;
            }
        }
    }
    assert!(missing.is_empty(), "references in fields without x-lmoxide-ref:\n{}", missing.iter().map(|(k, (n, eg))| format!("{k} ×{n} (e.g. {eg})")).collect::<Vec<_>>().join("\n"));
}

/// The world editor's working copy reads the whole tree, and knows where records are used.
#[test]
fn work_opens_the_tree() {
    let t = std::time::Instant::now();
    let work = lmoxide_content::work::Work::open(tree()).unwrap();
    assert!(work.notes.is_empty(), "{:?}", work.notes);
    assert_eq!(work.records().count(), raw_records().len());
    let used = work.used_by("npc/villager");
    assert!(used.iter().any(|u| u.key.id.to_string() == "spawn/1200/villager" && u.path == "/template"), "{used:?}");
    eprintln!("opened in {:?}", t.elapsed());
}

/// Every client catalog the schemas name has entries, from data/ or the records.
#[test]
fn catalogs_have_names() {
    let work = lmoxide_content::work::Work::open(tree()).unwrap();
    for name in lmoxide_content::catalog::CATALOGS {
        let list = lmoxide_content::catalog::catalog(name, &work, &tree().join("../data")).unwrap_or_default();
        assert!(list.len() > 3, "{name}: {} entries", list.len());
        eprintln!("{name}: {} entries, e.g. {:?}", list.len(), list[list.len() / 2]);
    }
    let lore = lmoxide_content::catalog::catalog("lore", &work, &tree().join("../data")).unwrap();
    assert!(lore.iter().any(|e| e.id == 4048 && e.name == "12 Days of Winter" && e.record.as_deref() == Some("achievement/12-days-of-winter")));
}

/// Every id in the tree is a well-formed record id (lmoxide_content::id).
#[test]
fn ids_parse() {
    let bad: Vec<String> = raw_records().into_iter().filter_map(|(_, r)| r["id"].as_str().filter(|id| id.parse::<lmoxide_content::id::RecordId>().is_err()).map(str::to_string)).collect();
    assert!(bad.is_empty(), "{bad:?}");
}
