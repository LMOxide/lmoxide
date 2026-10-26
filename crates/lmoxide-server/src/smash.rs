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
//! How long a minifigure stays smashed was a server tweak: TWK_Fatigue_MemberCooldown (members,
//! SubscriptionFlags bit 18 "UseMemberSmashedTimer") or TWK_Fatigue_Cooldown; the client's
//! defaults are 60 s and 14400 s. Every player is a member here. The rebuild time is saved with
//! the character, so a relog or a server restart doesn't skip or restart it.

use std::time::Duration;

use bytes::Bytes;
use lmoxide_net::Writer;

/// TWK_Fatigue_MemberCooldown (the client's default).
pub const REBUILD: Duration = Duration::from_secs(60);
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

/// LockTemplate arguments for a minifigure rebuilt at `until` (unix ms).
pub fn lock_args(template: u32, until: i64) -> Bytes {
    let total = REBUILD.as_secs_f32();
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
