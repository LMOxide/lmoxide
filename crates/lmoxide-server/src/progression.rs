//! Character progression (`--progression`): player level and XP from defeated enemies, and
//! minifigure ranks bought with stars. Off by default: every character is at the level cap and
//! every minifigure fully ranked (the collection as it was chosen, docs/TUNABLES.md).
//!
//! - Player: XP (stat 52), Level (54), LastXP (57) and NextXP (350) drive the client's XP bar
//!   (XpBar.swf); the recorded level-1 player had 0 / 1 / 0 / 200. A playfield's level cap is
//!   the 5th trailing u32 of `PlayfieldFIIR_t` (the client's `SimpleChar_t::GetMaxLevelCap`:
//!   the smaller of it and TWK_MaxPlayerLevel; 0 = no cap): live sent 11 in Pirate World, 0 in
//!   the tutorial. At or above the cap the XP bar says so: the level stays and no XP is gained.
//! - Minifigures: rank 0..8 (item stat MinifigureLevel 3000058; live started minifigures at 0).
//!   The Collection's rank-up button sends `LevelUpMinifigure(template)`; it costs stars
//!   (token 100, "minifigure XP") up to the next MinifigureXPTables threshold (RDB 1000525,
//!   cumulative in item stat MinifigureXP 3000057) and needs the player level of lore tag
//!   466 + rank ("Tier N Upgrade Requirement": 3, 6, ... 27). Each rank unlocks one upgrade
//!   tier.
//!
//! The player XP curve, the XP per kill and the other worlds' caps were server data; the values
//! here are fitted to the recording where possible (docs/TUNABLES.md).

use std::sync::LazyLock;

use serde::Deserialize;

/// TWK_MaxPlayerLevel.
pub const MAX_PLAYER_LEVEL: u32 = crate::combat::LEVEL;
/// Highest minifigure rank (CarouselView.CanUpgrade refuses above 7).
pub const MAX_RANK: u32 = 8;

#[derive(Deserialize)]
struct Data {
    minifigure_xp: std::collections::BTreeMap<String, u64>,
    rank_player_level: Vec<u32>,
}

/// tunables.json `progression`.
static DATA: LazyLock<Data> = LazyLock::new(|| {
    let p = &crate::gamedata::get().tunables.progression;
    Data { minifigure_xp: p.minifigure_xp.iter().map(|(k, v)| (k.clone(), u64::from(*v))).collect(), rank_player_level: p.rank_player_level.clone() }
});

/// Total XP a character needs for `level + 1` (NextXP); the recorded level 1 needed 200.
/// Assumed curve: each level takes 100 × (level + 1) more.
pub fn next_xp(level: u32) -> u64 {
    let l = level as u64;
    50 * l * (l + 3)
}

/// Total XP at which `level` began (LastXP).
pub fn last_xp(level: u32) -> u64 {
    if level <= 1 { 0 } else { next_xp(level - 1) }
}

/// The level reached with `xp`, at most `cap`.
pub fn level_for(xp: u64, cap: u32) -> u32 {
    let mut level = 1;
    while level < cap && xp >= next_xp(level) {
        level += 1;
    }
    level
}

/// A playfield's level cap, 0 = none: its world's `level.cap`, else its super-region's
/// (tunables `progression.level_caps` by world `super_region`, RDB 1000052: micro-dungeons and
/// the hub get their world's); the tutorial has none. Pirate World's 11 and the tutorial's none
/// are recorded; the others assume each world caps two levels above where the next world's
/// proposed enemies start (TUNABLES).
pub fn level_cap(playfield: u32) -> u32 {
    let c = crate::gamedata::get();
    let Some(w) = c.world_by_playfield(playfield) else { return 0 };
    if let Some(cap) = w.level.as_ref().and_then(|l| l.cap) {
        return cap;
    }
    w.super_region.and_then(|r| c.tunables.progression.level_caps.get(&r.to_string()).copied()).unwrap_or(0)
}

/// The cap that applies on `playfield` (0 there = the game's maximum).
pub fn effective_cap(playfield: u32) -> u32 {
    match level_cap(playfield) {
        0 => MAX_PLAYER_LEVEL,
        cap => cap.min(MAX_PLAYER_LEVEL),
    }
}

/// XP for defeating an enemy of `level` (smashables give none, as recorded). Recorded: a
/// level-2 Musketeer 14 XP, the level-3 boss Captain Dreadleg 45; the tutorial's level-1
/// Scallywags gave 28.
pub fn kill_xp(level: u32, boss: bool, champion: bool) -> u64 {
    let level = level.max(1) as u64;
    if boss {
        15 * level
    } else if champion {
        14 * (level + 1)
    } else {
        7 * level
    }
}

/// Stars it takes to raise a minifigure from `rank` to `rank + 1`, or `None` at the top.
pub fn rank_cost(rank: u32) -> Option<u64> {
    if rank >= MAX_RANK {
        return None;
    }
    Some(rank_xp(rank + 1)? - rank_xp(rank).unwrap_or(0))
}

/// MinifigureXP (cumulative) at `rank`.
pub fn rank_xp(rank: u32) -> Option<u64> {
    if rank == 0 { Some(0) } else { DATA.minifigure_xp.get(&rank.to_string()).copied() }
}

/// Player level needed to rank a minifigure up from `rank`.
pub fn rank_player_level(rank: u32) -> u32 {
    DATA.rank_player_level.get(rank as usize).copied().unwrap_or(u32::MAX)
}

/// Upgrade bits a minifigure of `rank` may have: tier t (bits [0], [1,2], [3], [4,5], [6],
/// [7,8], [9], [10,11]) needs rank t + 1.
pub fn unlocked_upgrades(rank: u32) -> u32 {
    const TIERS: [u32; 8] = [0b1, 0b110, 0b1000, 0b11_0000, 0b100_0000, 0b1_1000_0000, 0b10_0000_0000, 0b1100_0000_0000];
    TIERS.iter().take(rank.min(MAX_RANK) as usize).fold(0, |m, t| m | t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_matches_recording() {
        assert_eq!(next_xp(1), 200);
        assert_eq!(last_xp(1), 0);
        assert_eq!(last_xp(2), 200);
        assert_eq!(level_for(199, 30), 1);
        assert_eq!(level_for(200, 30), 2);
        assert_eq!(level_for(1_000_000, 11), 11);
        assert_eq!(kill_xp(2, false, false), 14);
        assert_eq!(kill_xp(3, true, false), 45);
    }

    #[test]
    fn ranks() {
        assert_eq!(rank_cost(0), Some(50));
        assert_eq!(rank_cost(1), Some(60));
        assert_eq!(rank_cost(MAX_RANK), None);
        assert_eq!(rank_player_level(0), 3);
        assert_eq!(unlocked_upgrades(0), 0);
        assert_eq!(unlocked_upgrades(2), 0b111);
        assert_eq!(unlocked_upgrades(MAX_RANK), 0xfff);
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn caps_by_world_and_super_region() {
        assert_eq!(level_cap(4200), 11, "Isle of Yarr");
        assert_eq!(level_cap(1200), 11, "the hub is in the Pirate super-region");
        assert_eq!(level_cap(1643), 11, "Monkey Cave, a Pirate micro-dungeon");
        assert_eq!(level_cap(1600), 19, "a Space micro-dungeon");
        assert_eq!(level_cap(1300), 0, "the tutorial");
    }
}
