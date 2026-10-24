//! Upgrade procs: minifigure upgrades whose effect was server script ("After you smash an
//! enemy, ...", "Every fifth attack ..."). Their numbers come from the "XX_passive generic ..."
//! nanos in the client's spell data (RDB 1000621): the buff each one applies, its stat change
//! and duration (TimeExist). The server shows that buff on the player (`MagicBuffIIR_c`; the
//! client applies its client-side Modify actions itself, so speed boosts move the player
//! faster) and applies damage and defense changes where it computes damage (npc.rs). Values
//! the data doesn't give (heal amounts, durations of buffs without TimeExist) are assumptions,
//! listed in docs/TUNABLES.md.
//!
//! Not done: knockbacks and pulls (Knockback, Harpoon, Crowd Control, Equalizing Burst, Vortex),
//! cooldown changes (Specialist, Technician: the client keeps the recharge times), power-up drop
//! changes (Professional, Resourceful), Elemental Affinity (location elements), Charge Up.

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use lmoxide_net::types::Id32;
use tracing::debug;

use crate::{effects, world};

// Upgrade abilities (minifigure_stats.json `ability`).
const LIONHEARTED: u32 = 421_224;
const BLOCK: u32 = 421_250;
const SMASH_AND_DASH: u32 = 421_249;
const HUNTER: u32 = 421_238;
const BERSERKER: u32 = 421_231;
const BRUTE_FORCE: u32 = 421_217;
const BOOST_SHIELD: u32 = 531_640;
const HARDEN_UP: u32 = 447_636;
const POWERED_BARRIER: u32 = 531_121;
const LEFT_HOOK: u32 = 421_171;
const SLIPPERY_POWER: u32 = 531_629;
const SLIPPERY_DEFENSE: u32 = 531_635;
const MELTDOWN: u32 = 421_178;
const PERSONAL_SPACE: u32 = 531_682;
const RECONSTRUCT: u32 = 421_222;
const SPONGE: u32 = 421_243;
const SECOND_WIND: u32 = 464_965;
const TRUE_GRIT: u32 = 380_599;
const ESCAPE_ARTIST: u32 = 465_317;
const SKIRMISHER: u32 = 481_806;
const REGENERATE: u32 = 421_240;
const SHIELDS_UP: u32 = 443_935;

