//! Buffs and effect packages the server shows on characters (spells' server-side parts).
//!
//! Layouts checked against the recorded original session. None of these records has a
//! protobuf header; the identity is the dynel the effect plays on.

use bytes::Bytes;
use lmoxide_net::{iir, types::Id32};

/// `MagicBuffIIR_c` mode 0 (add): u32 buff, u8 mode, ID32 caster, u32 counter, u32 counter
/// type, i32 duration (centiseconds), u8, u8, GameActionExecutorData (7 × u32, operator>>
/// 0x1b343ac), u32 casting level, u64 end time (67 bytes in the recorded session). The client
/// plays the buff's EffectPackage (stat 1109) on every add.
pub fn buff_add(target: Id32, buff: u32, caster: Id32, duration_cs: i32) -> Bytes {
    buff_add_counter(target, buff, caster, 0, duration_cs)
}

/// `buff_add` with a counter (a non-zero counter is applied right away, as UpdateBuff does).
pub fn buff_add_counter(target: Id32, buff: u32, caster: Id32, counter: u32, duration_cs: i32) -> Bytes {
    iir::record("MagicBuffIIR_c", target, false, |w| {
        let w = caster.write(w.u32(buff).u8(0)).u32(counter).u32(0).i32(duration_cs).u8(0).u8(1);
        let w = [1, 0, buff, 1, 0, 0, 0].iter().fold(w, |w, v| w.u32(*v));
        w.u32(1).u64(0)
    })
}

/// `MagicBuffIIR_c` add of a buff an NPC wears by itself (the champions' and bosses' rings), as
/// live sent the bosses' 161771: no caster, the second u8 0, GameActionExecutorData (1, 0, buff,
/// 0, 0, 0, 0) and the NPC's level as the casting level (byte-checked against the live capture
/// but for the end time, which is 0 here as in `buff_add`). Removed with `buff_remove` and a
/// zero caster.
pub fn npc_buff_add(target: Id32, buff: u32, duration_cs: i32, level: u32) -> Bytes {
    iir::record("MagicBuffIIR_c", target, false, |w| {
        let w = Id32::default().write(w.u32(buff).u8(0)).u32(0).u32(0).i32(duration_cs).u8(0).u8(0);
        let w = [1, 0, buff, 0, 0, 0, 0].iter().fold(w, |w, v| w.u32(*v));
        w.u32(level).u64(0)
    })
}

/// `MagicBuffIIR_c` mode 2 (update, ReadSubClass 0x139fdcc -> CoreMagicHolder::UpdateBuff): u32
/// buff, u8 2, ID32 caster, u32 counter, u32 counter type, u32 time left (centiseconds), u32
/// casting level, u32 (passed on to UpdateBuff). Used for buff counters (AddBuffCounter).
pub fn buff_update_counter(target: Id32, buff: u32, caster: Id32, counter: u32, time_left_cs: u32) -> Bytes {
    iir::record("MagicBuffIIR_c", target, false, |w| caster.write(w.u32(buff).u8(2)).u32(counter).u32(0).u32(time_left_cs).u32(1).u32(0))
}

/// `MagicBuffIIR_c` mode 1 (remove): u32 buff, u8 1, ID32 caster, u8 0.
pub fn buff_remove(target: Id32, buff: u32, caster: Id32) -> Bytes {
    iir::record("MagicBuffIIR_c", target, false, |w| caster.write(w.u32(buff).u8(1)).u8(0))
}

/// `ChainSpellIIR_t` (ReadSubClass 0x139f110, Activate 0x13a061e), identity the caster (its
/// MagicHolder casts, its EffectHolder plays the effect): ID32 the character the chain jumps to
/// (the spell is cast on it, the effect's target), ID32 the last chain target (named target
/// "lastChainTarget", the arc's other end: the caster on the first jump), u32 spell,
/// GameActionExecutorData (live: 1, 0, spell, 15, 0, a number rising by one per cast, 0), u32
/// hook value (× 1000 for the effect's "on_chain_jump" hook; live 100), u8 play effect: the
/// spell's EffectPackage (the Wizard's Lightning Blast jump 505506: 506679, the arc). Live's
/// 62 recorded jumps (enemies' chains) had the flag 0; nothing else sends the arc, so 1.
pub fn chain_jump(caster: Id32, to: Id32, last: Id32, spell: u32) -> Bytes {
    let seq = CHAIN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    iir::record("ChainSpellIIR_t", caster, false, |w| {
        let w = last.write(to.write(w)).u32(spell);
        let w = [1, 0, spell, 15, 0, seq, 0].iter().fold(w, |w, v| w.u32(*v));
        w.u32(100).u8(1)
    })
}

