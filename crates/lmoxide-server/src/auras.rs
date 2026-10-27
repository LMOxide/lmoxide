//! Minifigure achievement auras: the glow around a player whose active minifigure's
//! achievements are done. The original server ran the Lua script of nano 540656 ("XX_Minifigure
//! Aura Applier", LuaScriptID 2152, shipped in the client's RDB 1010223) when a minifigure
//! became active and from achievement rewards: four achievement tags per minifigure
//! (data/minifigure_auras.json, tools/re/extract_auras.py) pick one of four aura buffs. The
//! auras are ordinary buffs (`MagicBuffIIR_c`, TimeExist 999999 cs) everyone on the playfield
//! sees.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_net::types::Id32;

use crate::{effects, world};

/// Tag 1 to 4 met: Disciple, Master, Grandmaster, Superachiever.
const DISCIPLE: u32 = 540_565;
const MASTER: u32 = 540_567;
const GRANDMASTER: u32 = 540_568;
const SUPERACHIEVER: u32 = 540_569;
/// The auras' TimeExist (999999 cs, as the recorded buffs' durations were their TimeExist).
const AURA_TIME: Duration = Duration::from_millis(9_999_990);

/// The minifigures' aura tags (content/minifigures `aura_tags`).
static TAGS: LazyLock<HashMap<u32, [u32; 4]>> =
    LazyLock::new(|| crate::minifigs::all().into_iter().filter_map(|t| Some((t, crate::minifigs::aura_tags(t)?))).collect());

/// The aura each player shows.
static SHOWN: LazyLock<Mutex<HashMap<Id32, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The four aura tags of `template` (empty: it has no aura).
pub fn tags(template: u32) -> Vec<u32> {
    TAGS.get(&template).map_or_else(Vec::new, |t| t.to_vec())
}

/// The aura of `template` for a player with achievement `tags`, as script 2152 picks it.
pub fn tier(template: u32, tags: &BTreeMap<u32, i64>) -> Option<u32> {
    let [t1, t2, t3, t4] = *TAGS.get(&template)?;
    let has = |t: u32| tags.contains_key(&t);
    match (has(t1), has(t2), has(t3), has(t4)) {
        (true, true, true, true) => Some(SUPERACHIEVER),
        (true, true, true, _) => Some(GRANDMASTER),
        (true, true, _, _) => Some(MASTER),
        (true, _, _, _) => Some(DISCIPLE),
        _ => None,
    }
}

/// Shows `player`'s aura for its active minifigure `template` (or takes it off), to everyone on
/// the playfield. `again`: show it even if unchanged (others may have just rebuilt the
/// character).
pub fn apply(player: Id32, template: u32, tags: &BTreeMap<u32, i64>, again: bool) {
    let aura = tier(template, tags);
    let before = {
        let mut shown = SHOWN.lock().unwrap();
        let before = shown.get(&player).copied();
        match aura {
            Some(a) => shown.insert(player, a),
            None => shown.remove(&player),
        };
        before
    };
    if before == aura && !again {
        return;
    }
    let mut records = Vec::new();
    if let Some(old) = before.filter(|old| Some(*old) != aura) {
        effects::forget_buff(player, old);
        records.push(effects::buff_remove(player, old, player));
    }
    if let Some(a) = aura {
        records.extend(effects::show_buff(player, a, player, Some(AURA_TIME)));
    }
    if !records.is_empty() {
        world::send_playfield(player, true, world::Out::Records(records));
    }
}

/// The records that show `player`'s aura to someone who just got its character.
pub fn records(player: Id32) -> Vec<Bytes> {
    let Some(aura) = SHOWN.lock().unwrap().get(&player).copied() else { return Vec::new() };
    vec![effects::buff_remove(player, aura, player), effects::buff_add(player, aura, player, (AURA_TIME.as_millis() / 10) as i32)]
}

/// `player` left.
pub fn left(player: Id32) {
    SHOWN.lock().unwrap().remove(&player);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers() {
        // Roman Commander: {3375, 3328, 3642, 3645}
        let tags = |t: &[u32]| t.iter().map(|&t| (t, 0)).collect::<BTreeMap<u32, i64>>();
        assert_eq!(tier(317609, &tags(&[])), None);
        assert_eq!(tier(317609, &tags(&[3375])), Some(DISCIPLE));
        assert_eq!(tier(317609, &tags(&[3375, 3328])), Some(MASTER));
        assert_eq!(tier(317609, &tags(&[3375, 3328, 3642])), Some(GRANDMASTER));
        assert_eq!(tier(317609, &tags(&[3375, 3328, 3642, 3645])), Some(SUPERACHIEVER));
        assert_eq!(tier(317609, &tags(&[3328, 3642, 3645])), None);
    }
}