// Their buffs (nano, duration).
/// "warriordiscipline": FinalDamageMultiplier +0.1, 3 s (taken as Lionhearted's).
const LIONHEARTED_BUFF: (u32, f32, Duration) = (429_771, 0.1, Duration::from_secs(3));
/// MitigationOverrideFlat +0.1, 3 s.
const BLOCK_BUFF: (u32, f32, Duration) = (429_698, 0.1, Duration::from_secs(3));
/// "sprint": MovementMultiplier +50, 2 s.
const SPRINT_BUFF: (u32, Duration) = (428_731, Duration::from_secs(2));
/// "speedonkill": MovementMultiplier +50, 2 s.
const HUNTER_BUFF: (u32, Duration) = (423_293, Duration::from_secs(2));
/// FinalDamageMultiplier +1.0 on the 5th attack.
const BERSERKER_BUFF: (u32, f32) = (423_317, 1.0);
const BERSERKER_EVERY: u32 = 5;
/// How long "this attack" lasts for its hits (an area attack hits several enemies).
const THIS_ATTACK: Duration = Duration::from_millis(1000);
/// FinalDamageMultiplier +0.33 for the next attack.
const BRUTE_FORCE_BUFF: (u32, f32) = (423_295, 0.33);
/// FinalDefenseMultiplier +0.33 against the next hit.
const BOOST_SHIELD_BUFF: (u32, f32) = (531_641, 0.33);
/// FinalDefenseMultiplier +0.1, 3 s, every 3rd normal attack.
const HARDEN_UP_BUFF: (u32, f32, Duration) = (447_637, 0.1, Duration::from_secs(3));
const HARDEN_UP_EVERY: u32 = 3;
/// FinalDefenseMultiplier +1.0 against the next hit, every 5th attack.
const POWERED_BARRIER_BUFF: (u32, f32) = (531_119, 1.0);
const POWERED_BARRIER_EVERY: u32 = 5;
/// "beatup": FinalDamageMultiplier +0.1 every 3rd normal attack; how long is assumed.
const LEFT_HOOK_BUFF: (u32, f32, Duration) = (526_233, 0.1, Duration::from_secs(3));
const LEFT_HOOK_EVERY: u32 = 3;
/// +0.2 damage / defense while not hit for 5 s.
const SLIPPERY_POWER_BUFF: (u32, f32) = (531_630, 0.2);
const SLIPPERY_DEFENSE_BUFF: (u32, f32) = (531_633, 0.2);
const SLIPPERY_AFTER: Duration = Duration::from_secs(5);
/// Meltdown's ground effect (442041) and its pulses: radius 3, AbilityMod 0.05.
const MELTDOWN_EFFECT: u32 = 442_041;
const MELTDOWN_RADIUS: f32 = 3.0;
const MELTDOWN_MOD: f32 = 0.05;
const MELTDOWN_PULSES: u32 = 5;
/// Personal Space: 3 s, a pulse every 0.25 s, radius 3, AbilityMod 0.05, at the smashed enemy.
const PERSONAL_SPACE_BUFF: u32 = 531_687;
const PERSONAL_SPACE_PULSE: Duration = Duration::from_millis(250);
const PERSONAL_SPACE_FOR: Duration = Duration::from_secs(3);
/// True Grit: MitigationOverrideFlat +0.9 for 5 s below 20% health, once per 5 minutes.
const TRUE_GRIT_BUFF: (u32, f32, Duration) = (380_600, 0.9, Duration::from_secs(5));
const TRUE_GRIT_LOCKOUT: Duration = Duration::from_secs(300);
/// Escape Artist: speed and walking through enemies below 20% health (465318), once per 5
/// minutes (lockout nano 523058, 300 s).
const ESCAPE_ARTIST_BUFF: (u32, Duration) = (465_318, Duration::from_secs(3));
const ESCAPE_ARTIST_LOCKOUT: Duration = Duration::from_secs(300);
/// Second Wind: a heal below 20% health, once per 10 minutes (lockout nano 464971, 600 s).
const SECOND_WIND_LOCKOUT: Duration = Duration::from_secs(600);
/// Skirmisher: MovementMultiplier +50 after a normal attack hits.
const SKIRMISHER_BUFF: (u32, Duration) = (481_807, Duration::from_secs(2));
const LOW_HEALTH: f32 = 0.2;
// Heal amounts (shares of maximum health): not in the data.
const RECONSTRUCT_HEAL: f32 = 0.05;
const SPONGE_HEAL: f32 = 0.01;
const SECOND_WIND_HEAL: f32 = 0.5;
const REGENERATE_HEAL: f32 = 0.02;
/// Shields Up: the stance's defense boost (Fortify's 446818 values: +0.02 a hit, at most 5).
const SHIELDS_UP_STEP: f32 = 0.02;
const SHIELDS_UP_MAX: f32 = 0.1;

#[derive(Default)]
struct State {
    template: u32,
    upgrades: u32,
    level: u32,
    abilities: HashSet<u32>,
    normal: u32,
    special: u32,
    attacks: u32,
    normal_attacks: u32,
    /// Damage bonuses: (bonus, until; `None` = the next hit only).
    damage: Vec<(f32, Option<Instant>, u32)>,
    /// Defense: (FinalDefenseMultiplier bonus, flat mitigation, until; `None` = the next hit).
    defense: Vec<(f32, f32, Option<Instant>, u32)>,
    last_hit_taken: Option<Instant>,
    lockouts: HashMap<u32, Instant>,
}

static PLAYERS: LazyLock<Mutex<HashMap<Id32, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The player's active minifigure changed (or its upgrades).
pub fn set_active(player: Id32, template: u32, upgrades: u32, level: u32) {
    let abilities = crate::combat::active_abilities(template, upgrades);
    let spells = crate::collection::spells(template);
    let mut players = PLAYERS.lock().unwrap();
    let s = players.entry(player).or_default();
    *s = State {
        template,
        upgrades,
        level,
        abilities,
        normal: spells.first().copied().unwrap_or(0),
        special: spells.get(1).copied().unwrap_or(0),
        lockouts: std::mem::take(&mut s.lockouts),
        last_hit_taken: s.last_hit_taken,
        ..Default::default()
    };
}

