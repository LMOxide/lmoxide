//! Buffs and effect packages the server shows on characters (spells' server-side parts).
//!
//! Layouts checked against the recorded original session. None of these records has a
//! protobuf header; the identity is the dynel the effect plays on.

use bytes::Bytes;
use lmo_net::{iir, types::Id32};

/// `MagicBuffIIR_c` mode 0 (add): u32 buff, u8 mode, ID32 caster, u32 counter, u32 counter
/// type, i32 duration (centiseconds), u8, u8, GameActionExecutorData (7 × u32, operator>>
/// 0x1b343ac), u32 casting level, u64 end time (67 bytes in the recorded session). The client
/// plays the buff's EffectPackage (stat 1109) on every add.
pub fn buff_add(target: Id32, buff: u32, caster: Id32, duration_cs: i32) -> Bytes {
    iir::record("MagicBuffIIR_c", target, false, |w| {
        let w = caster.write(w.u32(buff).u8(0)).u32(0).u32(0).i32(duration_cs).u8(0).u8(1);
        let w = [1, 0, buff, 1, 0, 0, 0].iter().fold(w, |w, v| w.u32(*v));
        w.u32(1).u64(0)
    })
}

/// `MagicBuffIIR_c` mode 1 (remove): u32 buff, u8 1, ID32 caster, u8 0.
pub fn buff_remove(target: Id32, buff: u32, caster: Id32) -> Bytes {
    iir::record("MagicBuffIIR_c", target, false, |w| caster.write(w.u32(buff).u8(1)).u8(0))
}

/// `EffectPackageIIR`: ID32 named target, u32 package, string code hook, string stop trigger,
/// f32 hook value, u8, u32 instance id (a running package with the same id isn't restarted),
/// vec3 ground position (zero: on the identity dynel), u32 owner spell.
pub fn effect_package(target: Id32, package: u32) -> Bytes {
    effect_package_hook(target, package, "", "", 0.0, 0, new_instance())
}

/// An effect package started at a code hook (e.g. "charge", then "execute" with stop trigger
/// "charge" to move on): see `effect_package`. Records of one running package share `instance`.
pub fn effect_package_hook(target: Id32, package: u32, hook: &str, stop: &str, value: f32, flag: u8, instance: u32) -> Bytes {
    iir::record("EffectPackageIIR", target, false, |w| {
        target.write(w).u32(package).string(hook).string(stop).f32(value).u8(flag).u32(instance).f32(0.0).f32(0.0).f32(0.0).u32(0)
    })
}

/// A fresh effect package instance id.
pub fn new_instance() -> u32 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
