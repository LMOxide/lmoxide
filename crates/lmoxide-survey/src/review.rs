//! A working set (what editors made, on the world editor's server) against an approved one (what
//! a game server applies): the changes waiting for approval, grouped by playfield, feature and
//! author, approving them, and summaries of either set (docs/WORLD-CONTENT.md).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::{Note, Placement, Script, SmashRule, TriggerWall, set::Set};

/// The playfield smash achievement rules' changes go under (they belong to no playfield; servers
/// name it "Achievements").
pub const ACHIEVEMENTS: u32 = 0;

/// What is approved together: one author's changes to one feature in one playfield
/// ([`ACHIEVEMENTS`] for smash achievement rules).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChangeKey {
    pub playfield: u32,
    pub feature: String,
    pub author: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Placement,
    Trigger,
    Note,
    Script,
    SmashRule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Added,
    Changed,
    Removed,
}

/// One record that differs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub kind: RecordKind,
    pub id: String,
    /// What it is ("Crate", "Trigger wall", a note's comment, a script's name, a smash rule's
    /// achievement key and enemies).
    pub name: String,
    pub op: Op,
    /// For placements: what changed ("moved 3.2 m", "level 5 -> 7", "deletes a guess", ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    /// The working record's last change (unix seconds; removals: the approved record's).
    pub updated: u64,
    /// The record's tags (removals: the approved record's).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    #[serde(flatten)]
    pub key: ChangeKey,
    pub items: Vec<Item>,
    /// "2 added, 1 moved, 1 removed".
    pub summary: String,
}

