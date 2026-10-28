//! Rolling loot tables: the rules (chances, weighted entries rolled `rolls` times, `when`,
//! nested tables) in one place, for the game server (which turns the drops into client items)
//! and for the world editor's drop rates ([`simulate`]), so both roll the same way.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::{LootBundle, LootEntry, LootTable, MinifigureSource};
use crate::record::Ref;
use crate::units::Range;

/// Tables nest at most this deep (a table naming itself stops there).
pub const MAX_DEPTH: u32 = 8;

/// One thing a roll drops.
#[derive(Debug, Clone, PartialEq)]
pub enum Drop {
    Star,
    Heart,
    Potion,
    /// A part of a minifigure from here.
    Part(MinifigureSource),
    /// A minifigure bag.
    Bag(MinifigureSource),
    /// One of these chests (`item/...`).
    Chest(Vec<Ref>),
    /// This item (`item/...`).
    Item(Ref),
}

/// What a roll needs from its caller.
pub struct Roller<'a> {
    /// A loot table by id (`loot/...`).
    pub table: &'a dyn Fn(&str) -> Option<LootTable>,
    /// Whether an entry limited to these worlds (`when.worlds`) drops here.
    pub in_worlds: &'a dyn Fn(&[Ref]) -> bool,
    /// Extra chance in percent for entries raised by `orb_chance` (Resourceful).
    pub orb_bonus: u64,
    /// Random numbers (any distribution over u64).
    pub random: &'a mut dyn FnMut() -> u64,
    /// Tables missing or nesting too deep.
    pub warnings: Vec<String>,
}

impl Roller<'_> {
    /// True with probability `p` (0..1).
    pub fn chance_of(&mut self, p: f64) -> bool {
        ((self.random)() % 1_000_000) < (p * 1_000_000.0).round().max(0.0) as u64
    }

    /// A number in `r`.
    pub fn in_range(&mut self, r: Range) -> u32 {
        r.min + ((self.random)() % u64::from(r.max - r.min + 1)) as u32
    }

    /// One of `items` (weight, value), by weight.
    pub fn pick_weighted<T: Copy>(&mut self, items: &[(u32, T)]) -> Option<T> {
        let total: u64 = items.iter().map(|&(w, _)| u64::from(w)).sum();
        if total == 0 {
            return None;
        }
        let mut r = (self.random)() % total;
        for &(w, v) in items {
            if r < u64::from(w) {
                return Some(v);
            }
            r -= u64::from(w);
        }
        None
    }

    /// Rolls table `id` into `out`: entries in order; those with a `chance` roll on their own,
    /// those without `chance` or `weight` always drop, and the weighted ones (`rolls` times, one
    /// each) where the first of them is listed.
    pub fn roll(&mut self, id: &str, out: &mut Vec<Drop>) {
        self.roll_at(id, 0, out)
    }

    fn roll_at(&mut self, id: &str, depth: u32, out: &mut Vec<Drop>) {
        let Some(table) = (self.table)(id) else {
            self.warnings.push(format!("no loot table {id}"));
            return;
        };
        if depth > MAX_DEPTH {
            self.warnings.push(format!("loot tables nest too deep at {id}"));
            return;
        }
        let applies = |e: &LootEntry, in_worlds: &dyn Fn(&[Ref]) -> bool| {
            // everyone is a member here; once_per_player isn't kept yet
            e.when.as_ref().is_none_or(|w| w.worlds.is_empty() || in_worlds(&w.worlds))
        };
        let weighted: Vec<&LootEntry> = table.entries.iter().filter(|e| e.weight.is_some() && applies(e, self.in_worlds)).collect();
        let mut weighted_done = false;
        for e in &table.entries {
            if e.weight.is_some() {
                if !weighted_done {
                    weighted_done = true;
                    let choices: Vec<(u32, usize)> = weighted.iter().enumerate().map(|(i, e)| (e.weight.unwrap_or(0), i)).collect();
                    for _ in 0..table.rolls {
                        if let Some(i) = self.pick_weighted(&choices) {
                            self.give(&weighted[i].drop, depth, out);
                        }
                    }
                }
                continue;
            }
            if !applies(e, self.in_worlds) {
                continue;
            }
            let drops = match e.chance {
                None => true,
                Some(p) => {
                    let bonus = e.when.as_ref().and_then(|w| w.bonus_stat.as_deref()) == Some("orb_chance");
                    let extra = if bonus { self.orb_bonus as f64 / 100.0 } else { 0.0 };
                    self.chance_of(p.fraction() + extra)
                }
            };
            if drops {
                self.give(&e.drop, depth, out);
            }
        }
    }

    /// The drops of one loot bundle.
    fn give(&mut self, b: &LootBundle, depth: u32, out: &mut Vec<Drop>) {
        for (r, d) in [(b.stars, Drop::Star), (b.hearts, Drop::Heart), (b.potions, Drop::Potion)] {
            if let Some(r) = r {
                let n = self.in_range(r);
                out.extend(std::iter::repeat_n(d, n as usize));
            }
        }
        if let Some(d) = &b.minifig_parts {
            let n = self.in_range(d.count);
            out.extend(std::iter::repeat_n(Drop::Part(d.from.clone()), n as usize));
        }
        if let Some(d) = &b.minifig_bags {
            let n = self.in_range(d.count);
            out.extend(std::iter::repeat_n(Drop::Bag(d.from.clone()), n as usize));
        }
        if !b.chest.is_empty() {
            out.push(Drop::Chest(b.chest.clone()));
        }
        out.extend(b.items.iter().cloned().map(Drop::Item));
        if let Some(table) = &b.table {
            self.roll_at(table, depth + 1, out);
        }
    }
}

