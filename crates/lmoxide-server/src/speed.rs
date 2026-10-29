//! A character's movement speed factor (`!speed`, a test command): 1 normal, 0.5 half, 2 double.
//! Kept per character (DB `characters.speed`).
//!
//! The client moves a character at its run speed float stat (1397; Vehicle_t::GetMovementSpeed:
//! the stat × GetMovementMultiplier × GetRunspeedMultiplier, the multipliers being what buffs
//! like the speed pad's change). The factor scales the walk and run speed stats (1396-1399) the
//! character's full update gives it, and `FloatStatIIR`s change them while it plays (the own
//! client keeps its defaults over the full update's, and a new active minifigure's body brings
//! them back: they are sent again at every CharInPlay and minifigure swap); the dash
//! speeds (1400-1402) stay. So buffs multiply on top of it. The server's own idea of where a
//! player walking a path is (`game::reported_motion`) uses the same scaled speed.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use bytes::Bytes;
use lmoxide_net::types::Id32;

use crate::game::PLAYER_SPEEDS;

/// The factor's limits.
pub const MIN: f32 = 0.1;
pub const MAX: f32 = 10.0;

/// The walk and run speed stats the factor scales (not the dashes' 1400-1402).
const SCALED: [u32; 4] = [1396, 1397, 1398, 1399];

/// The factor of each character in play (for the records other players get of it).
static FACTORS: LazyLock<Mutex<HashMap<u32, f32>>> = LazyLock::new(Default::default);

/// A factor as typed (`0.5`, `2`, `x2`, `150%`): within MIN..=MAX.
pub fn parse(arg: &str) -> Result<f32, String> {
    let a = arg.trim().trim_start_matches(['x', 'X']);
    let f = match a.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().map(|p| p / 100.0),
        None => a.parse::<f32>(),
    }
    .map_err(|_| format!("not a speed: {arg} (1 normal, 0.5 half, 2 double)"))?;
    if !f.is_finite() || !(MIN..=MAX).contains(&f) {
        return Err(format!("speed {arg}: between {MIN} and {MAX}"));
    }
    Ok(f)
}

/// A saved factor as the server uses it (older or odd values: within the limits, else 1).
pub fn sane(f: f32) -> f32 {
    if f.is_finite() && f > 0.0 { f.clamp(MIN, MAX) } else { 1.0 }
}

/// The movement speed stats of a character with factor `f`.
pub fn speeds(f: f32) -> Cow<'static, [(u32, f32)]> {
    let f = sane(f);
    if f == 1.0 {
        return Cow::Borrowed(PLAYER_SPEEDS);
    }
    PLAYER_SPEEDS.iter().map(|&(stat, v)| (stat, if SCALED.contains(&stat) { v * f } else { v })).collect()
}

/// The run speed (m/s) paths move a character with factor `f` at.
pub fn run_speed(f: f32) -> f32 {
    speeds(f).iter().find(|(stat, _)| *stat == 1397).map_or(5.6, |&(_, v)| v)
}

/// The `FloatStatIIR`s that change a character's speed stats to factor `f` while it plays.
pub fn records(id: Id32, f: f32) -> Vec<Bytes> {
    speeds(f)
        .iter()
        .filter(|(stat, _)| SCALED.contains(stat))
        .map(|&(stat, v)| lmoxide_net::iir::record("FloatStatIIR", id, false, |w| w.u32(stat).f32(v)))
        .collect()
}

/// Character `character` plays with factor `f` (login, `!speed`).
pub fn set(character: u32, f: f32) {
    FACTORS.lock().unwrap().insert(character, sane(f));
}

/// The factor of character `character` (1 if not known).
pub fn of(character: u32) -> f32 {
    FACTORS.lock().unwrap().get(&character).copied().unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors() {
        assert_eq!(parse("0.5"), Ok(0.5));
        assert_eq!(parse("x2"), Ok(2.0));
        assert_eq!(parse("150%"), Ok(1.5));
        assert!(parse("0").is_err() && parse("11").is_err() && parse("fast").is_err() && parse("NaN").is_err());
        assert_eq!(sane(0.0), 1.0);
        assert_eq!(sane(50.0), MAX);
        assert!(matches!(speeds(1.0), Cow::Borrowed(_)));
        let half = speeds(0.5);
        assert_eq!(half.iter().find(|s| s.0 == 1397).unwrap().1, 2.8);
        // dashes keep their speed
        assert_eq!(half.iter().find(|s| s.0 == 1400).unwrap().1, 37.5);
        assert_eq!(run_speed(2.0), 11.2);
        assert_eq!(records(Id32::new(crate::territory::CHARACTER_TYPE, 1), 2.0).len(), 4);
    }
}
