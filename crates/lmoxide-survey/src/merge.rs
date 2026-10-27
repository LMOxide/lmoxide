//! Importing someone else's set into yours (docs/SURVEY.md "Merging").
//!
//! Their records are matched to yours first (scripts and waves, then notes, then placements), and
//! anything that names a matched record (a placement's wave, a wave trigger's note, placement or
//! wave) is pointed at yours. Matched records are either duplicates (same id, same content),
//! near-duplicates (different ids, the same thing within the [`Tolerance`]) or conflicts (the same
//! record, or the same replaced guess, saying different things), which [`Prefer`] settles. Smash
//! achievement rules are matched by id (their achievement) alone.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{Area, NotesFile, Placement, PlacementsFile, Script, SmashRule, Trigger, TriggerWall, Wave, WallAction, distance, heading_difference, review::ACHIEVEMENTS, set::Set};

/// How close two records must be to be the same thing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    /// Metres between positions (area centres for notes and scripts).
    pub distance: f32,
    /// Degrees between headings.
    pub heading: f32,
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance { distance: 1.0, heading: 20.0 }
    }
}

/// Which side wins a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Prefer {
    Ours,
    Theirs,
    /// The one changed last (`updated`); ours on a tie.
    #[default]
    Newer,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub tolerance: Tolerance,
    pub prefer: Prefer,
    /// Their ids to add even though they look like near-duplicates of ours.
    pub take: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Ours,
    Theirs,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Near {
    pub playfield: u32,
    pub theirs: String,
    pub ours: String,
    /// Metres apart.
    pub distance: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    pub playfield: u32,
    pub theirs: String,
    pub ours: String,
    pub kept: Side,
}

/// What a merge did, by their ids (the preview the web UI shows).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    pub added: Vec<String>,
    pub duplicates: Vec<String>,
    pub near: Vec<Near>,
    pub conflicts: Vec<Conflict>,
}

/// Merges their set into ours.
pub fn merge(ours: &mut Set, theirs: &Set, opts: &Options) -> Report {
    for a in &theirs.info.authors {
        if !ours.info.authors.contains(a) {
            ours.info.authors.push(a.clone());
        }
    }
    let mut report = Report::default();
    let playfields: std::collections::BTreeSet<u32> =
        theirs.placements.keys().chain(theirs.notes.keys()).copied().collect();
    for pf in playfields {
        let empty_p = PlacementsFile::new(pf);
        let empty_n = NotesFile::new(pf);
        let their_p = theirs.placements.get(&pf).unwrap_or(&empty_p);
        let their_n = theirs.notes.get(&pf).unwrap_or(&empty_n);
        let mut m = Merger { pf, opts, report: &mut report, map: HashMap::new() };
        m.scripts(&mut ours.notes_mut(pf).scripts, &their_n.scripts);
        m.notes(ours.notes_mut(pf), &their_n.notes);
        m.placements(&mut ours.placements_mut(pf).placements, &their_p.placements);
        m.walls(&mut ours.placements_mut(pf).triggers, &their_p.triggers);
        ours.placements_mut(pf);
        let (Some(n), Some(p)) = (ours.notes.get_mut(&pf), ours.placements.get_mut(&pf)) else { unreachable!() };
        m.repoint(n, &mut p.placements, &mut p.triggers);
        if ours.placements[&pf].placements.is_empty() && ours.placements[&pf].triggers.is_empty() {
            ours.placements.remove(&pf);
        }
        if ours.notes[&pf].notes.is_empty() && ours.notes[&pf].scripts.is_empty() {
            ours.notes.remove(&pf);
        }
    }
    let mut m = Merger { pf: ACHIEVEMENTS, opts, report: &mut report, map: HashMap::new() };
    m.smash_rules(&mut ours.smash_rules, &theirs.smash_rules);
    report
}

struct Merger<'a> {
    pf: u32,
    opts: &'a Options,
    report: &'a mut Report,
    /// Their id -> our id, for records matched under another id.
    map: HashMap<String, String>,
}