static CHAIN_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Buffs shown with `show_buff`, with the generation of their latest add.
static SHOWN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<(Id32, u32), u64>>> =
    std::sync::LazyLock::new(Default::default);
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Shows `buff` on `target` as the original server did: a buff still shown is removed first
/// (adding it again stacked another copy of its effect) and one with a duration is removed
/// when it is up (`MagicBuffIIR_c` mode 1, sent to `target`'s playfield; the client doesn't end
/// it by itself). `None` lasts until `forget_buff` + `buff_remove`. Returns the records to send
/// now.
pub fn show_buff(target: Id32, buff: u32, caster: Id32, duration: Option<std::time::Duration>) -> Vec<Bytes> {
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut records = Vec::new();
    if SHOWN.lock().unwrap().insert((target, buff), generation).is_some() {
        records.push(buff_remove(target, buff, caster));
    }
    let cs = duration.map_or(60_000, |d| (d.as_millis() / 10) as i32);
    records.push(buff_add(target, buff, caster, cs));
    if let Some(d) = duration
        && let Ok(rt) = tokio::runtime::Handle::try_current()
    {
        rt.spawn(async move {
            tokio::time::sleep(d).await;
            let mut shown = SHOWN.lock().unwrap();
            if shown.get(&(target, buff)) == Some(&generation) {
                shown.remove(&(target, buff));
                drop(shown);
                crate::world::send_playfield(target, true, crate::world::Out::Records(ended(target, buff, caster)));
            }
        });
    }
    records
}

/// Re-casts `buff` on `target` while it is still shown (a channel's tick re-casting its
/// self-buff, e.g. the Barbarian's Spin Slash): only its time is renewed (`MagicBuffIIR_c` mode
/// 2), since removing and adding it again restarts its effect and the animation it sets.
/// Returns `None` when it isn't shown (`show_buff` then).
pub fn refresh_buff(target: Id32, buff: u32, caster: Id32, duration: std::time::Duration) -> Option<Vec<Bytes>> {
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    {
        let mut shown = SHOWN.lock().unwrap();
        let g = shown.get_mut(&(target, buff))?;
        *g = generation;
    }
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(async move {
            tokio::time::sleep(duration).await;
            let mut shown = SHOWN.lock().unwrap();
            if shown.get(&(target, buff)) == Some(&generation) {
                shown.remove(&(target, buff));
                drop(shown);
                crate::world::send_playfield(target, true, crate::world::Out::Records(ended(target, buff, caster)));
            }
        });
    }
    Some(vec![buff_update_counter(target, buff, caster, 0, (duration.as_millis() / 10) as u32)])
}

/// Whether `buff` is shown on `target` (`show_buff`, not yet removed).
pub fn is_shown(target: Id32, buff: u32) -> bool {
    SHOWN.lock().unwrap().contains_key(&(target, buff))
}

/// Buffs whose look only their expiry list undoes, and the effect it starts. The client runs a
/// buff's list 37 when its own timer ends it (`MagicHolder::SlotDeactivateBuff`), not on the
/// server's removal (`DeactivateBuff` runs list 20), and the server's removal comes first:
/// Petrifying Gaze's stun turns the enemy to stone on add (its package 637013: desaturation 0,
/// `stunned`) and its list 37's StartEffect 637015 brings the colour back (0.33, 0.66, 1).
const END_EFFECTS: &[(u32, u32)] = &[(16_090, 637_015)];

