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