pub fn left(player: Id32) {
    PLAYERS.lock().unwrap().remove(&player);
}

fn show(player: Id32, records: Vec<Bytes>) {
    world::send_playfield(player, true, world::Out::Records(records));
}

fn buff(player: Id32, nano: u32, for_: Option<Duration>) -> Bytes {
    // buffs without an end last until removed (`unbuff`)
    let cs = for_.map_or(60_000, |d| (d.as_millis() / 10) as i32);
    effects::buff_add(player, nano, player, cs)
}

fn unbuff(player: Id32, nano: u32) -> Bytes {
    effects::buff_remove(player, nano, player)
}

/// `player` casts `spell` (any MagicCommand): counts attacks for "every n-th attack" procs and
/// runs the special attack's procs.
pub fn cast(player: Id32, spell: u32) {
    let mut records = Vec::new();
    {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&player) else { return };
        let now = Instant::now();
        let normal = spell == s.normal;
        if !normal && spell != s.special {
            return;
        }
        s.attacks += 1;
        if normal {
            s.normal_attacks += 1;
        }
        if s.abilities.contains(&BERSERKER) && s.attacks % BERSERKER_EVERY == 0 {
            // this attack's hits
            s.damage.push((BERSERKER_BUFF.1, Some(now + THIS_ATTACK), BERSERKER_BUFF.0));
            records.push(buff(player, BERSERKER_BUFF.0, Some(THIS_ATTACK)));
        }
        if s.abilities.contains(&POWERED_BARRIER) && s.attacks % POWERED_BARRIER_EVERY == 0 {
            s.defense.push((POWERED_BARRIER_BUFF.1, 0.0, None, POWERED_BARRIER_BUFF.0));
            records.push(buff(player, POWERED_BARRIER_BUFF.0, None));
        }
        if normal && s.abilities.contains(&LEFT_HOOK) && s.normal_attacks % LEFT_HOOK_EVERY == 0 {
            let (nano, bonus, d) = LEFT_HOOK_BUFF;
            s.damage.push((bonus, Some(now + d), nano));
            records.push(buff(player, nano, Some(d)));
        }
        if normal && s.abilities.contains(&HARDEN_UP) && s.normal_attacks % HARDEN_UP_EVERY == 0 {
            let (nano, bonus, d) = HARDEN_UP_BUFF;
            s.defense.push((bonus, 0.0, Some(now + d), nano));
            records.push(buff(player, nano, Some(d)));
        }
        if !normal {
            if s.abilities.contains(&LIONHEARTED) {
                let (nano, bonus, d) = LIONHEARTED_BUFF;
                s.damage.push((bonus, Some(now + d), nano));
                records.push(buff(player, nano, Some(d)));
            }
            if s.abilities.contains(&BLOCK) {
                let (nano, mitigation, d) = BLOCK_BUFF;
                s.defense.push((0.0, mitigation, Some(now + d), nano));
                records.push(buff(player, nano, Some(d)));
            }
            if s.abilities.contains(&SMASH_AND_DASH) {
                records.push(buff(player, SPRINT_BUFF.0, Some(SPRINT_BUFF.1)));
            }
        }
    }
    if !records.is_empty() {
        debug!(?player, spell, "attack procs");
        show(player, records);
    }
}

