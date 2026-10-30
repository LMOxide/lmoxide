//! Enemies' support abilities that no client spell describes (tunables `enemy_abilities`):
//!
//! - healers (the Medic) throw a heal at the most hurt ally in range below `below` of its
//!   maximum (bosses too with `heals_bosses`), never at themselves;
//! - shields (Skeleton Trident, Space Merc Defender, ...) cover the enemy itself, an ally or
//!   everyone around for `duration`: a shielded enemy takes `taken` of the damage (0%: none,
//!   npc.rs `hit`);
//! - totems (the Witch Doctor) mark the ground under the player and hit everyone within `radius`
//!   of the mark `delay` later.
//!
//! The client has the effects (and the animations) but no spell saying who cast them how: which
//! enemy does what and all numbers are the user's memory or guesses (docs/TUNABLES.md "Enemy
//! support abilities"). An ability is chosen in `npc::engage` before the enemy's attacks, when
//! its cooldown is over and it has something to do.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use lmoxide_content::tunables::{EnemyHealer, EnemyShield, EnemyTotem, ShieldOn};
use lmoxide_net::types::Id32;
use tracing::info;

use crate::effects;
use crate::instance::Pf;
use crate::world;

/// Cooldown keys in the NPC's spell cooldowns (no client spell has these ids).
pub const HEAL_COOLDOWN: u32 = u32::MAX - 1;
pub const SHIELD_COOLDOWN: u32 = u32::MAX - 2;
pub const TOTEM_COOLDOWN: u32 = u32::MAX - 3;
/// The caster stands still this long after casting one.
const CAST_TIME: Duration = Duration::from_millis(1000);

/// What enemies of one look (monster data) can do.
#[derive(Default)]
pub struct Kit {
    pub heal: Option<&'static EnemyHealer>,
    pub shield: Option<&'static EnemyShield>,
    pub totem: Option<&'static EnemyTotem>,
}

static KITS: LazyLock<HashMap<u32, Kit>> = LazyLock::new(|| {
    let data = crate::gamedata::get();
    let t = &data.tunables.enemy_abilities;
    let looks = |npcs: &[String]| -> Vec<u32> {
        npcs.iter().filter_map(|r| data.npcs.get(r.as_str())).filter_map(|n| n.render.monster_data).collect()
    };
    let mut kits: HashMap<u32, Kit> = HashMap::new();
    for h in &t.healers {
        for look in looks(&h.npcs) {
            kits.entry(look).or_default().heal = Some(h);
        }
    }
    for s in &t.shields {
        for look in looks(&s.npcs) {
            kits.entry(look).or_default().shield = Some(s);
        }
    }
    for x in &t.totems {
        for look in looks(&x.npcs) {
            kits.entry(look).or_default().totem = Some(x);
        }
    }
    kits
});

/// The support abilities of enemies with monster data `look`, if any.
pub fn kit(look: u32) -> Option<&'static Kit> {
    KITS.get(&look)
}

/// An enemy near the caster, as the abilities see it.
#[derive(Debug, Clone, Copy)]
pub struct Ally {
    pub id: Id32,
    pub pos: [f32; 3],
    pub health: u32,
    pub max: u32,
    pub boss: bool,
    pub shielded: bool,
}

/// An ability to carry out.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Heal { ally: Id32, amount: u32 },
    Shield { on: Vec<Id32> },
    Totem { at: [f32; 3] },
}

impl Plan {
    /// Its cooldown key and length.
    pub fn cooldown(&self, kit: &Kit) -> (u32, Duration) {
        match self {
            Plan::Heal { .. } => (HEAL_COOLDOWN, kit.heal.map_or(Duration::ZERO, |h| h.cooldown.to_std())),
            Plan::Shield { .. } => (SHIELD_COOLDOWN, kit.shield.map_or(Duration::ZERO, |s| s.cooldown.to_std())),
            Plan::Totem { .. } => (TOTEM_COOLDOWN, kit.totem.map_or(Duration::ZERO, |t| t.cooldown.to_std())),
        }
    }

    pub fn busy(&self) -> Duration {
        CAST_TIME
    }
}

