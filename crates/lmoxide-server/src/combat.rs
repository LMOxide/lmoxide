//! Health and damage, following the client's own formulas (SimpleChar_t::CalculateLife,
//! AttackUtils_c::CalculatePowerRating / CalculateRawDamage) with the minifigure data from
//! content (content/minifigures, their upgrades' buffs in content/buffs, tunables.json `combat`)
//! and the player defaults the original server used (seen in the recorded session).
//!
//! - Life = (level × HealthPerLevel + MinifigureLife) × Π(1 + p/100) over the active upgrades'
//!   Life multipliers ("Fit").
//! - Power rating = int(PowerBaseFig × level × StarFactor) + PowerRatingAll; a hit deals
//!   rating × the attack's AbilityMod × FinalDamageMultiplier (1 + 0.03 per "Strong") ×
//!   (1 ± variance), × the upgrades' `skill_mod` AbilityMod for normal or special attacks (1.2
//!   for normals with "Empowered Normals"); NPCs mitigate part of it (npc.rs).

use std::{collections::HashMap, sync::LazyLock};

use lmoxide_content::{Attack, BuffEffect, Minifigure};

/// The game's level cap (TWK_MaxPlayerLevel); characters are at it.
pub const LEVEL: u32 = 30;

/// Minifigures by item template (content/minifigures).
static MINIFIGURES: LazyLock<HashMap<u32, &'static Minifigure>> =
    LazyLock::new(|| crate::gamedata::get().minifigures.values().map(|m| (m.template, m)).collect());

fn minifigure(template: u32) -> Option<&'static Minifigure> {
    MINIFIGURES.get(&template).copied()
}

/// A skill's client spell id.
fn spell_of(skill: &str) -> u32 {
    crate::gamedata::get().skills.get(skill).and_then(|s| s.client.as_ref()).map_or(0, |c| c.spell)
}

fn attacks(m: &Minifigure) -> [&Attack; 2] {
    [&m.normal, &m.special]
}

/// A minifigure's combo steps: the spells its attacks' hits are reached through besides the
/// attack itself (Gladius Slash 317739 -> 317740, 317741), which the client casts while the
/// previous step's combo buff is on.
pub fn combo_steps(template: u32) -> Vec<u32> {
    let Some(m) = minifigure(template) else { return Vec::new() };
    let mut steps = Vec::new();
    for attack in attacks(m) {
        let spell = spell_of(&attack.skill);
        for &s in attack.hits.iter().flat_map(|h| &h.via) {
            if s != spell && !steps.contains(&s) {
                steps.push(s);
            }
        }
    }
    steps
}

/// The buffs of the active upgrades of a minifigure (bits of `active`).
fn active_upgrades(m: &Minifigure, active: u32) -> impl Iterator<Item = &'static lmoxide_content::Buff> + '_ {
    let buffs = &crate::gamedata::get().buffs;
    m.upgrades.iter().filter(move |u| u.bit < 32 && active & (1 << u.bit) != 0).filter_map(|u| buffs.get(&u.buff))
}

/// The sum of an upgrade stat's values over the active upgrades.
fn upgrade_sum(m: &Minifigure, active: u32, stat: &str) -> f32 {
    active_upgrades(m, active)
        .flat_map(|b| &b.effects)
        .filter_map(|e| match e {
            BuffEffect::Stat { stat: s, value, .. } if s.as_str() == Some(stat) => value.as_f64(),
            _ => None,
        })
        .sum::<f64>() as f32
}

/// The product of the active upgrades' AbilityMod changes of the normal or special attack.
fn ability_mod_factor(m: &Minifigure, active: u32, is_normal: bool) -> f32 {
    let which = if is_normal { "normal" } else { "special" };
    active_upgrades(m, active)
        .flat_map(|b| &b.effects)
        .filter_map(|e| match e {
            BuffEffect::SkillMod { skill, ability_mod: Some(r), .. } if skill == which => Some(r.fraction() as f32),
            _ => None,
        })
        .product()
}

/// Maximum health of a minifigure with the given active upgrades, for a character of `level`.
pub fn max_life(template: u32, active: u32, level: u32) -> u32 {
    let per_level = crate::gamedata::get().tunables.combat.health_per_level;
    let Some(m) = minifigure(template) else { return level * per_level as u32 + 250 };
    let percent = upgrade_sum(m, active, "Life");
    let life = (level as f32 * per_level + m.stats.life as f32) * (1.0 + percent / 100.0);
    (life + 0.5).max(1.0) as u32
}

