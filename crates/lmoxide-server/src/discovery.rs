//! Things players find in the worlds (achievements of kind `find`): the five collectibles
//! (`collectible_*` playfield simple dynels, spawned by scenery.rs) and each world's boombox
//! (the "boombox 1" marker). What made live count a find was server script; here a collectible
//! counts when the player uses it or stands next to it, a boombox when the player comes near
//! (docs/TUNABLES.md "Achievements").

use std::{collections::HashMap, sync::LazyLock};

use serde::Deserialize;

/// How close counts as found (m).
const COLLECTIBLE_RADIUS: f32 = 4.0;
const BOOMBOX_RADIUS: f32 = 6.0;

#[derive(Deserialize)]
struct Dynel {
    name: String,
    pos: [f32; 3],
    guid: u32,
}

pub struct Find {
    pub playfield: u32,
    pub name: String,
    pub pos: [f32; 3],
    pub guid: u32,
    radius: f32,
}

static FINDS: LazyLock<Vec<Find>> = LazyLock::new(|| {
    let worlds: HashMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!(*crate::world_data::WORLD_CONTENT)).expect("world content");
    let mut finds = Vec::new();
    for (pf, w) in worlds {
        let Ok(playfield) = pf.parse::<u32>() else { continue };
        let Some(list) = w.get("simple_dynels").and_then(|v| v.as_array()) else { continue };
        for d in list.iter().filter_map(|d| serde_json::from_value::<Dynel>(d.clone()).ok()) {
            let radius = match d.name.as_str() {
                n if is_collectible(n) => COLLECTIBLE_RADIUS,
                "boombox 1" => BOOMBOX_RADIUS,
                _ => continue,
            };
            finds.push(Find { playfield, name: d.name, pos: d.pos, guid: d.guid, radius });
        }
    }
    finds.sort_by_key(|f| (f.playfield, f.guid));
    finds
});

pub fn is_collectible(name: &str) -> bool {
    name.starts_with("collectible_")
}

/// What the player at `pos` has found there.
pub fn near(playfield: u32, pos: [f32; 3]) -> impl Iterator<Item = &'static Find> {
    FINDS.iter().filter(move |f| {
        let d = [0, 1, 2].map(|i| f.pos[i] - pos[i]);
        f.playfield == playfield && (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() <= f.radius
    })
}

/// The collectible with this simple dynel instance (used by the player).
pub fn collectible(guid: u32) -> Option<&'static Find> {
    FINDS.iter().find(|f| f.guid == guid && is_collectible(&f.name))
}

/// Test command targets: collectibles or boomboxes.
pub fn points(collectibles: bool) -> Vec<(String, u32, [f32; 3])> {
    FINDS.iter().filter(|f| is_collectible(&f.name) == collectibles).map(|f| (f.name.clone(), f.playfield, f.pos)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_load() {
        assert_eq!(points(true).len(), 5);
        assert_eq!(points(false).len(), 8);
        let eye = points(true).into_iter().find(|p| p.0 == "collectible_all_seeing_eye").unwrap();
        assert_eq!(near(eye.1, [eye.2[0] + 1.0, eye.2[1], eye.2[2]]).count(), 1);
        assert_eq!(near(eye.1, [eye.2[0] + 10.0, eye.2[1], eye.2[2]]).count(), 0);
    }
}