/// The records for `buff` running out on `target`: its removal, and the effect that undoes its
/// look (`END_EFFECTS`).
fn ended(target: Id32, buff: u32, caster: Id32) -> Vec<Bytes> {
    let mut records = vec![buff_remove(target, buff, caster)];
    records.extend(END_EFFECTS.iter().filter(|(b, _)| *b == buff).map(|&(_, package)| effect_package(target, package)));
    records
}

/// `buff` is no longer shown on `target` (removed by other means).
pub fn forget_buff(target: Id32, buff: u32) {
    SHOWN.lock().unwrap().remove(&(target, buff));
}

/// Whether `buff` is shown on `target` (`show_buff`, until it ends or is forgotten).
pub fn has_buff(target: Id32, buff: u32) -> bool {
    SHOWN.lock().unwrap().contains_key(&(target, buff))
}

/// Forgets every buff shown on `target` (it left).
pub fn forget_buffs(target: Id32) {
    SHOWN.lock().unwrap().retain(|(t, _), _| *t != target);
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

/// An effect package of `owner` (the record's identity) with `target` as its named target: the
/// parts aimed at the target (`target="target"`) play on it (`!fx <id> @near`).
pub fn effect_package_on(owner: Id32, target: Id32, package: u32, hook: &str, instance: u32) -> Bytes {
    iir::record("EffectPackageIIR", owner, false, |w| {
        target.write(w).u32(package).string(hook).string("").f32(0.0).u8(0).u32(instance).f32(0.0).f32(0.0).f32(0.0).u32(0)
    })
}

/// An effect package played at a ground position, owned by `owner` (the record's identity), as
/// the original server sent ground effects: zero named target, the position, the owning
/// `spell` (0: none). Ends by itself or with `stop_effect_package` (same instance).
pub fn effect_package_at(owner: Id32, package: u32, at: [f32; 3]) -> Bytes {
    ground_effect(owner, package, at, 0, new_instance())
}

/// `effect_package_at` with the owning spell and instance.
pub fn ground_effect(owner: Id32, package: u32, at: [f32; 3], spell: u32, instance: u32) -> Bytes {
    iir::record("EffectPackageIIR", owner, false, |w| {
        Id32::new(0, 0).write(w).u32(package).string("").string("").f32(0.0).u8(0).u32(instance).f32(at[0]).f32(at[1]).f32(at[2]).u32(spell)
    })
}

/// `ground_effect` started at a code hook (the Witch Doctor's totem: "onhit" plays its mark and
/// drop on the ground).
pub fn ground_effect_hook(owner: Id32, package: u32, hook: &str, at: [f32; 3], instance: u32) -> Bytes {
    iir::record("EffectPackageIIR", owner, false, |w| {
        Id32::new(0, 0).write(w).u32(package).string(hook).string("").f32(0.0).u8(0).u32(instance).f32(at[0]).f32(at[1]).f32(at[2]).u32(0)
    })
}

/// `StopEffectPackageIIR` (owner): u32 package, u32 owning spell, u32 instance (recorded).
pub fn stop_effect_package(owner: Id32, package: u32, spell: u32, instance: u32) -> Bytes {
    iir::record("StopEffectPackageIIR", owner, false, |w| w.u32(package).u32(spell).u32(instance))
}

/// A fresh effect package instance id.
pub fn new_instance() -> u32 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Totemus Maximus (0xd69fc, level 4) getting the bosses' ring 161771 in the live capture
    /// (S->C :7106, record 25457); only the end time (the last 8 bytes) isn't reproduced.
    #[test]
    fn npc_ring_as_live() {
        let live = "6b87e3160000c350000d69fc00000277eb00000000000000000000000000000000000000170c00000000000100000000000277eb000000000000000000000000000000000000000400000000de37c108";
        let ours = npc_buff_add(Id32::new(50_000, 0xd69fc), 161_771, 5900, 4);
        let hex: String = ours.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex.len(), live.len());
        assert_eq!(hex[..hex.len() - 16], live[..live.len() - 16]);
        let removed: String = buff_remove(Id32::new(50_000, 0xd69fc), 161_771, Id32::default()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(removed, "6b87e3160000c350000d69fc00000277eb01000000000000000000");
    }
}