/// What the caster `me` (at `here`) does now, if anything: a heal for a hurt ally, else a
/// shield, else a totem at the target (at `at`). `ready(key)`: whether the
/// ability's cooldown is over. `allies` are the living enemies near it, itself among them.
pub fn choose(kit: &Kit, me: Id32, here: [f32; 3], at: [f32; 3], allies: &[Ally], ready: impl Fn(u32) -> bool, heals_bosses: bool) -> Option<Plan> {
    if let Some(h) = kit.heal.filter(|_| ready(HEAL_COOLDOWN)) {
        let below = h.below.fraction() as f32;
        let hurt = allies
            .iter()
            .filter(|a| a.id != me && a.health > 0 && a.max > 0 && (heals_bosses || !a.boss))
            .filter(|a| (a.health as f32) < a.max as f32 * below && distance(here, a.pos) <= h.range)
            .min_by(|a, b| (a.health as f32 / a.max as f32).total_cmp(&(b.health as f32 / b.max as f32)));
        if let Some(a) = hurt {
            let amount = ((a.max as f64 * h.heal.fraction()).round() as u32).max(1);
            return Some(Plan::Heal { ally: a.id, amount });
        }
    }
    if let Some(s) = kit.shield.filter(|_| ready(SHIELD_COOLDOWN)) {
        let on: Vec<Id32> = match s.on {
            ShieldOn::Itself => allies.iter().filter(|a| a.id == me && !a.shielded).map(|a| a.id).collect(),
            ShieldOn::Ally => {
                let hurt = |a: &&Ally| a.health as f32 / a.max.max(1) as f32;
                allies
                    .iter()
                    .filter(|a| a.id != me && !a.shielded && distance(here, a.pos) <= s.range)
                    .min_by(|a, b| hurt(a).total_cmp(&hurt(b)))
                    .or_else(|| allies.iter().find(|a| a.id == me && !a.shielded))
                    .map(|a| a.id)
                    .into_iter()
                    .collect()
            }
            ShieldOn::Area => allies.iter().filter(|a| a.id == me || distance(here, a.pos) <= s.range).filter(|a| !a.shielded).map(|a| a.id).collect(),
        };
        if !on.is_empty() {
            return Some(Plan::Shield { on });
        }
    }
    if let Some(t) = kit.totem.filter(|_| ready(TOTEM_COOLDOWN))
        && distance(here, at) <= t.range
    {
        return Some(Plan::Totem { at });
    }
    None
}

/// Carries out `plan` of enemy `caster` (on `playfield`, hitting for `power` before the
/// ability's AbilityMod).
pub fn run(caster: Id32, playfield: Pf, power: f32, kit: &'static Kit, plan: Plan) {
    info!(npc = ?caster, ?plan, "enemy ability");
    match plan {
        Plan::Heal { ally, amount } => {
            let Some(h) = kit.heal else { return };
            if let Some(fx) = h.projectile_effect {
                show(caster, vec![effects::effect_package_on(caster, ally, fx, "on_projectile_fired", effects::new_instance())]);
            }
            let (travel, heal_fx) = (h.travel.to_std(), h.heal_effect);
            tokio::spawn(async move {
                tokio::time::sleep(travel).await;
                if crate::npc::heal_npc(ally, amount).is_some()
                    && let Some(fx) = heal_fx
                {
                    show(ally, vec![effects::effect_package(ally, fx)]);
                }
            });
        }
        Plan::Shield { on } => {
            let Some(s) = kit.shield else { return };
            let (taken, duration) = (s.taken.fraction() as f32, s.duration.to_std());
            let mut records = Vec::new();
            if let Some(fx) = s.cast_effect {
                records.push(effects::effect_package(caster, fx));
            }
            let mut shown = Vec::new();
            for &target in &on {
                crate::npc::shield_npc(target, taken, Instant::now() + duration);
                if let Some(fx) = s.projectile_effect.filter(|_| target != caster) {
                    records.push(effects::effect_package_on(caster, target, fx, "on_projectile_fired", effects::new_instance()));
                }
                if let Some(fx) = s.effect {
                    let instance = effects::new_instance();
                    records.push(effects::effect_package_hook(target, fx, "", "", 0.0, 0, instance));
                    shown.push((target, fx, instance));
                }
            }
            show(caster, records);
            if !shown.is_empty() {
                tokio::spawn(async move {
                    tokio::time::sleep(duration).await;
                    for (target, fx, instance) in shown {
                        if crate::npc::health_of(target).is_some() {
                            show(target, vec![effects::stop_effect_package(target, fx, 0, instance)]);
                        }
                    }
                });
            }
        }
        Plan::Totem { at } => {
            let Some(t) = kit.totem else { return };
            if let Some(fx) = t.effect {
                let instance = effects::new_instance();
                show(
                    caster,
                    vec![
                        effects::effect_package_hook(caster, fx, "charge", "", 0.0, 0, instance),
                        effects::ground_effect_hook(caster, fx, "onhit", at, effects::new_instance()),
                    ],
                );
            }
            let (delay, radius, ability_mod, drop_fx) = (t.delay.to_std(), t.radius, t.ability_mod, t.drop_effect);
            let stun = t.stun_buff.map(|b| (b, t.stun_time.to_std()));
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                if crate::npc::health_of(caster).is_none() {
                    return;
                }
                if let Some(fx) = drop_fx {
                    show(caster, vec![effects::effect_package_at(caster, fx, at)]);
                }
                let hit: Vec<Id32> = world::live_positions(playfield)
                    .into_iter()
                    .filter(|(id, p)| crate::npc::is_player(*id) && distance(*p, at) <= radius)
                    .map(|(id, _)| id)
                    .collect();
                for player in hit {
                    crate::npc::hurt_player(caster, player, crate::npc::DEFAULT_SPELL, crate::npc::enemy_hit(power, ability_mod));
                    if let Some((buff, time)) = stun {
                        show(caster, effects::show_buff(player, buff, caster, Some(time)));
                    }
                }
            });
        }
    }
}

