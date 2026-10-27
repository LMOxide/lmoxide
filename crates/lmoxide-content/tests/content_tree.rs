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