impl Merger<'_> {
    fn ours<'s>(&'s self, id: &'s str) -> &'s str {
        self.map.get(id).map_or(id, String::as_str)
    }

    fn keep_theirs(&self, our_updated: u64, their_updated: u64) -> bool {
        match self.opts.prefer {
            Prefer::Ours => false,
            Prefer::Theirs => true,
            Prefer::Newer => their_updated > our_updated,
        }
    }

    fn near(&mut self, theirs: &str, ours: &str, distance: f32) {
        self.report.near.push(Near { playfield: self.pf, theirs: theirs.into(), ours: ours.into(), distance });
        self.map.insert(theirs.into(), ours.into());
    }

    fn conflict(&mut self, theirs: &str, ours: &str, our_updated: u64, their_updated: u64) -> bool {
        let keep = self.keep_theirs(our_updated, their_updated);
        let kept = if keep { Side::Theirs } else { Side::Ours };
        self.report.conflicts.push(Conflict { playfield: self.pf, theirs: theirs.into(), ours: ours.into(), kept });
        if theirs != ours {
            self.map.insert(theirs.into(), ours.into());
        }
        keep
    }

    fn close(&self, a: Option<&Area>, b: Option<&Area>) -> Option<f32> {
        match (a, b) {
            (None, None) => Some(0.0),
            (Some(a), Some(b)) => Some(distance(a.center(), b.center())).filter(|d| *d <= self.opts.tolerance.distance),
            _ => None,
        }
    }

    fn scripts(&mut self, ours: &mut Vec<Script>, theirs: &[Script]) {
        for ts in theirs {
            let by_id = ours.iter().position(|s| s.id == ts.id);
            let near = || {
                if self.opts.take.contains(&ts.id) || ts.name.is_empty() {
                    return None;
                }
                ours.iter().enumerate().find_map(|(i, s)| {
                    let same_name = s.name.eq_ignore_ascii_case(&ts.name);
                    same_name.then(|| self.close(s.area.as_ref(), ts.area.as_ref())).flatten().map(|d| (i, d))
                })
            };
            let i = match (by_id, by_id.is_none().then(near).flatten()) {
                (Some(i), _) => {
                    let header = |s: &Script| Script { waves: Vec::new(), author: String::new(), updated: 0, ..s.clone() };
                    if header(&ours[i]) == header(ts) {
                        self.report.duplicates.push(ts.id.clone());
                    } else if self.conflict(&ts.id, &ours[i].id.clone(), ours[i].updated, ts.updated) {
                        let waves = std::mem::take(&mut ours[i].waves);
                        ours[i] = Script { waves, ..ts.clone() };
                    }
                    i
                }
                (None, Some((i, d))) => {
                    self.near(&ts.id, &ours[i].id.clone(), d);
                    i
                }
                (None, None) => {
                    self.report.added.push(ts.id.clone());
                    ours.push(ts.clone());
                    continue;
                }
            };
            let our_updated = ours[i].updated;
            for tw in &ts.waves {
                self.wave(&mut ours[i].waves, tw, our_updated, ts.updated);
            }
        }
    }

    /// Waves have no time of their own: a wave conflict goes by their scripts' times.
    fn wave(&mut self, ours: &mut Vec<Wave>, tw: &Wave, our_updated: u64, their_updated: u64) {
        if let Some(i) = ours.iter().position(|w| w.id == tw.id) {
            if ours[i] == *tw {
                self.report.duplicates.push(tw.id.clone());
            } else if self.conflict(&tw.id, &tw.id, our_updated, their_updated) {
                ours[i] = tw.clone();
            }
        } else if let Some(i) = (!tw.name.is_empty() && !self.opts.take.contains(&tw.id))
            .then(|| ours.iter().position(|w| w.name.eq_ignore_ascii_case(&tw.name)))
            .flatten()
        {
            self.near(&tw.id, &ours[i].id.clone(), 0.0);
        } else {
            self.report.added.push(tw.id.clone());
            ours.push(tw.clone());
        }
    }

    fn notes(&mut self, ours: &mut NotesFile, theirs: &[crate::Note]) {
        let ours = &mut ours.notes;
        for tn in theirs {
            if let Some(i) = ours.iter().position(|n| n.id == tn.id) {
                let strip = |n: &crate::Note| crate::Note { author: String::new(), updated: 0, ..n.clone() };
                if strip(&ours[i]) == strip(tn) {
                    self.report.duplicates.push(tn.id.clone());
                } else if self.conflict(&tn.id, &tn.id, ours[i].updated, tn.updated) {
                    ours[i] = tn.clone();
                }
                continue;
            }
            let near = (!self.opts.take.contains(&tn.id))
                .then(|| {
                    ours.iter().find_map(|n| {
                        let same = n.kind == tn.kind && n.reference == tn.reference && n.comment.trim() == tn.comment.trim();
                        same.then(|| self.close(Some(&n.area), Some(&tn.area))).flatten().map(|d| (n.id.clone(), d))
                    })
                })
                .flatten();
            match near {
                Some((id, d)) => self.near(&tn.id, &id, d),
                None => {
                    self.report.added.push(tn.id.clone());
                    ours.push(tn.clone());
                }
            }
        }
    }

    fn placements(&mut self, ours: &mut Vec<Placement>, theirs: &[Placement]) {
        let t = self.opts.tolerance;
        for tp in theirs {
            let mut tp = tp.clone();
            self.repoint_placement(&mut tp);
            if let Some(i) = ours.iter().position(|p| p.id == tp.id) {
                if ours[i].same_content(&tp) {
                    self.report.duplicates.push(tp.id.clone());
                } else if self.conflict(&tp.id, &tp.id, ours[i].updated, tp.updated) {
                    ours[i] = tp;
                }
                continue;
            }
            let looks_alike = |p: &Placement| {
                p.kind == tp.kind && p.monster_data == tp.monster_data && p.template == tp.template && p.removed == tp.removed
            };
            // (bouncers: their landing points must be as close too)
            let same_target = |p: &Placement| match (p.target, tp.target) {
                (Some(a), Some(b)) => distance(a, b) <= t.distance,
                (a, b) => a.is_none() && b.is_none(),
            };
            let within = |p: &Placement| {
                let d = distance(p.pos, tp.pos);
                (d <= t.distance && heading_difference(p.heading, tp.heading) <= t.heading && same_target(p)).then_some(d)
            };
            // the same guess replaced on both sides: the same placement if alike and close
            if let Some(origin) = &tp.replaces
                && let Some(i) = ours.iter().position(|p| p.replaces.as_ref().is_some_and(|o| o.id == origin.id)) {
                    match within(&ours[i]).filter(|_| looks_alike(&ours[i]) && ours[i].wave == tp.wave) {
                        Some(d) => self.near(&tp.id, &ours[i].id.clone(), d),
                        None => {
                            let id = ours[i].id.clone();
                            if self.conflict(&tp.id, &id, ours[i].updated, tp.updated) {
                                ours[i] = Placement { id, ..tp };
                            }
                        }
                    }
                    continue;
                }
            let near = (!self.opts.take.contains(&tp.id))
                .then(|| {
                    ours.iter()
                        .filter(|p| looks_alike(p) && p.wave == tp.wave && p.replaces.is_none() && tp.replaces.is_none())
                        .filter_map(|p| within(p).map(|d| (p.id.clone(), d)))
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                })
                .flatten();
            match near {
                Some((id, d)) => self.near(&tp.id, &id, d),
                None => {
                    self.report.added.push(tp.id.clone());
                    ours.push(tp);
                }
            }
        }
    }

    /// Trigger walls: the same id is a duplicate or a conflict; a wall doing the same thing
    /// (action) within the tolerance is a near-duplicate.
    fn walls(&mut self, ours: &mut Vec<TriggerWall>, theirs: &[TriggerWall]) {
        let t = self.opts.tolerance;
        for tw in theirs {
            let mut tw = tw.clone();
            if let WallAction::Script { script: Some(s), wave } = &mut tw.action {
                *s = self.ours(s).to_string();
                if let Some(w) = wave {
                    *w = self.ours(w).to_string();
                }
            }
            if let Some(i) = ours.iter().position(|w| w.id == tw.id) {
                if ours[i].same_content(&tw) {
                    self.report.duplicates.push(tw.id.clone());
                } else if self.conflict(&tw.id, &tw.id, ours[i].updated, tw.updated) {
                    ours[i] = tw;
                }
                continue;
            }
            let near = (!self.opts.take.contains(&tw.id))
                .then(|| {
                    ours.iter()
                        .filter(|w| w.action == tw.action && w.removed == tw.removed)
                        .map(|w| (w.id.clone(), distance(w.pos, tw.pos), heading_difference(w.heading, tw.heading)))
                        .filter(|(_, d, h)| *d <= t.distance && *h <= t.heading)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                })
                .flatten();
            match near {
                Some((id, d, _)) => self.near(&tw.id, &id, d),
                None => {
                    self.report.added.push(tw.id.clone());
                    ours.push(tw);
                }
            }
        }
    }

    /// Smash rules: the same id (achievement) is a duplicate or a conflict, else theirs is added.
    fn smash_rules(&mut self, ours: &mut Vec<SmashRule>, theirs: &[SmashRule]) {
        for tr in theirs {
            match ours.iter().position(|r| r.id == tr.id) {
                Some(i) => {
                    let strip = |r: &SmashRule| SmashRule { author: String::new(), updated: 0, ..r.clone() };
                    if strip(&ours[i]) == strip(tr) {
                        self.report.duplicates.push(tr.id.clone());
                    } else if self.conflict(&tr.id, &tr.id, ours[i].updated, tr.updated) {
                        ours[i] = tr.clone();
                    }
                }
                None => {
                    self.report.added.push(tr.id.clone());
                    ours.push(tr.clone());
                }
            }
        }
    }

    fn repoint_placement(&self, p: &mut Placement) {
        if let Some(w) = &mut p.wave {
            w.script = self.ours(&w.script).to_string();
            w.wave = self.ours(&w.wave).to_string();
        }
    }

    /// Points everything that names one of their matched records at ours. Only their ids are
    /// in the map, and none of them is one of our ids naming something else, so all of ours can
    /// be rewritten.
    fn repoint(&self, notes: &mut NotesFile, placements: &mut [Placement], walls: &mut [TriggerWall]) {
        for p in placements {
            self.repoint_placement(p);
        }
        for w in walls {
            if let WallAction::Script { script: Some(s), wave } = &mut w.action {
                *s = self.ours(s).to_string();
                if let Some(wv) = wave {
                    *wv = self.ours(wv).to_string();
                }
            }
        }
        for w in notes.scripts.iter_mut().flat_map(|s| s.waves.iter_mut()) {
            match &mut w.trigger {
                Some(Trigger::HealthBelow { placement: Some(id), .. }) | Some(Trigger::EnterArea { note: Some(id) }) => {
                    *id = self.ours(id).to_string();
                }
                Some(Trigger::WaveCleared { wave }) => *wave = self.ours(wave).to_string(),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Note, NoteKind, Origin, Review, WaveRef};

    fn crate_at(id: &str, pos: [f32; 3], updated: u64) -> Placement {
        Placement {
            id: id.into(),
            kind: Kind::Smashable,
            name: "Crate".into(),
            monster_data: Some(152417),
            template: Some(153171),
            level: None,
            pos,
            heading: 0.0,
            target: None,
            replaces: None,
            removed: false,
            wave: None,
            patrol: Vec::new(),
            comment: String::new(),
            tags: Vec::new(),
            feature: crate::features::live(),
            author: "x".into(),
            updated,
        }
    }

    fn set_with(placements: Vec<Placement>) -> Set {
        let mut s = Set::default();
        s.placements_mut(1200).placements = placements;
        s
    }

    fn ids(s: &Set) -> Vec<&str> {
        s.placements.get(&1200).map_or(Vec::new(), |f| f.placements.iter().map(|p| p.id.as_str()).collect())
    }

    #[test]
    fn duplicates_and_near_duplicates_are_skipped() {
        let mut ours = set_with(vec![crate_at("p-a-1", [0.0; 3], 1)]);
        let theirs = set_with(vec![
            crate_at("p-a-1", [0.0; 3], 9),         // the same record, saved later
            crate_at("p-b-1", [0.5, 0.0, 0.5], 1),  // 0.7 m away
            crate_at("p-b-2", [3.0, 0.0, 0.0], 1),  // too far
        ]);
        let r = merge(&mut ours, &theirs, &Options::default());
        assert_eq!(r.duplicates, ["p-a-1"]);
        assert_eq!(r.near.len(), 1);
        assert_eq!(r.near[0].theirs, "p-b-1");
        assert_eq!(r.added, ["p-b-2"]);
        assert_eq!(ids(&ours), ["p-a-1", "p-b-2"]);
    }

    #[test]
    fn bouncers_with_other_targets_are_not_near_duplicates() {
        let pad = |id: &str, target: [f32; 3], updated| Placement { kind: Kind::Bouncer, monster_data: None, template: Some(148906), target: Some(target), ..crate_at(id, [0.0; 3], updated) };
        let mut ours = set_with(vec![pad("p-a-1", [10.0, 0.0, 0.0], 1)]);
        let theirs = set_with(vec![
            pad("p-b-1", [10.3, 0.0, 0.0], 1), // the same pad, nearly the same landing point
            pad("p-b-2", [20.0, 0.0, 0.0], 1), // the same spot, another landing point
        ]);
        let r = merge(&mut ours, &theirs, &Options::default());
        assert_eq!(r.near.len(), 1);
        assert_eq!(r.near[0].theirs, "p-b-1");
        assert_eq!(r.added, ["p-b-2"]);
        // the same record with a moved target: a conflict, the newer wins
        let mut ours = set_with(vec![pad("p-a-1", [10.0, 0.0, 0.0], 1)]);
        merge(&mut ours, &set_with(vec![pad("p-a-1", [12.0, 0.0, 0.0], 5)]), &Options::default());
        assert_eq!(ours.placements[&1200].placements[0].target, Some([12.0, 0.0, 0.0]));
    }

    #[test]
    fn tolerance_take_and_heading() {
        let theirs = set_with(vec![crate_at("p-b-1", [0.5, 0.0, 0.5], 1)]);
        let tight = Options { tolerance: Tolerance { distance: 0.5, heading: 20.0 }, ..Default::default() };
        let mut ours = set_with(vec![crate_at("p-a-1", [0.0; 3], 1)]);
        assert_eq!(merge(&mut ours, &theirs, &tight).added, ["p-b-1"]);

        let take = Options { take: ["p-b-1".to_string()].into(), ..Default::default() };
        let mut ours = set_with(vec![crate_at("p-a-1", [0.0; 3], 1)]);
        assert_eq!(merge(&mut ours, &theirs, &take).added, ["p-b-1"]);

        let mut turned = crate_at("p-b-1", [0.5, 0.0, 0.5], 1);
        turned.heading = 1.0; // 57 degrees
        let mut ours = set_with(vec![crate_at("p-a-1", [0.0; 3], 1)]);
        assert_eq!(merge(&mut ours, &set_with(vec![turned]), &Options::default()).added, ["p-b-1"]);
    }

    #[test]
    fn conflicts_keep_the_newer_and_deletes_stay_deleted() {
        let mut deleted = crate_at("p-a-1", [0.0; 3], 5);
        deleted.removed = true;
        let mut ours = set_with(vec![deleted.clone()]);
        let r = merge(&mut ours, &set_with(vec![crate_at("p-a-1", [0.0; 3], 2)]), &Options::default());
        assert_eq!(r.conflicts[0].kept, Side::Ours);
        assert!(ours.placements[&1200].placements[0].removed);

        let mut ours = set_with(vec![crate_at("p-a-1", [0.0; 3], 2)]);
        let r = merge(&mut ours, &set_with(vec![deleted]), &Options::default());
        assert_eq!(r.conflicts[0].kept, Side::Theirs);
        assert!(ours.placements[&1200].placements[0].removed);
    }

    #[test]
    fn the_same_guess_replaced_twice() {
        let origin = Origin { id: "g-1".into(), kind: Kind::Smashable, monster_data: Some(152417), template: Some(153171), pos: [0.0; 3] };
        let replacing = |id: &str, pos, updated| Placement { replaces: Some(origin.clone()), ..crate_at(id, pos, updated) };
        // close: one placement
        let mut ours = set_with(vec![replacing("p-a-1", [0.0; 3], 1)]);
        let r = merge(&mut ours, &set_with(vec![replacing("p-b-1", [0.3, 0.0, 0.0], 1)]), &Options::default());
        assert_eq!(r.near.len(), 1);
        // far apart: a conflict; theirs is newer and takes our id
        let mut ours = set_with(vec![replacing("p-a-1", [0.0; 3], 1)]);
        let r = merge(&mut ours, &set_with(vec![replacing("p-b-1", [5.0, 0.0, 0.0], 3)]), &Options::default());
        assert_eq!(r.conflicts[0].kept, Side::Theirs);
        assert_eq!(ids(&ours), ["p-a-1"]);
        assert_eq!(ours.placements[&1200].placements[0].pos, [5.0, 0.0, 0.0]);
    }

    #[test]
    fn smash_rules() {
        let rule = |id: &str, enemies: &[&str], updated| SmashRule {
            id: id.into(),
            enemies: enemies.iter().map(|e| e.to_string()).collect(),
            once: false,
            item: None,
            comment: String::new(),
            tags: Vec::new(),
            feature: crate::features::live(),
            author: String::new(),
            updated,
        };
        let mut ours = Set { smash_rules: vec![rule("2163", &["Skeleton"], 1), rule("2164", &["Zombie"], 5)], ..Default::default() };
        let theirs = Set { smash_rules: vec![rule("2163", &["Skeleton"], 9), rule("2164", &["Ghost"], 2), rule("847", &["Hades"], 1)], ..Default::default() };
        let r = merge(&mut ours, &theirs, &Options::default());
        assert_eq!((r.duplicates.as_slice(), r.added.as_slice()), (&["2163".to_string()][..], &["847".to_string()][..]));
        assert_eq!((r.conflicts[0].playfield, r.conflicts[0].kept), (ACHIEVEMENTS, Side::Ours), "ours is newer");
        assert_eq!(ours.smash_rule("2164").unwrap().enemies, ["Zombie"]);
        let r = merge(&mut ours, &theirs, &Options { prefer: Prefer::Theirs, ..Default::default() });
        assert_eq!(r.conflicts[0].kept, Side::Theirs);
        assert_eq!(ours.smash_rule("2164").unwrap().enemies, ["Ghost"]);
        assert_eq!(ours.smash_rules.len(), 3);
        assert!(ours.placements.is_empty() && ours.notes.is_empty());
    }

    #[test]
    fn trigger_walls() {
        let wall = |id: &str, x: f32, action: WallAction| TriggerWall {
            id: id.into(),
            name: String::new(),
            pos: [x, 0.0, 0.0],
            heading: 0.0,
            size: [4.0, 3.0, 1.0],
            radius: None,
            color: "red".into(),
            action,
            once: false,
            comment: String::new(),
            removed: false,
            tags: Vec::new(),
            feature: crate::features::live(),
            author: String::new(),
            updated: 0,
        };
        let discover = || WallAction::Discover { name: Some("Treasure Cove".into()), achievement: None };
        let mut ours = Set::default();
        ours.placements_mut(1200).triggers.push(wall("t-a", 0.0, discover()));
        let mut theirs = Set::default();
        theirs.placements_mut(1200).triggers = vec![
            wall("t-a", 0.0, discover()),                                   // duplicate
            wall("t-b", 0.4, discover()),                                   // the same wall
            wall("t-c", 0.4, WallAction::Message { text: "hi".into() }), // does something else
        ];
        let r = merge(&mut ours, &theirs, &Options::default());
        assert_eq!((r.duplicates.as_slice(), r.near.len(), r.added.as_slice()), (&["t-a".to_string()][..], 1, &["t-c".to_string()][..]));
        assert_eq!(ours.placements[&1200].triggers.len(), 2);
    }

    #[test]
    fn matched_scripts_take_their_waves_and_placements() {
        let script = |id: &str, waves: Vec<Wave>| Script {
            id: id.into(),
            name: "Dock ambush".into(),
            area: Some(Area::Circle { center: [10.0, 0.0, 10.0], radius: 5.0 }),
            reference: None,
            comment: String::new(),
            waves,
            status: Review::Unreviewed,
            tags: Vec::new(),
            feature: crate::features::live(),
            author: String::new(),
            updated: 0,
        };
        let wave = |id: &str, name: &str, trigger| Wave { id: id.into(), name: name.into(), trigger, comment: String::new(), tags: Vec::new() };
        let mut ours = Set::default();
        ours.notes_mut(1200).scripts.push(script("s-a", vec![wave("w-a1", "First skeletons", None)]));
        ours.notes_mut(1200).notes.push(Note {
            id: "n-a".into(),
            kind: NoteKind::Script,
            area: Area::Point([10.0, 0.0, 10.0]),
            reference: None,
            comment: "dock".into(),
            status: Review::Unreviewed,
            tags: Vec::new(),
            feature: crate::features::live(),
            author: String::new(),
            updated: 0,
        });

        let mut theirs = Set::default();
        theirs.notes_mut(1200).scripts.push(script(
            "s-b",
            vec![
                wave("w-b1", "first skeletons", Some(Trigger::EnterArea { note: Some("n-b".into()) })),
                wave("w-b2", "captain", Some(Trigger::WaveCleared { wave: "w-b1".into() })),
            ],
        ));
        theirs.notes_mut(1200).notes.push(Note { id: "n-b".into(), ..ours.notes[&1200].notes[0].clone() });
        let mut captain = crate_at("p-b-1", [10.0, 0.0, 10.0], 1);
        captain.wave = Some(WaveRef { script: "s-b".into(), wave: "w-b2".into() });
        let mut skeleton = crate_at("p-b-2", [11.0, 0.0, 11.0], 1);
        skeleton.wave = Some(WaveRef { script: "s-b".into(), wave: "w-b1".into() });
        theirs.placements_mut(1200).placements = vec![captain, skeleton];

        let r = merge(&mut ours, &theirs, &Options::default());
        let n = &ours.notes[&1200];
        assert_eq!(n.scripts.len(), 1);
        assert_eq!(n.notes.len(), 1);
        let waves = &n.scripts[0].waves;
        assert_eq!(waves.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["w-a1", "w-b2"]);
        assert_eq!(waves[1].trigger, Some(Trigger::WaveCleared { wave: "w-a1".into() }));
        let refs: Vec<_> = ours.placements[&1200].placements.iter().map(|p| p.wave.clone().unwrap()).collect();
        assert_eq!(refs[0], WaveRef { script: "s-a".into(), wave: "w-b2".into() });
        assert_eq!(refs[1], WaveRef { script: "s-a".into(), wave: "w-a1".into() });
        assert_eq!(r.added, ["w-b2", "p-b-1", "p-b-2"]);
    }
}