/// `attacker` hits with `spell` for `damage`: returns the damage with the attacker's proc
/// bonuses (one-hit bonuses are used up), and runs the per-hit procs.
pub fn outgoing(attacker: Id32, spell: u32, damage: u32) -> u32 {
    let mut records = Vec::new();
    let mut heal = 0.0;
    let factor = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&attacker) else { return damage };
        let now = Instant::now();
        if spell == s.normal && s.abilities.contains(&SKIRMISHER) {
            records.push(buff(attacker, SKIRMISHER_BUFF.0, Some(SKIRMISHER_BUFF.1)));
        }
        if s.abilities.contains(&SPONGE) {
            heal += SPONGE_HEAL;
        }
        s.damage.retain(|&(_, until, _)| until.is_none_or(|u| u > now));
        let mut bonus: f32 = s.damage.iter().map(|&(b, _, _)| b).sum();
        if s.abilities.contains(&SLIPPERY_POWER) && s.last_hit_taken.is_none_or(|t| t.elapsed() >= SLIPPERY_AFTER) {
            bonus += SLIPPERY_POWER_BUFF.1;
        }
        for (_, _, nano) in s.damage.iter().filter(|(_, until, _)| until.is_none()) {
            records.push(unbuff(attacker, *nano));
        }
        s.damage.retain(|&(_, until, _)| until.is_some());
        1.0 + bonus
    };
    if heal > 0.0 {
        heal_share(attacker, heal);
    }
    if !records.is_empty() {
        show(attacker, records);
    }
    ((damage as f32) * factor).round() as u32
}

/// `attacker` defeated an enemy with `spell` at `at` on `playfield`.
pub fn killed(attacker: Id32, spell: u32, playfield: u32, at: [f32; 3]) {
    let mut records = Vec::new();
    let mut heal = 0.0;
    let (template, upgrades, level, special_kill, meltdown, personal_space) = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&attacker) else { return };
        if s.abilities.contains(&HUNTER) {
            records.push(buff(attacker, HUNTER_BUFF.0, Some(HUNTER_BUFF.1)));
        }
        if s.abilities.contains(&BRUTE_FORCE) {
            s.damage.push((BRUTE_FORCE_BUFF.1, None, BRUTE_FORCE_BUFF.0));
            records.push(buff(attacker, BRUTE_FORCE_BUFF.0, None));
        }
        if s.abilities.contains(&BOOST_SHIELD) {
            s.defense.push((BOOST_SHIELD_BUFF.1, 0.0, None, BOOST_SHIELD_BUFF.0));
            records.push(buff(attacker, BOOST_SHIELD_BUFF.0, None));
        }
        if s.abilities.contains(&RECONSTRUCT) {
            heal += RECONSTRUCT_HEAL;
        }
        let special = spell != s.normal;
        (s.template, s.upgrades, s.level, special, s.abilities.contains(&MELTDOWN), s.abilities.contains(&PERSONAL_SPACE))
    };
    if heal > 0.0 {
        heal_share(attacker, heal);
    }
    if !records.is_empty() {
        show(attacker, records);
    }
    if special_kill && (meltdown || personal_space) {
        let damage = crate::combat::damage_with_mod(template, upgrades, spell, MELTDOWN_MOD, level);
        if meltdown {
            // a field around the player: an impact, then a pulse a second
            let here = world::position_of(attacker).unwrap_or(at);
            show(attacker, vec![effects::effect_package_at(attacker, MELTDOWN_EFFECT, here)]);
            tokio::spawn(async move {
                for _ in 0..=MELTDOWN_PULSES {
                    let here = world::position_of(attacker).unwrap_or(here);
                    crate::npc::hit_area(attacker, playfield, here, MELTDOWN_RADIUS, spell, damage);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });
        }
        if personal_space {
            // a damaging field where the enemy was smashed
            show(attacker, vec![buff(attacker, PERSONAL_SPACE_BUFF, Some(PERSONAL_SPACE_FOR))]);
            tokio::spawn(async move {
                let until = Instant::now() + PERSONAL_SPACE_FOR;
                while Instant::now() < until {
                    crate::npc::hit_area(attacker, playfield, at, MELTDOWN_RADIUS, spell, damage);
                    tokio::time::sleep(PERSONAL_SPACE_PULSE).await;
                }
            });
        }
    }
}

