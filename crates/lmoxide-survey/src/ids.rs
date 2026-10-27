//! Ids. New records get `<prefix>-<author>-<random hex>`, so two surveyors never make the same
//! id. Base content (live, client and guessed objects) gets a stable id from what and where it
//! is, so a placement can name the guess it replaces without the extractor storing ids.

use std::hash::{BuildHasher, Hasher};

use crate::{Kind, Vec3};

/// FNV-1a, 64-bit: stable across runs and builds (std's hasher isn't).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3))
}

/// A base object's id: `g-` + a hash of its playfield, kind, look and position (to 10 cm).
pub fn base_id(playfield: u32, kind: Kind, monster_data: Option<u32>, template: Option<u32>, pos: Vec3) -> String {
    let mut key = format!("{playfield}|{}|{}|{}", kind.as_str(), monster_data.unwrap_or(0), template.unwrap_or(0));
    for v in pos {
        key.push_str(&format!("|{}", (v * 10.0).round() as i64));
    }
    format!("g-{:016x}", fnv1a(key.as_bytes()))
}

/// A new record's id, e.g. `p-hosttemple-9f3c…` (prefixes: `p` placement, `n` note, `s` script,
/// `w` wave).
pub fn new_id(prefix: &str, author: &str) -> String {
    let mut slug: String = author.chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect();
    slug.make_ascii_lowercase();
    if slug.is_empty() {
        slug.push_str("anon");
    }
    // RandomState is seeded randomly per instance; mixing in the time keeps ids made in a loop apart.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{prefix}-{slug}-{:016x}", h.finish())
}

/// Unix seconds now, for `updated`.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_ids_are_stable_to_ten_centimetres() {
        let a = base_id(4600, Kind::Enemy, Some(82410), None, [334.0, 48.75, 124.0]);
        assert_eq!(a, base_id(4600, Kind::Enemy, Some(82410), None, [334.02, 48.76, 124.04]));
        assert_ne!(a, base_id(4600, Kind::Enemy, Some(82410), None, [334.2, 48.75, 124.0]));
        assert_ne!(a, base_id(4600, Kind::Npc, Some(82410), None, [334.0, 48.75, 124.0]));
        assert_ne!(a, base_id(4610, Kind::Enemy, Some(82410), None, [334.0, 48.75, 124.0]));
        // a fixed value: changing the hash would orphan every saved replacement
        assert_eq!(base_id(1, Kind::Enemy, Some(2), None, [0.0; 3]), "g-d926d87bd9bc0996");
    }

    #[test]
    fn new_ids_differ() {
        let a = new_id("p", "Host Temple!");
        assert!(a.starts_with("p-hosttemple-"));
        assert_ne!(a, new_id("p", "Host Temple!"));
        assert!(new_id("n", "").starts_with("n-anon-"));
    }
}
