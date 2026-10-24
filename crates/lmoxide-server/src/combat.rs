//! Health and damage, following the client's own formulas (SimpleChar_t::CalculateLife,
//! AttackUtils_c::CalculatePowerRating / CalculateRawDamage) with the minifigure data from
//! data/minifigure_stats.json (tools/re/extract_minifig_stats.py) and the player defaults the
//! original server used (seen in the recorded session).
//!
//! - Life = (level × HealthPerLevel + MinifigureLife) × Π(1 + p/100) over the active upgrades'
//!   Life multipliers ("Fit").
//! - Power rating = int(PowerBaseFig × level × StarFactor) + PowerRatingAll; a hit deals
//!   rating × the attack's AbilityMod × FinalDamageMultiplier (1 + 0.03 per "Strong") ×
//!   (1 ± variance), × 1.2 for normal attacks with "Empowered Normals"; NPCs mitigate part of it
//!   (npc.rs).

use std::{collections::HashMap, sync::LazyLock};

use serde::Deserialize;

/// The game's level cap (TWK_MaxPlayerLevel); characters are at it.
pub const LEVEL: u32 = 30;
const HEALTH_PER_LEVEL: f32 = 100.0;
const STAR_FACTOR: f32 = 3.0;
const POWER_RATING_ALL: f32 = 50.0;
const VARIANCE: f32 = 0.1;
const EMPOWERED_NORMALS: f32 = 1.2;

#[derive(Deserialize)]
struct Data {
    minifigures: HashMap<String, Minifigure>,
}

#[derive(Deserialize)]
struct Minifigure {
    #[serde(default)]
    life: u32,
    /// LEGO Minifigures series (9..15 for most).
    #[serde(default)]
    series: Option<u32>,
    #[serde(default)]
    power: u32,
    normal: Option<Attack>,
    special: Option<Attack>,
    #[serde(default)]
    upgrades: Vec<Upgrade>,
}

#[derive(Deserialize)]
struct Attack {
    spell: u32,
    #[serde(default)]
    hits: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    #[serde(default = "one")]
    ability_mod: f32,
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize)]
struct Upgrade {
    bit: u32,
    /// The upgrade's ability (spell) id; spells check for it in their action criteria.
    #[serde(default)]
    ability: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    effects: Vec<HashMap<String, serde_json::Value>>,
}

static DATA: LazyLock<Data> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../data/minifigure_stats.json")).expect("minifigure_stats.json"));

fn minifigure(template: u32) -> Option<&'static Minifigure> {
    DATA.minifigures.get(&template.to_string())
}

/// The active upgrades of a minifigure (bits of `active`).
fn active_upgrades(m: &Minifigure, active: u32) -> impl Iterator<Item = &Upgrade> {
    m.upgrades.iter().filter(move |u| u.bit < 32 && active & (1 << u.bit) != 0)
}

/// The sum of an upgrade stat's values over the active upgrades.
fn upgrade_sum(m: &Minifigure, active: u32, stat: &str) -> f32 {
    active_upgrades(m, active)
        .flat_map(|u| u.effects.iter())
        .filter(|e| e.get("stat_name").and_then(|v| v.as_str()) == Some(stat))
        .filter_map(|e| {
            e.get("GameAction_FloatValue1").or_else(|| e.get("GameAction_Value")).and_then(|v| v.as_f64())
        })
        .sum::<f64>() as f32
}

/// Maximum health of a minifigure with the given active upgrades.
pub fn max_life(template: u32, active: u32) -> u32 {
    let Some(m) = minifigure(template) else { return LEVEL * HEALTH_PER_LEVEL as u32 + 250 };
    let percent = upgrade_sum(m, active, "Life");
    let life = (LEVEL as f32 * HEALTH_PER_LEVEL + m.life as f32) * (1.0 + percent / 100.0);
    (life + 0.5).max(1.0) as u32
}

/// Damage of one hit of `spell` cast by `template` (before the target's mitigation). `charge`
/// picks the hit of a charge ability's level.
pub fn damage(template: u32, active: u32, spell: u32, charge: Option<usize>) -> u32 {
    let Some(m) = minifigure(template) else { return 50 };
    let is_normal = m.normal.as_ref().is_some_and(|a| a.spell == spell);
    let attack = [m.normal.as_ref(), m.special.as_ref()].into_iter().flatten().find(|a| a.spell == spell);
    let ability_mod = attack
        .and_then(|a| match charge {
            Some(level) if a.hits.len() > 1 => a.hits.get(level.min(a.hits.len() - 1)),
            _ => a.hits.first(),
        })
        .map_or(1.0, |h| h.ability_mod);
    scaled(m, active, is_normal, ability_mod)
}

/// Damage of a hit of `template`'s `spell` with an explicit AbilityMod (hits whose WeaponHit
/// isn't the spell's own, e.g. a dash's path and arrival hits).
pub fn damage_with_mod(template: u32, active: u32, spell: u32, ability_mod: f32) -> u32 {
    let Some(m) = minifigure(template) else { return 50 };
    let is_normal = m.normal.as_ref().is_some_and(|a| a.spell == spell);
    scaled(m, active, is_normal, ability_mod)
}

fn scaled(m: &Minifigure, active: u32, is_normal: bool, ability_mod: f32) -> u32 {
    let rating = (m.power as f32 * LEVEL as f32 * STAR_FACTOR).floor() + POWER_RATING_ALL;
    let mut multiplier = 1.0 + upgrade_sum(m, active, "FinalDamageMultiplier");
    if is_normal && active_upgrades(m, active).any(|u| u.name.contains("Empowered Normals")) {
        multiplier *= EMPOWERED_NORMALS;
    }
    let roll = 1.0 + VARIANCE * (2.0 * random_unit() - 1.0);
    (rating * ability_mod * multiplier * roll).round().max(1.0) as u32
}

/// The LEGO Minifigures series of minifigure `template`.
pub fn series(template: u32) -> Option<u32> {
    minifigure(template).and_then(|m| m.series)
}

/// Whether `template` with the upgrades `active` has upgrade ability `ability` (e.g. the Samurai
/// Warrior's Lotus Dash).
pub fn has_ability(template: u32, active: u32, ability: u32) -> bool {
    minifigure(template).is_some_and(|m| active_upgrades(m, active).any(|u| u.ability == ability))
}

/// Share of incoming damage a minifigure's upgrades absorb ("Tough": MitigationOverrideFlat).
pub fn mitigation(template: u32, active: u32) -> f32 {
    minifigure(template).map_or(0.0, |m| upgrade_sum(m, active, "MitigationOverrideFlat").min(0.55))
}

fn random_unit() -> f32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    (nanos.wrapping_mul(2_654_435_761) % 10_000) as f32 / 10_000.0
}
