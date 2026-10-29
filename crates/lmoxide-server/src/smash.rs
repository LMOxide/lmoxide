//! Smashed minifigures and their rebuild.
//!
//! When the active minifigure's health reaches 0 the server locks its item template:
//! `GameCharInterface::LockTemplate(u32 template, f32 seconds, u32 flags, f32 time since start)`
//! (the client keeps start = now - time since start, end = now + seconds). A locked minifigure is
//! "on cooldown" for the HUD and the Collection (smashed overlay, rebuild bar, can't be picked),
//! and the client switches to the next one (ItemHolder::DoRessurectLogicOnItemHolder). When the
//! time is up the server sends `UnlockTemplate(template)`: the client sets the minifigure's health
//! share (item stat 3000093) to 1 and lets it be picked again (ItemHolder::UnlockTemplate).
//!
//! How long a minifigure stays smashed is the server Lua's (scripts 973 / 3804 "minifigure
//! swapper smashed", tunables `smashed`): by the player's level (1-15 5 min, 16-20 20 min, 21-25
//! 60 min, 26+ 120 min), 60 s for members (a subscribed account or UseMemberSmashedTimer) and in
//! the tutorial and Pirate super-regions (1300, 4000: the hub too). Every player is a member
//! here (`everyone_member`), so it is 60 s. The swap also pushes the enemies around away
//! (465935) and makes the next minifigure unhurtable for 2 s (465943). The rebuild time is saved
//! with the character, so a relog or a server restart doesn't skip or restart it.

use std::time::Duration;

use bytes::Bytes;
use lmoxide_net::Writer;

/// The rebuild time of the members' timer (the client's default TWK_Fatigue_MemberCooldown
/// too), the longest a Lost Creations run's lock shows.
pub const REBUILD: Duration = Duration::from_secs(60);

/// How long a minifigure smashed by a character of `level` on playfield `model` stays smashed.
pub fn rebuild_time(level: u32, model: u32) -> Duration {
    let t = &crate::gamedata::get().tunables.smashed;
    let region = crate::gamedata::get().world_by_playfield(model).and_then(|w| w.super_region);
    if t.everyone_member || region.is_some_and(|r| t.free_super_regions.contains(&r)) {
        return t.free_rebuild.to_std();
    }
    t.rebuild_by_level
        .iter()
        .filter_map(|(l, d)| Some((l.parse::<u32>().ok()?, d)))
        .filter(|(l, _)| *l <= level.max(1))
        .max_by_key(|(l, _)| *l)
        .map_or(t.free_rebuild.to_std(), |(_, d)| d.to_std())
}

/// Lock flags for a smashed minifigure: any non-zero value makes the client restore full health
/// when it switches to a minifigure whose lock ran out (ItemHolder::SwitchMinifigureStats). The
/// recorded locks were spells (0x20 spell line, 0x80 cooldown); the minifigure value is assumed.
const LOCK_FLAGS: u32 = 0x1;

/// SubscriptionFlags (stat 512): bit 7 a subscribed account (CoreCharacter::
/// IsSubscribedAccountCharacter: member chests, member areas), bit 18 the member smash timer
/// (UseMemberSmashedTimer).
pub const STAT_SUBSCRIPTION_FLAGS: u32 = 512;
pub const MEMBER_FLAGS: u32 = 1 << 7 | 1 << 18;

/// Unix time in milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// LockTemplate arguments for a minifigure rebuilt at `until` (unix ms), of a rebuild that takes
/// `total`.
pub fn lock_args(template: u32, until: i64, total: Duration) -> Bytes {
    let total = total.as_secs_f32();
    let left = ((until - now_ms()) as f32 / 1000.0).clamp(0.0, total);
    Writer::new().u32(template).f32(left).u32(LOCK_FLAGS).f32(total - left).finish()
}

pub fn unlock_args(template: u32) -> Bytes {
    Writer::new().u32(template).finish()
}

/// How long until `until` (unix ms).
pub fn left(until: i64) -> Duration {
    Duration::from_millis((until - now_ms()).max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_rebuild_in_a_minute() {
        // everyone is a member: 60 s at any level, anywhere
        assert_eq!(rebuild_time(30, 2210), Duration::from_secs(60));
        assert_eq!(rebuild_time(1, 3200), Duration::from_secs(60));
    }

    #[test]
    fn free_players_by_level_unless_in_a_free_region() {
        let t = &crate::gamedata::get().tunables.smashed;
        assert!(t.everyone_member);
        let by = |level: u32| {
            t.rebuild_by_level.iter().filter_map(|(l, d)| Some((l.parse::<u32>().ok()?, d.to_std()))).filter(|(l, _)| *l <= level).max_by_key(|(l, _)| *l).unwrap().1
        };
        assert_eq!(by(15), Duration::from_secs(300));
        assert_eq!(by(16), Duration::from_secs(1200));
        assert_eq!(by(25), Duration::from_secs(3600));
        assert_eq!(by(30), Duration::from_secs(7200));
        assert!(t.free_super_regions.contains(&4000) && t.free_super_regions.contains(&1300));
        // the hub is in the Pirate super-region
        assert_eq!(crate::gamedata::get().world_by_playfield(1200).and_then(|w| w.super_region), Some(4000));
    }
}