/// A record of either set, by kind.
#[derive(Clone)]
enum Rec<'a> {
    P(&'a Placement),
    T(&'a TriggerWall),
    N(&'a Note),
    S(&'a Script),
    R(&'a SmashRule),
}

impl Rec<'_> {
    fn feature(&self) -> &str {
        match self {
            Rec::P(x) => &x.feature,
            Rec::T(x) => &x.feature,
            Rec::N(x) => &x.feature,
            Rec::S(x) => &x.feature,
            Rec::R(x) => &x.feature,
        }
    }
    fn author(&self) -> &str {
        match self {
            Rec::P(x) => &x.author,
            Rec::T(x) => &x.author,
            Rec::N(x) => &x.author,
            Rec::S(x) => &x.author,
            Rec::R(x) => &x.author,
        }
    }
    fn tags(&self) -> &[String] {
        match self {
            Rec::P(x) => &x.tags,
            Rec::T(x) => &x.tags,
            Rec::N(x) => &x.tags,
            Rec::S(x) => &x.tags,
            Rec::R(x) => &x.tags,
        }
    }
    fn updated(&self) -> u64 {
        match self {
            Rec::P(x) => x.updated,
            Rec::T(x) => x.updated,
            Rec::N(x) => x.updated,
            Rec::S(x) => x.updated,
            Rec::R(x) => x.updated,
        }
    }
    /// Deleted (placements and walls keep a removed record).
    fn removed(&self) -> bool {
        match self {
            Rec::P(x) => x.removed,
            Rec::T(x) => x.removed,
            _ => false,
        }
    }
    fn name(&self) -> String {
        let short = |s: &str| if s.chars().count() > 60 { format!("{}…", s.chars().take(60).collect::<String>()) } else { s.to_string() };
        match self {
            Rec::P(x) if !x.name.is_empty() => x.name.clone(),
            Rec::P(x) => format!("{} {}", x.kind.as_str(), x.monster_data.or(x.template).unwrap_or(0)),
            Rec::T(x) if !x.name.is_empty() => x.name.clone(),
            Rec::T(_) => "Trigger wall".into(),
            Rec::N(x) if !x.comment.is_empty() => short(&x.comment),
            Rec::N(x) => format!("{:?} note", x.kind),
            Rec::S(x) if !x.name.is_empty() => x.name.clone(),
            Rec::S(_) => "Script".into(),
            Rec::R(x) if x.enemies.is_empty() => x.id.clone(),
            Rec::R(x) => short(&format!("{}: {}", x.id, x.enemies.join(", "))),
        }
    }
    /// Same content, who saved it and when aside.
    fn same(&self, other: &Rec) -> bool {
        match (self, other) {
            (Rec::P(a), Rec::P(b)) => a.same_content(b),
            (Rec::T(a), Rec::T(b)) => a.same_content(b),
            (Rec::N(a), Rec::N(b)) => Note { author: String::new(), updated: 0, ..(*a).clone() } == Note { author: String::new(), updated: 0, ..(*b).clone() },
            (Rec::S(a), Rec::S(b)) => Script { author: String::new(), updated: 0, ..(*a).clone() } == Script { author: String::new(), updated: 0, ..(*b).clone() },
            (Rec::R(a), Rec::R(b)) => SmashRule { author: String::new(), updated: 0, ..(*a).clone() } == SmashRule { author: String::new(), updated: 0, ..(*b).clone() },
            _ => false,
        }
    }
}

/// Every record of a playfield, by kind and id (the smash rules in [`ACHIEVEMENTS`]).
fn records(set: &Set, pf: u32) -> HashMap<(RecordKind, &str), Rec<'_>> {
    let mut out = HashMap::new();
    if let Some(f) = set.placements.get(&pf) {
        out.extend(f.placements.iter().map(|x| ((RecordKind::Placement, x.id.as_str()), Rec::P(x))));
        out.extend(f.triggers.iter().map(|x| ((RecordKind::Trigger, x.id.as_str()), Rec::T(x))));
    }
    if let Some(f) = set.notes.get(&pf) {
        out.extend(f.notes.iter().map(|x| ((RecordKind::Note, x.id.as_str()), Rec::N(x))));
        out.extend(f.scripts.iter().map(|x| ((RecordKind::Script, x.id.as_str()), Rec::S(x))));
    }
    if pf == ACHIEVEMENTS {
        out.extend(set.smash_rules.iter().map(|x| ((RecordKind::SmashRule, x.id.as_str()), Rec::R(x))));
    }
    out
}

fn playfields(a: &Set, b: &Set) -> Vec<u32> {
    let mut pfs: Vec<u32> = a.placements.keys().chain(a.notes.keys()).chain(b.placements.keys()).chain(b.notes.keys()).copied().collect();
    if !a.smash_rules.is_empty() || !b.smash_rules.is_empty() {
        pfs.push(ACHIEVEMENTS);
    }
    pfs.sort();
    pfs.dedup();
    pfs
}

/// What changed in a placement, in words.
fn placement_details(new: &Placement, old: Option<&Placement>) -> Vec<String> {
    let mut d = Vec::new();
    match old {
        None if new.removed && new.replaces.is_some() => d.push("deletes a guess".into()),
        None if new.replaces.is_some() => d.push("confirms or edits a guess".into()),
        None => {}
        Some(o) => {
            let moved = crate::distance(o.pos, new.pos);
            if moved >= 0.05 {
                d.push(format!("moved {moved:.1} m"));
            }
            if (o.heading - new.heading).abs() > 0.01 {
                d.push("turned".into());
            }
            if o.level != new.level {
                d.push(format!("level {} -> {}", o.level.map_or("-".into(), |l| l.to_string()), new.level.map_or("-".into(), |l| l.to_string())));
            }
            if o.monster_data != new.monster_data || o.template != new.template || o.kind != new.kind {
                d.push("changed into another spawn".into());
            }
            if o.wave != new.wave {
                d.push("wave changed".into());
            }
            if o.patrol != new.patrol {
                d.push("patrol changed".into());
            }
            if o.feature != new.feature {
                d.push(format!("moved from feature {}", o.feature));
            }
            if !o.removed && new.removed {
                d.push("deleted".into());
            }
        }
    }
    d
}

/// "tags +todo -approximate", if they changed.
fn tag_details(old: &[String], new: &[String]) -> Option<String> {
    let added = new.iter().filter(|t| !old.contains(t)).map(|t| format!("+{t}"));
    let removed = old.iter().filter(|t| !new.contains(t)).map(|t| format!("-{t}"));
    let all: Vec<String> = added.chain(removed).collect();
    (!all.is_empty()).then(|| format!("tags {}", all.join(" ")))
}

/// The changes of `working` that `approved` doesn't have yet, by playfield, feature and author
/// (a removal is the remover's when the working set keeps a removed record, else the approved
/// record's author's).
pub fn changes(working: &Set, approved: &Set) -> Vec<Change> {
    let mut by_key: BTreeMap<ChangeKey, Vec<Item>> = BTreeMap::new();
    for pf in playfields(working, approved) {
        let (w, a) = (records(working, pf), records(approved, pf));
        for (k, rec) in &w {
            let old = a.get(k);
            if old.is_some_and(|o| o.same(rec)) {
                continue;
            }
            // a removed record the approved set never had changes nothing
            if old.is_none() && rec.removed() && !matches!(rec, Rec::P(p) if p.replaces.is_some()) {
                continue;
            }
            let op = match old {
                None => Op::Added,
                Some(o) if rec.removed() && !o.removed() => Op::Removed,
                Some(_) => Op::Changed,
            };
            let op = if old.is_none() && rec.removed() { Op::Removed } else { op };
            let mut details = match (rec, old) {
                (Rec::P(p), Some(Rec::P(o))) => placement_details(p, Some(o)),
                (Rec::P(p), _) => placement_details(p, None),
                _ => Vec::new(),
            };
            details.extend(old.and_then(|o| tag_details(o.tags(), rec.tags())));
            let key = ChangeKey { playfield: pf, feature: rec.feature().to_string(), author: rec.author().to_string() };
            by_key.entry(key).or_default().push(Item { kind: k.0, id: k.1.to_string(), name: rec.name(), op, details, updated: rec.updated(), tags: rec.tags().to_vec() });
        }
        for (k, old) in &a {
            if w.contains_key(k) {
                continue;
            }
            let key = ChangeKey { playfield: pf, feature: old.feature().to_string(), author: old.author().to_string() };
            by_key.entry(key).or_default().push(Item { kind: k.0, id: k.1.to_string(), name: old.name(), op: Op::Removed, details: Vec::new(), updated: old.updated(), tags: old.tags().to_vec() });
        }
    }
    by_key
        .into_iter()
        .map(|(key, mut items)| {
            items.sort_by(|a, b| (a.kind, &a.name, &a.id).cmp(&(b.kind, &b.name, &b.id)));
            let count = |op: Op| items.iter().filter(|i| i.op == op).count();
            let parts: Vec<String> = [(Op::Added, "added"), (Op::Changed, "changed"), (Op::Removed, "removed")]
                .into_iter()
                .filter(|(op, _)| count(*op) > 0)
                .map(|(op, w)| format!("{} {w}", count(op)))
                .collect();
            Change { key, summary: parts.join(", "), items }
        })
        .collect()
}

/// Takes the working set's version of the given records (`None`: every record of the change)
/// into the approved set; records the working set no longer has are removed. The working set's
/// features registry comes along. Returns how many records changed.
pub fn approve(approved: &mut Set, working: &Set, change: &Change, only: Option<&[String]>) -> usize {
    let pf = change.key.playfield;
    let mut n = 0;
    for item in change.items.iter().filter(|i| only.is_none_or(|o| o.contains(&i.id))) {
        n += 1;
        let id = item.id.as_str();
        match item.kind {
            RecordKind::Placement => {
                let new = working.placements.get(&pf).and_then(|f| f.placements.iter().find(|x| x.id == id)).cloned();
                let list = &mut approved.placements_mut(pf).placements;
                list.retain(|x| x.id != id);
                list.extend(new);
            }
            RecordKind::Trigger => {
                let new = working.placements.get(&pf).and_then(|f| f.triggers.iter().find(|x| x.id == id)).cloned();
                let list = &mut approved.placements_mut(pf).triggers;
                list.retain(|x| x.id != id);
                list.extend(new);
            }
            RecordKind::Note => {
                let new = working.notes.get(&pf).and_then(|f| f.notes.iter().find(|x| x.id == id)).cloned();
                let list = &mut approved.notes_mut(pf).notes;
                list.retain(|x| x.id != id);
                list.extend(new);
            }
            RecordKind::Script => {
                let new = working.notes.get(&pf).and_then(|f| f.scripts.iter().find(|x| x.id == id)).cloned();
                let list = &mut approved.notes_mut(pf).scripts;
                list.retain(|x| x.id != id);
                list.extend(new);
            }
            RecordKind::SmashRule => {
                let new = working.smash_rule(id).cloned();
                approved.smash_rules.retain(|x| x.id != id);
                approved.smash_rules.extend(new);
            }
        }
    }
    approved.features = working.features.clone();
    n
}

/// How much a set holds for one feature in one playfield.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub placements: usize,
    /// placements that delete a guess
    pub deletions: usize,
    pub in_waves: usize,
    pub triggers: usize,
    pub notes: usize,
    pub scripts: usize,
    #[serde(default)]
    pub smash_rules: usize,
    pub authors: Vec<String>,
    /// How many records carry each tag.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, usize>,
    /// unix seconds of the newest change
    pub updated: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryRow {
    pub playfield: u32,
    pub feature: String,
    #[serde(flatten)]
    pub counts: Counts,
}

/// The whole set at a glance: per playfield and feature, what it holds (smash rules in
/// [`ACHIEVEMENTS`]).
pub fn summary(set: &Set) -> Vec<SummaryRow> {
    let mut rows: BTreeMap<(u32, String), Counts> = BTreeMap::new();
    let mut add = |pf: u32, feature: &str, author: &str, tags: &[String], updated: u64, f: &mut dyn FnMut(&mut Counts)| {
        let c = rows.entry((pf, feature.to_string())).or_default();
        f(c);
        for t in tags {
            *c.tags.entry(t.clone()).or_default() += 1;
        }
        if !author.is_empty() && !c.authors.iter().any(|a| a == author) {
            c.authors.push(author.into());
        }
        c.updated = c.updated.max(updated);
    };
    for (pf, file) in &set.placements {
        for p in &file.placements {
            if p.removed && p.replaces.is_none() {
                continue;
            }
            add(*pf, &p.feature, &p.author, &p.tags, p.updated, &mut |c| {
                if p.removed {
                    c.deletions += 1;
                } else {
                    c.placements += 1;
                    c.in_waves += p.wave.is_some() as usize;
                }
            });
        }
        for t in file.triggers.iter().filter(|t| !t.removed) {
            add(*pf, &t.feature, &t.author, &t.tags, t.updated, &mut |c| c.triggers += 1);
        }
    }
    for (pf, file) in &set.notes {
        for x in &file.notes {
            add(*pf, &x.feature, &x.author, &x.tags, x.updated, &mut |c| c.notes += 1);
        }
        for x in &file.scripts {
            add(*pf, &x.feature, &x.author, &x.tags, x.updated, &mut |c| c.scripts += 1);
        }
    }
    for x in &set.smash_rules {
        add(ACHIEVEMENTS, &x.feature, &x.author, &x.tags, x.updated, &mut |c| c.smash_rules += 1);
    }
    rows.into_iter().map(|((playfield, feature), mut counts)| {
        counts.authors.sort();
        SummaryRow { playfield, feature, counts }
    }).collect()
}

/// Keeps only the records of features `active` selects (a feature is active when it or one of
/// its ancestors is listed), for a game server to apply.
pub fn only_features(set: &Set, active: &[String]) -> Set {
    let on = |f: &str| active.iter().any(|a| crate::features::within(f, a));
    let mut out = set.clone();
    for f in out.placements.values_mut() {
        f.placements.retain(|x| on(&x.feature));
        f.triggers.retain(|x| on(&x.feature));
    }
    for f in out.notes.values_mut() {
        f.notes.retain(|x| on(&x.feature));
        f.scripts.retain(|x| on(&x.feature));
    }
    out.smash_rules.retain(|x| on(&x.feature));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Origin, PlacementsFile};

    fn crate_at(id: &str, x: f32, feature: &str, author: &str) -> Placement {
        Placement {
            id: id.into(),
            kind: Kind::Smashable,
            name: "Crate".into(),
            monster_data: Some(1),
            template: Some(2),
            level: None,
            pos: [x, 0.0, 0.0],
            heading: 0.0,
            replaces: None,
            removed: false,
            wave: None,
            patrol: Vec::new(),
            comment: String::new(),
            tags: Vec::new(),
            feature: feature.into(),
            author: author.into(),
            updated: 1,
        }
    }

    fn with(placements: Vec<Placement>) -> Set {
        let mut s = Set::default();
        s.placements.insert(1200, PlacementsFile { placements, ..PlacementsFile::new(1200) });
        s
    }

    #[test]
    fn changes_and_approval() {
        let approved = with(vec![crate_at("p-1", 0.0, "live", "ann"), crate_at("p-2", 5.0, "live", "ann")]);
        let mut moved = crate_at("p-1", 3.0, "live", "bob");
        moved.updated = 2;
        let mut gone = crate_at("g-9", 9.0, "live", "bob");
        gone.removed = true;
        gone.replaces = Some(Origin { id: "g-9".into(), kind: Kind::Smashable, monster_data: Some(1), template: Some(2), pos: [9.0, 0.0, 0.0] });
        let working = with(vec![moved, crate_at("p-3", 7.0, "feature/xmas/2026", "bob"), gone]);
        let ch = changes(&working, &approved);
        let keys: Vec<(&str, &str)> = ch.iter().map(|c| (c.key.feature.as_str(), c.key.author.as_str())).collect();
        assert_eq!(keys, [("feature/xmas/2026", "bob"), ("live", "ann"), ("live", "bob")]);
        let live_bob = &ch[2];
        assert_eq!(live_bob.summary, "1 changed, 1 removed");
        assert!(live_bob.items.iter().any(|i| i.id == "p-1" && i.details == ["moved 3.0 m"]));
        assert!(live_bob.items.iter().any(|i| i.id == "g-9" && i.op == Op::Removed && i.details == ["deletes a guess"]));
        // p-2 is gone from the working set: a removal, ann's
        assert_eq!(ch[1].items[0].op, Op::Removed);

        let mut a = approved.clone();
        for c in &ch {
            approve(&mut a, &working, c, None);
        }
        assert!(changes(&working, &a).is_empty(), "everything approved");
        let s = summary(&a);
        assert_eq!(s.iter().map(|r| (r.feature.as_str(), r.counts.placements, r.counts.deletions)).collect::<Vec<_>>(), [("feature/xmas/2026", 1, 0), ("live", 1, 1)]);
        let live_only = only_features(&a, &["live".into()]);
        assert_eq!(live_only.placements[&1200].placements.len(), 2);
    }

    #[test]
    fn smash_rules() {
        let rule = |enemies: &[&str], feature: &str, updated| SmashRule {
            id: "2163".into(),
            enemies: enemies.iter().map(|e| e.to_string()).collect(),
            comment: String::new(),
            tags: Vec::new(),
            feature: feature.into(),
            author: "ann".into(),
            updated,
        };
        let mut working = with(vec![crate_at("p-1", 0.0, "live", "ann")]);
        working.smash_rules.push(rule(&["Skeleton"], "live", 1));
        let mut approved = Set::default();
        let ch = changes(&working, &approved);
        assert_eq!(ch.iter().map(|c| c.key.playfield).collect::<Vec<_>>(), [ACHIEVEMENTS, 1200]);
        assert_eq!((ch[0].items[0].kind, ch[0].items[0].op, ch[0].items[0].name.as_str()), (RecordKind::SmashRule, Op::Added, "2163: Skeleton"));
        assert_eq!(serde_json::to_value(ch[0].items[0].kind).unwrap(), "smash_rule");
        approve(&mut approved, &working, &ch[0], None);
        assert_eq!(approved.smash_rules, working.smash_rules);
        assert_eq!(changes(&working, &approved).len(), 1, "the placement is left");
        // saved again by someone else, same content: no change
        working.smash_rules[0] = SmashRule { author: "bob".into(), updated: 9, ..working.smash_rules[0].clone() };
        assert!(changes(&working, &approved).iter().all(|c| c.key.playfield != ACHIEVEMENTS));
        working.smash_rules[0].enemies.push("Zombie".into());
        let ch = changes(&working, &approved);
        assert_eq!((ch[0].key.author.as_str(), ch[0].items[0].op), ("bob", Op::Changed));
        // dropped: a removal
        working.smash_rules.clear();
        let ch = changes(&working, &approved);
        assert_eq!(ch[0].items[0].op, Op::Removed);
        approve(&mut approved, &working, &ch[0], None);
        assert!(approved.smash_rules.is_empty());
        // counted, and only the active features' rules kept
        let s = Set { smash_rules: vec![rule(&["Witch"], "feature/halloween", 1), SmashRule { id: "2164".into(), ..rule(&[], "live", 1) }], ..Default::default() };
        assert_eq!(summary(&s).iter().map(|r| (r.playfield, r.counts.smash_rules)).collect::<Vec<_>>(), [(ACHIEVEMENTS, 1), (ACHIEVEMENTS, 1)]);
        assert_eq!(only_features(&s, &["live".into()]).smash_rules.len(), 1);
    }

    #[test]
    fn tags_show_in_changes_and_summary() {
        let approved = with(vec![crate_at("p-1", 0.0, "live", "ann")]);
        let tagged = Placement { tags: vec!["todo".into(), "approximate".into()], updated: 2, ..crate_at("p-1", 0.0, "live", "ann") };
        let working = with(vec![tagged.clone()]);
        let ch = changes(&working, &approved);
        assert_eq!(ch[0].items[0].details, ["tags +todo +approximate"]);
        assert_eq!(ch[0].items[0].tags, ["todo", "approximate"]);
        let less = with(vec![Placement { tags: vec!["todo".into()], ..tagged }]);
        assert_eq!(changes(&less, &working)[0].items[0].details, ["tags -approximate"]);
        assert_eq!(summary(&working)[0].counts.tags.get("todo"), Some(&1));
    }
}
