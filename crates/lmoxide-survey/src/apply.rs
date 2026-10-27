//! Base content + placements → what a playfield spawns, each with its category (docs/SURVEY.md
//! "Content categories"). The server passes its own spawn definitions as `T` and gets them back.

use serde::Serialize;

use crate::{Kind, Origin, Placement, Vec3, distance, ids};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Live,
    Demo,
    Client,
    Guessed,
    Surveyed,
}

impl Category {
    /// From `world_content.json`'s `source` (`proposed`, `live`, `demo`, `rdb_*`, `cine_point`).
    pub fn from_source(source: Option<&str>) -> Category {
        match source {
            Some("proposed") => Category::Guessed,
            Some("live") => Category::Live,
            Some("demo") => Category::Demo,
            _ => Category::Client,
        }
    }

    /// Only guesses can be confirmed, edited or deleted; everything else can only be hidden.
    pub fn editable(self) -> bool {
        matches!(self, Category::Guessed | Category::Surveyed)
    }
}

/// An object of the base content.
#[derive(Debug, Clone)]
pub struct Base<T> {
    pub kind: Kind,
    pub monster_data: Option<u32>,
    pub template: Option<u32>,
    pub pos: Vec3,
    pub category: Category,
    pub data: T,
}

impl<T> Base<T> {
    pub fn id(&self, playfield: u32) -> String {
        ids::base_id(playfield, self.kind, self.monster_data, self.template, self.pos)
    }

    /// The [`Origin`] a placement replacing this object saves.
    pub fn origin(&self, playfield: u32) -> Origin {
        Origin { id: self.id(playfield), kind: self.kind, monster_data: self.monster_data, template: self.template, pos: self.pos }
    }
}

#[derive(Debug)]
pub enum Spawn<'a, T> {
    /// A base object as it is.
    Base { id: String, category: Category, data: &'a T },
    /// A placement, and the guess it replaces (whose data can fill in what the placement lacks:
    /// spells, NPC template, ...).
    Surveyed { placement: &'a Placement, replaced: Option<&'a T> },
}

#[derive(Debug)]
pub struct Applied<'a, T> {
    /// Spawned when the playfield starts.
    pub start: Vec<Spawn<'a, T>>,
    /// Spawned by their wave's trigger.
    pub waves: Vec<&'a Placement>,
    /// Placements that replace a guess no longer in the base content (not spawned, reported).
    pub orphans: Vec<&'a Placement>,
}