fn show(from: Id32, records: Vec<bytes::Bytes>) {
    if !records.is_empty() {
        world::send_playfield(from, true, world::Out::Records(records));
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmoxide_content::units::{Duration as D, Ratio};

    fn id(n: u32) -> Id32 {
        Id32::new(crate::territory::CHARACTER_TYPE, n)
    }

    fn ally(n: u32, x: f32, health: u32, boss: bool) -> Ally {
        Ally { id: id(n), pos: [x, 0.0, 0.0], health, max: 100, boss, shielded: false }
    }

    #[test]
    fn the_content_names_the_enemies() {
        // Medic, Witch Doctor, Skeleton Trident, Space Merc Defender looks
        assert!(kit(192825).is_some_and(|k| k.heal.is_some()));
        assert!(kit(201479).is_some_and(|k| k.totem.is_some() && k.heal.is_none()));
        assert!(kit(99330).is_some_and(|k| k.shield.is_some_and(|s| s.on == ShieldOn::Itself)));
        assert!(kit(379392).is_some_and(|k| k.shield.is_some_and(|s| s.on == ShieldOn::Ally)));
    }

    #[test]
    fn healers_heal_the_most_hurt_ally_not_themselves() {
        let heal = Box::leak(Box::new(EnemyHealer { heal: Ratio(0.2), below: Ratio(0.6), range: 10.0, cooldown: D(8.0), travel: D(0.5), ..Default::default() }));
        let kit = Kit { heal: Some(heal), ..Default::default() };
        let allies = [ally(1, 0.0, 10, false), ally(2, 5.0, 50, false), ally(3, 6.0, 30, true), ally(4, 50.0, 5, false)];
        // itself (1) is the most hurt, 4 is too far: the boss (3) at 30%
        let plan = choose(&kit, id(1), [0.0; 3], [0.0; 3], &allies, |_| true, true);
        assert_eq!(plan, Some(Plan::Heal { ally: id(3), amount: 20 }));
        let plan = choose(&kit, id(1), [0.0; 3], [0.0; 3], &allies, |_| true, false);
        assert_eq!(plan, Some(Plan::Heal { ally: id(2), amount: 20 }));
        assert_eq!(choose(&kit, id(1), [0.0; 3], [0.0; 3], &allies, |_| false, true), None);
    }

    #[test]
    fn shields_and_totems() {
        let shield = Box::leak(Box::new(EnemyShield { taken: Ratio(0.0), on: ShieldOn::Area, range: 5.0, duration: D(5.0), cooldown: D(15.0), ..Default::default() }));
        let totem = Box::leak(Box::new(EnemyTotem { range: 12.0, radius: 2.0, delay: D(1.0), cooldown: D(5.0), ability_mod: 2.5, ..Default::default() }));
        let kit = Kit { shield: Some(shield), totem: Some(totem), ..Default::default() };
        let mut allies = [ally(1, 0.0, 100, false), ally(2, 4.0, 100, false), ally(3, 9.0, 100, false)];
        let plan = choose(&kit, id(1), [0.0; 3], [8.0, 0.0, 0.0], &allies, |_| true, true);
        assert_eq!(plan, Some(Plan::Shield { on: vec![id(1), id(2)] }));
        allies[0].shielded = true;
        allies[1].shielded = true;
        // everyone near is shielded: the totem, at the player
        let plan = choose(&kit, id(1), [0.0; 3], [8.0, 0.0, 0.0], &allies, |_| true, true);
        assert_eq!(plan, Some(Plan::Totem { at: [8.0, 0.0, 0.0] }));
        assert_eq!(choose(&kit, id(1), [0.0; 3], [20.0, 0.0, 0.0], &allies, |_| true, true), None);
    }
}