/// `player` is hit for `damage` (before mitigation): returns it with the defense procs (and
/// uses up one-hit shields); `channelling` = during a channelled special.
pub fn incoming(player: Id32, damage: u32, channelling: bool) -> u32 {
    let mut records = Vec::new();
    let mut heal = 0.0;
    let damage = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&player) else { return damage };
        let now = Instant::now();
        let slippery = s.abilities.contains(&SLIPPERY_DEFENSE) && s.last_hit_taken.is_none_or(|t| t.elapsed() >= SLIPPERY_AFTER);
        s.last_hit_taken = Some(now);
        if channelling && s.abilities.contains(&REGENERATE) {
            heal += REGENERATE_HEAL;
        }
        if channelling && s.abilities.contains(&SHIELDS_UP) {
            let stacked: f32 = s.defense.iter().filter(|d| d.3 == 446_818).map(|d| d.0).sum();
            if stacked < SHIELDS_UP_MAX {
                s.defense.push((SHIELDS_UP_STEP, 0.0, Some(now + Duration::from_millis(1500)), 446_818));
            }
            records.push(buff(player, 446_818, Some(Duration::from_millis(1500))));
        }
        s.defense.retain(|&(_, _, until, _)| until.is_none_or(|u| u > now));
        let mut defense: f32 = s.defense.iter().map(|d| d.0).sum();
        let mitigation: f32 = s.defense.iter().map(|d| d.1).sum();
        if slippery {
            defense += SLIPPERY_DEFENSE_BUFF.1;
        }
        for (_, _, _, nano) in s.defense.iter().filter(|d| d.2.is_none()) {
            records.push(unbuff(player, *nano));
        }
        s.defense.retain(|d| d.2.is_some());
        ((damage as f32) * (1.0 - mitigation.min(0.9)) / (1.0 + defense)).round().max(1.0) as u32
    };
    if heal > 0.0 {
        heal_share(player, heal);
    }
    if !records.is_empty() {
        show(player, records);
    }
    damage
}

/// After `player` took damage: the low-health procs (Second Wind, True Grit, Escape Artist).
pub fn after_damage(player: Id32, health: u32, max: u32) {
    if health == 0 || max == 0 || (health as f32) >= max as f32 * LOW_HEALTH {
        return;
    }
    let mut records = Vec::new();
    let mut heal = 0.0;
    {
        let mut players = PLAYERS.lock().unwrap();
        let Some(s) = players.get_mut(&player) else { return };
        let now = Instant::now();
        let ready = |ability: u32, lockout: Duration, s: &mut State| {
            let ok = s.abilities.contains(&ability) && s.lockouts.get(&ability).is_none_or(|t| *t <= now);
            if ok {
                s.lockouts.insert(ability, now + lockout);
            }
            ok
        };
        if ready(SECOND_WIND, SECOND_WIND_LOCKOUT, s) {
            heal += SECOND_WIND_HEAL;
        }
        if ready(TRUE_GRIT, TRUE_GRIT_LOCKOUT, s) {
            let (nano, mitigation, d) = TRUE_GRIT_BUFF;
            s.defense.push((0.0, mitigation, Some(now + d), nano));
            records.push(buff(player, nano, Some(d)));
        }
        if ready(ESCAPE_ARTIST, ESCAPE_ARTIST_LOCKOUT, s) {
            records.push(buff(player, ESCAPE_ARTIST_BUFF.0, Some(ESCAPE_ARTIST_BUFF.1)));
        }
    }
    if heal > 0.0 {
        heal_share(player, heal);
    }
    if !records.is_empty() {
        show(player, records);
    }
}

fn heal_share(player: Id32, share: f32) {
    let max = crate::npc::player_max_health(player);
    if max == 0 {
        return;
    }
    let health = crate::npc::heal_player(player, ((max as f32) * share).ceil() as u32);
    world::send_to(player, world::Out::Records(vec![crate::game::int32_stat(player, crate::game::STAT_HEALTH, health)]));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn berserker_and_brute_force() {
        let p = Id32::new(50_000, 1);
        // the Alien Avenger has Berserker among its upgrades; activate every bit
        let alien_avenger = 41_754;
        set_active(p, alien_avenger, 0xfff, 30);
        let has = PLAYERS.lock().unwrap()[&p].abilities.contains(&BERSERKER);
        if has {
            let normal = PLAYERS.lock().unwrap()[&p].normal;
            let hits: Vec<u32> = (0..5)
                .map(|_| {
                    cast(p, normal);
                    outgoing(p, normal, 100)
                })
                .collect();
            assert!(hits[4] >= 200, "{hits:?}");
            assert!(hits[0] < 200, "{hits:?}");
        }
        left(p);
    }
}