/// How often a table drops one thing, over many rolls.
#[derive(Debug, Clone, Serialize)]
pub struct Rate {
    /// What it is: `stars`, `item/...`, `chest: item/a | item/b`, `minifigure part (the world's)` ...
    pub what: String,
    /// Share of rolls that drop at least one.
    pub chance: f64,
    /// How many per roll, on average.
    pub per_roll: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Simulation {
    pub rolls: u32,
    pub rates: Vec<Rate>,
    /// Share of rolls that drop nothing.
    pub nothing: f64,
    pub warnings: Vec<String>,
}

fn label(d: &Drop) -> String {
    let from = |f: &MinifigureSource| match f {
        MinifigureSource::World(_) => "the world's".to_string(),
        MinifigureSource::Some(list) => list.join(" | "),
    };
    match d {
        Drop::Star => "stars".into(),
        Drop::Heart => "health orbs".into(),
        Drop::Potion => "potions".into(),
        Drop::Part(f) => format!("minifigure part ({})", from(f)),
        Drop::Bag(f) => format!("minifigure bag ({})", from(f)),
        Drop::Chest(list) => format!("chest: {}", list.join(" | ")),
        Drop::Item(i) => i.clone(),
    }
}

/// Rolls table `id` `rolls` times and counts what drops (with a fixed seed: the same table
/// gives the same rates).
pub fn simulate(table: &dyn Fn(&str) -> Option<LootTable>, in_worlds: &dyn Fn(&[Ref]) -> bool, id: &str, rolls: u32) -> Simulation {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut random = move || {
        // xorshift64*, as the server's
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    };
    let mut r = Roller { table, in_worlds, orb_bonus: 0, random: &mut random, warnings: Vec::new() };
    let mut counts: BTreeMap<String, (u64, u32)> = BTreeMap::new();
    let mut empty = 0;
    for _ in 0..rolls {
        let mut out = Vec::new();
        r.roll(id, &mut out);
        if out.is_empty() {
            empty += 1;
        }
        let mut this: BTreeMap<String, u64> = BTreeMap::new();
        for d in &out {
            *this.entry(label(d)).or_default() += 1;
        }
        for (what, n) in this {
            let c = counts.entry(what).or_default();
            c.0 += n;
            c.1 += 1;
        }
    }
    let mut warnings = r.warnings;
    warnings.sort();
    warnings.dedup();
    let n = rolls.max(1) as f64;
    let mut rates: Vec<Rate> = counts.into_iter().map(|(what, (total, rolls_with))| Rate { what, chance: rolls_with as f64 / n, per_roll: total as f64 / n }).collect();
    rates.sort_by(|a, b| b.chance.total_cmp(&a.chance).then_with(|| a.what.cmp(&b.what)));
    Simulation { rolls, rates, nothing: empty as f64 / n, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rates_follow_the_rules() {
        let tables: BTreeMap<String, LootTable> = [
            (
                "loot/a",
                json!({ "id": "loot/a", "rolls": 1, "entries": [
                    { "drop": { "stars": [2, 4] } },
                    { "chance": "25%", "drop": { "items": ["item/x"] } },
                    { "weight": 3, "drop": { "potions": 1 } },
                    { "weight": 1, "drop": { "table": "loot/b" } },
                    { "chance": "100%", "when": { "worlds": ["world/elsewhere"] }, "drop": { "hearts": 1 } }
                ] }),
            ),
            ("loot/b", json!({ "id": "loot/b", "entries": [{ "drop": { "chest": ["item/c1", "item/c2"] } }] })),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::from_value(v).unwrap()))
        .collect();
        let s = simulate(&|id| tables.get(id).cloned(), &|_| false, "loot/a", 20_000);
        let rate = |w: &str| s.rates.iter().find(|r| r.what == w).map_or((0.0, 0.0), |r| (r.chance, r.per_roll));
        assert_eq!(rate("stars").0, 1.0);
        assert!((rate("stars").1 - 3.0).abs() < 0.05, "{:?}", rate("stars"));
        assert!((rate("item/x").0 - 0.25).abs() < 0.02);
        assert!((rate("potions").0 - 0.75).abs() < 0.02);
        assert!((rate("chest: item/c1 | item/c2").0 - 0.25).abs() < 0.02);
        assert_eq!(rate("health orbs").0, 0.0, "only in world/elsewhere");
        assert_eq!(s.nothing, 0.0);
        assert!(simulate(&|id| tables.get(id).cloned(), &|_| true, "loot/missing", 10).warnings[0].contains("no loot table"));
    }
}