/// A minifigure item's health stats: (MinifigureLife, the client adds level x HealthPerLevel to
/// it; MinifigureHealth, the full health).
pub fn item_life(template: u32, active: u32, level: u32) -> (u32, u32) {
    let max = max_life(template, active, level);
    (max.saturating_sub(level * crate::gamedata::get().tunables.combat.health_per_level as u32), max)
}

/// Damage of one hit of `spell` cast by `template` (before the target's mitigation) for a
/// character of `level`. `charge` picks the hit of a charge ability's level.
pub fn damage(template: u32, active: u32, spell: u32, charge: Option<usize>, level: u32) -> u32 {
    let Some(m) = minifigure(template) else { return 50 };
    let is_normal = spell_of(&m.normal.skill) == spell;
    let attack = attacks(m).into_iter().find(|a| spell_of(&a.skill) == spell);
    let ability_mod = attack
        .and_then(|a| match charge {
            Some(level) if a.hits.len() > 1 => a.hits.get(level.min(a.hits.len() - 1)),
            _ => a.hits.first(),
        })
        .map_or(1.0, |h| h.ability_mod.unwrap_or(1.0));
    scaled(m, active, is_normal, ability_mod, level)
}

/// Damage of a hit of `template`'s `spell` with an explicit AbilityMod (hits whose WeaponHit
/// isn't the spell's own, e.g. a dash's path and arrival hits).
pub fn damage_with_mod(template: u32, active: u32, spell: u32, ability_mod: f32, level: u32) -> u32 {
    let Some(m) = minifigure(template) else { return 50 };
    let is_normal = spell_of(&m.normal.skill) == spell;
    scaled(m, active, is_normal, ability_mod, level)
}

fn scaled(m: &Minifigure, active: u32, is_normal: bool, ability_mod: f32, level: u32) -> u32 {
    let variance = crate::gamedata::get().tunables.combat.variance.fraction() as f32;
    let roll = 1.0 + variance * (2.0 * random_unit() - 1.0);
    (unrolled(m, active, is_normal, ability_mod, level) * roll).round().max(1.0) as u32
}

/// A hit's damage before the variance.
fn unrolled(m: &Minifigure, active: u32, is_normal: bool, ability_mod: f32, level: u32) -> f32 {
    let c = &crate::gamedata::get().tunables.combat;
    let rating = (m.stats.power as f32 * level as f32 * c.star_factor).floor() + c.power_rating_all;
    let multiplier = (1.0 + upgrade_sum(m, active, "FinalDamageMultiplier")) * ability_mod_factor(m, active, is_normal);
    rating * ability_mod * multiplier
}

/// The buffs of the upgrades active on `template` with the upgrade bits `active`, in upgrade
/// order.
pub fn active_upgrade_buffs(template: u32, active: u32) -> Vec<&'static lmoxide_content::Buff> {
    minifigure(template).map(|m| active_upgrades(m, active).collect()).unwrap_or_default()
}

/// The LEGO Minifigures series of minifigure `template`.
pub fn series(template: u32) -> Option<u32> {
    minifigure(template).map(|m| m.series)
}

/// Whether `template` with the upgrades `active` has upgrade ability `ability` (e.g. the Samurai
/// Warrior's Lotus Dash).
pub fn has_ability(template: u32, active: u32, ability: u32) -> bool {
    minifigure(template).is_some_and(|m| active_upgrades(m, active).any(|b| b.nano == Some(ability)))
}

/// Share of incoming damage a minifigure's upgrades absorb ("Tough": MitigationOverrideFlat).
pub fn mitigation(template: u32, active: u32) -> f32 {
    let max = crate::gamedata::get().tunables.minifigure_model["player_defaults"]["MaxMitigation"].as_f64().unwrap_or(0.55) as f32;
    minifigure(template).map_or(0.0, |m| upgrade_sum(m, active, "MitigationOverrideFlat").min(max))
}

fn random_unit() -> f32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    (nanos.wrapping_mul(2_654_435_761) % 10_000) as f32 / 10_000.0
}