/// Applies (merged) placements to a playfield's base content. A placement replaces the guess
/// its `replaces` names; if no guess has that id any more (the guesses were regenerated), the
/// nearest guess of the same kind and look within `match_distance` of the saved position.
pub fn apply<'a, T>(playfield: u32, base: &'a [Base<T>], placements: &'a [Placement], match_distance: f32) -> Applied<'a, T> {
    let ids: Vec<String> = base.iter().map(|b| b.id(playfield)).collect();
    let mut replaced: Vec<Option<&'a Placement>> = vec![None; base.len()];
    let mut orphans = Vec::new();
    for p in placements {
        let Some(origin) = &p.replaces else { continue };
        let guess = |i: &usize| base[*i].category == Category::Guessed;
        let found = (0..base.len()).find(|i| guess(i) && ids[*i] == origin.id).or_else(|| {
            (0..base.len())
                .filter(|i| guess(i) && replaced[*i].is_none())
                .filter(|i| base[*i].kind == origin.kind && base[*i].monster_data == origin.monster_data && base[*i].template == origin.template)
                .map(|i| (i, distance(base[i].pos, origin.pos)))
                .filter(|(_, d)| *d <= match_distance)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        });
        match found {
            Some(i) => replaced[i] = replaced[i].or(Some(p)),
            None => orphans.push(p),
        }
    }
    let mut start = Vec::new();
    for (i, b) in base.iter().enumerate() {
        if replaced[i].is_none() {
            start.push(Spawn::Base { id: ids[i].clone(), category: b.category, data: &b.data });
        }
    }
    let mut waves = Vec::new();
    for p in placements {
        if p.removed || orphans.iter().any(|o| std::ptr::eq(*o, p)) {
            continue;
        }
        if p.wave.is_some() {
            waves.push(p);
            continue;
        }
        let replaced = p.replaces.as_ref().and_then(|_| replaced.iter().position(|r| r.is_some_and(|r| std::ptr::eq(r, p)))).map(|i| &base[i].data);
        start.push(Spawn::Surveyed { placement: p, replaced });
    }
    Applied { start, waves, orphans }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WaveRef;

    fn base(pos: Vec3, category: Category, data: &'static str) -> Base<&'static str> {
        Base { kind: Kind::Enemy, monster_data: Some(82410), template: None, pos, category, data }
    }

    fn placement(id: &str, pos: Vec3, replaces: Option<Origin>) -> Placement {
        Placement {
            id: id.into(),
            kind: Kind::Enemy,
            name: String::new(),
            monster_data: Some(82410),
            template: None,
            level: None,
            pos,
            heading: 0.0,
            replaces,
            removed: false,
            wave: None,
            patrol: Vec::new(),
            comment: String::new(),
            author: String::new(),
            updated: 0,
        }
    }

    fn names<'a>(a: &Applied<'a, &'static str>) -> Vec<String> {
        a.start
            .iter()
            .map(|s| match s {
                Spawn::Base { data, category, .. } => format!("{data}:{category:?}"),
                Spawn::Surveyed { placement, replaced } => format!("{}<{}", placement.id, replaced.map_or("", |r| r)),
            })
            .collect()
    }

    #[test]
    fn confirm_edit_delete_and_add() {
        let b = vec![
            base([0.0; 3], Category::Guessed, "guess-a"),
            base([10.0, 0.0, 0.0], Category::Guessed, "guess-b"),
            base([20.0, 0.0, 0.0], Category::Guessed, "guess-c"),
            base([30.0, 0.0, 0.0], Category::Live, "live"),
        ];
        let confirmed = placement("p-1", [0.0; 3], Some(b[0].origin(1200)));
        let edited = placement("p-2", [12.0, 0.0, 0.0], Some(b[1].origin(1200)));
        let deleted = Placement { removed: true, ..placement("p-3", [20.0, 0.0, 0.0], Some(b[2].origin(1200))) };
        let new = placement("p-4", [40.0, 0.0, 0.0], None);
        let mut waved = placement("p-5", [41.0, 0.0, 0.0], None);
        waved.wave = Some(WaveRef { script: "s".into(), wave: "w".into() });
        let not_live = placement("p-6", [30.0, 0.0, 0.0], Some(b[3].origin(1200)));
        let ps = vec![confirmed, edited, deleted, new, waved, not_live];
        let a = apply(1200, &b, &ps, 1.0);
        assert_eq!(names(&a), ["live:Live", "p-1<guess-a", "p-2<guess-b", "p-4<"]);
        assert_eq!(a.waves.len(), 1);
        assert_eq!(a.orphans.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["p-6"]);
    }

    #[test]
    fn regenerated_guesses_are_found_by_look_and_place() {
        let old = base([0.0; 3], Category::Guessed, "old");
        let p = placement("p-1", [2.0, 0.0, 0.0], Some(old.origin(1200)));
        let moved = vec![base([0.4, 0.0, 0.0], Category::Guessed, "moved")];
        assert_eq!(names(&apply(1200, &moved, std::slice::from_ref(&p), 1.0)), ["p-1<moved"]);
        let gone = vec![base([5.0, 0.0, 0.0], Category::Guessed, "far")];
        let a = apply(1200, &gone, std::slice::from_ref(&p), 1.0);
        assert_eq!(names(&a), ["far:Guessed"]);
        assert_eq!(a.orphans.len(), 1);
    }
}
