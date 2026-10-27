//! Minifigure data from the content tree (content/minifigures) in the shapes the collection,
//! aura and character code read: attack spells, upgrade abilities, aura tags, looks, names.

use std::collections::HashMap;
use std::sync::LazyLock;

use lmoxide_content::Minifigure;

/// Minifigures by item template.
static BY_TEMPLATE: LazyLock<HashMap<u32, &'static Minifigure>> =
    LazyLock::new(|| crate::gamedata::get().minifigures.values().map(|m| (m.template, m)).collect());

fn get(template: u32) -> Option<&'static Minifigure> {
    BY_TEMPLATE.get(&template).copied()
}

fn spell_of(skill: &str) -> Option<u32> {
    crate::gamedata::get().skills.get(skill)?.client.as_ref().map(|c| c.spell)
}

/// Every minifigure item template, in order.
pub fn all() -> Vec<u32> {
    let mut all: Vec<u32> = BY_TEMPLATE.keys().copied().collect();
    all.sort_unstable();
    all
}

/// The normal and special attacks' spells (the left and right click).
pub fn spells(template: u32) -> Vec<u32> {
    get(template).map_or_else(Vec::new, |m| [&m.normal.skill, &m.special.skill].into_iter().filter_map(|s| spell_of(s)).collect())
}

/// The upgrade abilities by upgrade bit: (bit, ability).
pub fn abilities(template: u32) -> Vec<(u32, u32)> {
    let buffs = &crate::gamedata::get().buffs;
    get(template).map_or_else(Vec::new, |m| m.upgrades.iter().filter_map(|u| Some((u.bit, buffs.get(&u.buff)?.nano?))).collect())
}

/// The four achievement aura tags (none: no aura).
pub fn aura_tags(template: u32) -> Option<[u32; 4]> {
    get(template)?.aura_tags.clone().try_into().ok()
}

/// The look (monster data) of a minifigure.
pub fn monster_data(template: u32) -> Option<u32> {
    get(template)?.render.monster_data
}

/// A minifigure by its name (case-insensitive).
pub fn by_name(name: &str) -> Option<u32> {
    let wanted = name.to_ascii_lowercase();
    BY_TEMPLATE.values().find(|m| m.name.to_ascii_lowercase() == wanted).map(|m| m.template)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(format!("{}/../../data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap()
    }

    #[test]
    fn same_as_the_old_files() {
        let spells = data("minifigure_spells.json");
        assert_eq!(all().len(), spells.as_object().unwrap().len());
        for (t, s) in spells.as_object().unwrap() {
            let t: u32 = t.parse().unwrap();
            assert_eq!(serde_json::json!(super::spells(t)), *s, "spells {t}");
            let old: Vec<(u32, u32)> = data("minifigure_abilities.json")[t.to_string()].as_array().unwrap().iter().map(|a| (a[0].as_u64().unwrap() as u32, a[1].as_u64().unwrap() as u32)).collect();
            assert_eq!(abilities(t), old, "abilities {t}");
            assert_eq!(monster_data(t), data("minifigure_monster_data.json")[t.to_string()].as_u64().map(|n| n as u32), "look {t}");
            let tags = &data("minifigure_auras.json")[t.to_string()];
            assert_eq!(aura_tags(t).map(|a| serde_json::json!(a)), (!tags.is_null()).then(|| tags.clone()), "auras {t}");
        }
        for m in data("minifig_parts.json")["minifigures"].as_array().unwrap() {
            if let Some(n) = m["name"].as_str() {
                assert_eq!(by_name(n), m["minifigure_template_id"].as_u64().map(|t| t as u32), "{n}");
            }
        }
        assert_eq!(by_name("dj"), Some(1922));
    }
}
