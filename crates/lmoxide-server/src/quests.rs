//! Quests (content/quests, docs/PROTOCOL.md "Quests"). Area quests run here: entering a quest's
//! area (the client's `Quest_*` bounded areas, role `quest`) starts it, or resumes it where the
//! player left off; leaving pauses it (the tracker hides it); progress is kept. What counts
//! toward a goal is the goal's `counts` (as achievements count kills, smashes and builds) and
//! `enemies`; completing the last step starts the quest's cooldown, after which entering the
//! area starts it again. Story quests are content only: the journal still lists every story
//! line as done (teleport.rs `journal_record`), which the teleport windows and World Map need.
//!
//! Records, as LMO_Captures.cap showed them and the client's writers lay them out:
//! - start: `MainQuestUpdateIIR_c` (add) then `QuestFullUpdateIIR` (the quest with its goal:
//!   the bar's maximum and current count, the minimap arrow);
//! - progress: `QuestGoalIIR` (quest, goal, 2, 0, added to the bar);
//! - leaving / coming back: `QuestUpdateIIR` (quest, 3 pause / 4 resume, 0);
//! - a step done: `QuestGoalIIR` (.., 1, ..), `QuestIIR_c` 2 (completed), `JournalUpdateIIR`,
//!   `QuestIIR_c` 4 (removed), then the next step's `QuestFullUpdateIIR`;
//! - the last step done: the same, then `QuestCooldownIIR` and `MainQuestUpdateIIR_c` (remove).
//!
//! Live's reward (a loot bag and an item at completion, QuestRewardUpdateIIR) isn't given yet.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, RwLock};

use bytes::Bytes;
use lmoxide_content::{Quest, QuestKind, Shape};
use lmoxide_net::{Writer, iir, types::Id32};
use tracing::info;

use crate::achievements::Event;
use crate::store::{Character, QuestState};

/// How far above or below an area's heights still counts as inside (m).
const HEIGHT_SLACK: f32 = 5.0;
/// The client's MainQuestType: a story line, an area quest.
const MAIN_TYPE_STORY: u32 = 5;
const MAIN_TYPE_AREA: u32 = 1;
/// Quest flags of an area quest and a story quest (QuestFullUpdateIIR, recorded).
const FLAGS_AREA: u32 = 0x4000_0002;
/// TDB categories: main quest names, quest solved texts, goal texts.
const TEXT_MAIN_QUEST: u32 = 50300;
const TEXT_SOLVED: u32 = 50303;
const TEXT_GOAL: u32 = 50304;
/// "No name" for a goal without a target (recorded).
const NO_NAME: &str = r#"<localized token="Noname" category="100" flags="" >"#;
/// The resource type of quest rewards, and the reward resources whose decryption keys the
/// capture holds (the rest can't be shown).
const REWARD_RESOURCE: u32 = 1_000_620;
const REWARD_KEYS: &[(u32, [u8; 16])] = &[
    (160_904, *b"\xb1\x2b\x4d\xf6\x15\x92\x21\x04\x45\xd6\x87\x1d\xde\x89\x46\x8e"),
    (584_677, *b"\x5e\xf8\xbc\x68\xc9\x5f\xd8\xba\xf9\x1f\x33\x1d\x90\xd7\x14\xea"),
    (525_783, *b"\xec\xd0\xa0\x0f\x1c\x82\xe2\x7c\x82\xc9\xcd\xcc\x6a\x67\x93\xdd"),
    (462_486, *b"\x4f\x1e\xa9\x46\x84\x0c\xb5\x31\xca\xcb\xfb\x13\xfa\x60\x24\xe1"),
    (462_445, *b"\x1e\x70\xe0\x6b\x4c\x91\x25\x10\x36\x9d\x38\xb0\x8e\x95\x9b\x12"),
];
/// The second key of every recorded reward entry.
const REWARD_KEY2: [u8; 16] = *b"\xb2\x90\x80\x5c\x29\xe6\x27\xca\xb2\x90\x80\x5c\x29\xe6\x27\xca";

/// An area quest the server runs.
struct AreaQuest {
    quest: Quest,
    main: u32,
    playfield: u32,
    area: Shape,
}

#[derive(Default)]
struct Index {
    by_playfield: HashMap<u32, Vec<usize>>,
    quests: Vec<AreaQuest>,
}

static INDEX: LazyLock<RwLock<Index>> = LazyLock::new(|| {
    let c = crate::gamedata::get();
    RwLock::new(build(c.quests.values().cloned(), &|id| c.areas.get(id).and_then(|a| a.shape.clone())))
});

/// The playfield in an area id (`area/4200/quest-native`).
fn area_playfield(id: &str) -> Option<u32> {
    id.split('/').nth(1)?.parse().ok()
}

fn build(quests: impl Iterator<Item = Quest>, area: &dyn Fn(&str) -> Option<Shape>) -> Index {
    let mut index = Index::default();
    for q in quests {
        let (Some(main), Some(area_id)) = (q.main, q.area.clone()) else { continue };
        if q.kind != QuestKind::Area || q.steps.is_empty() {
            continue;
        }
        let (Some(playfield), Some(shape)) = (area_playfield(&area_id), area(&area_id)) else { continue };
        index.by_playfield.entry(playfield).or_default().push(index.quests.len());
        index.quests.push(AreaQuest { quest: q, main, playfield, area: shape });
    }
    index
}

/// The world editor's work changed (or was loaded): the quests and their areas as it has them
/// (content.rs, survey content_changed).
pub fn set_from_work(work: &lmoxide_content::work::Work) {
    use lmoxide_content::id::RecordKind;
    let live = |kind: RecordKind| {
        work.records()
            .filter(move |(k, _)| k.id.kind == kind && k.feature == "live")
            .map(|(_, e)| e.value.clone())
            .collect::<Vec<_>>()
    };
    let quests: Vec<Quest> = live(RecordKind::Quest).into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
    let areas: HashMap<String, Shape> = live(RecordKind::Area)
        .into_iter()
        .filter_map(|v| serde_json::from_value::<lmoxide_content::Area>(v).ok())
        .filter_map(|a| Some((a.id, a.shape?)))
        .collect();
    let index = build(quests.into_iter(), &|id| areas.get(id).cloned());
    info!(quests = index.quests.len(), "area quests");
    *INDEX.write().unwrap() = index;
}

/// What the session knows about the quests shown in the client (cleared on a playfield change:
/// the client's tracker forgets them).
#[derive(Default)]
pub struct Tracker {
    playfield: u32,
    /// Main quests in the client's tracker -> paused (the player left the area).
    shown: BTreeMap<u32, bool>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn player(character: u32) -> Id32 {
    Id32::new(crate::territory::CHARACTER_TYPE, character)
}

/// A string as the client writes them (NUL-terminated).
fn cstr(w: Writer, s: &str) -> Writer {
    if s.is_empty() {
        return w.u16(0);
    }
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    w.string_bytes(&b)
}

fn localized(category: u32, id: u32) -> String {
    format!(r#"<localized id="{id}" category="{category}" flags="" >"#)
}

/// Whether the quest can start now.
fn available(q: &AreaQuest, state: Option<&QuestState>, now: i64) -> bool {
    match state {
        None => true,
        Some(s) if s.cooldown_until > now => false,
        Some(s) => s.completions == 0 || q.quest.repeatable || s.step > 0 || s.progress > 0,
    }
}

/// The player moved to `pos` on `playfield`: starts, resumes or pauses the area quests there.
pub fn moved(t: &mut Tracker, c: &mut Character, character: u32, playfield: u32, pos: [f32; 3]) -> Vec<Bytes> {
    if t.playfield != playfield {
        t.playfield = playfield;
        t.shown.clear();
    }
    let index = INDEX.read().unwrap();
    let Some(list) = index.by_playfield.get(&playfield) else { return Vec::new() };
    let now = unix_now();
    let mut records = Vec::new();
    for &i in list {
        let q = &index.quests[i];
        let inside = q.area.contains(pos, HEIGHT_SLACK);
        match (inside, t.shown.get(&q.main).copied()) {
            (true, None) if available(q, c.quests.get(&q.main), now) => {
                let state = c.quests.entry(q.main).or_insert_with(|| QuestState { started_at: now, ..Default::default() });
                if state.completed_at > 0 && state.step == 0 && state.progress == 0 {
                    state.started_at = now;
                }
                let fresh = state.step == 0 && state.progress == 0;
                info!(character, main = q.main, quest = %q.quest.id, step = state.step, progress = state.progress, "area quest started");
                records.push(main_quest_added(character, q, state.step));
                records.push(full_update(character, q, state, fresh));
                t.shown.insert(q.main, false);
            }
            (true, Some(true)) => {
                if let Some(step) = c.quests.get(&q.main).and_then(|s| q.quest.steps.get(s.step as usize)) {
                    records.push(quest_update(character, step.quest, 4));
                }
                t.shown.insert(q.main, false);
            }
            (false, Some(false)) => {
                if let Some(step) = c.quests.get(&q.main).and_then(|s| q.quest.steps.get(s.step as usize)) {
                    records.push(quest_update(character, step.quest, 3));
                }
                t.shown.insert(q.main, true);
            }
            _ => {}
        }
    }
    records
}

/// Whether `event` counts toward `goal`.
fn counts(goal: &lmoxide_content::QuestGoal, event: &Event) -> bool {
    let (name, ok) = match (goal.counts.as_deref(), event) {
        (Some("kills"), Event::Defeated { name, smashable: false, .. }) => (name, true),
        (Some("smashes"), Event::Defeated { name, smashable: true, .. }) => (name, true),
        (Some("kills_or_smashes"), Event::Defeated { name, .. }) => (name, true),
        (Some("champions"), Event::Defeated { name, boss, champion, .. }) => (name, *boss || *champion),
        (Some("builds"), Event::Built { name }) => (name, true),
        _ => return false,
    };
    if !ok {
        return false;
    }
    if goal.enemies.is_empty() {
        return true;
    }
    let lower = name.to_lowercase();
    crate::achievements::expand_enemies(&goal.enemies).iter().any(|e| crate::achievements::same_name(e, &lower))
}

/// Counts `event` (`times` times) for the quests in the tracker.
pub fn on_event(t: &mut Tracker, c: &mut Character, character: u32, event: &Event, times: u64) -> Vec<Bytes> {
    let index = INDEX.read().unwrap();
    let Some(list) = index.by_playfield.get(&t.playfield) else { return Vec::new() };
    let now = unix_now();
    let mut records = Vec::new();
    for &i in list {
        let q = &index.quests[i];
        let Some(&paused) = t.shown.get(&q.main) else { continue };
        let Some(state) = c.quests.get_mut(&q.main) else { continue };
        let Some(step) = q.quest.steps.get(state.step as usize) else { continue };
        let Some(goal) = step.goals.iter().find(|g| g.counts.is_some()) else { continue };
        if (paused && !goal.anywhere) || !counts(goal, event) {
            continue;
        }
        let n = (times.min(u32::MAX as u64) as u32).min(goal.count.saturating_sub(state.progress));
        if n == 0 {
            continue;
        }
        state.progress += n;
        info!(character, main = q.main, progress = state.progress, count = goal.count, "quest progress");
        if state.progress < goal.count {
            records.push(goal_record(character, step.quest, goal.goal, 2, goal.step() * n));
            continue;
        }
        // the step is done
        records.push(goal_record(character, step.quest, goal.goal, 1, 0));
        records.push(quest_record(character, 2, step.quest));
        records.push(journal_record(character, q, step.quest, state.completions));
        records.push(quest_record(character, 4, step.quest));
        state.step += 1;
        state.progress = 0;
        if (state.step as usize) < q.quest.steps.len() {
            records.push(full_update(character, q, state, false));
            continue;
        }
        // the last step: the quest is done
        state.step = 0;
        state.completions += 1;
        state.completed_at = now;
        let cooldown = q.quest.cooldown.map_or(0, |d| d.0.round() as i64);
        state.cooldown_until = if q.quest.repeatable && cooldown > 0 { now + cooldown } else { 0 };
        info!(character, main = q.main, quest = %q.quest.id, completions = state.completions, cooldown, "area quest completed");
        if let (Some(id), true) = (q.quest.cooldown_id, cooldown > 0) {
            records.push(cooldown_record(character, id, cooldown as u32));
        }
        records.push(main_quest_removed(character, q.main));
        t.shown.remove(&q.main);
    }
    records
}

/// The completed area quests for the journal at login: (main quest, its quests, times done).
pub fn completed(c: &Character) -> Vec<(u32, Vec<u32>, u32)> {
    let index = INDEX.read().unwrap();
    c.quests
        .iter()
        .filter(|(_, s)| s.completions > 0)
        .filter_map(|(&main, s)| {
            let q = index.quests.iter().find(|q| q.main == main)?;
            Some((main, q.quest.steps.iter().map(|st| st.quest).collect(), s.completions))
        })
        .collect()
}

/// Test command: the character's quests (in progress, cooling down, done).
pub fn summary(c: &Character) -> Vec<String> {
    let index = INDEX.read().unwrap();
    let now = unix_now();
    let mut lines = vec![format!("{} area quests run here ({} with progress or done)", index.quests.len(), c.quests.len())];
    for (main, s) in &c.quests {
        let q = index.quests.iter().find(|q| q.main == *main);
        let name = q.map_or_else(|| main.to_string(), |q| q.quest.name.clone());
        let goal = q.and_then(|q| q.quest.steps.get(s.step as usize)).and_then(|st| st.goals.iter().find(|g| g.counts.is_some()));
        let progress = goal.map_or_else(|| format!("{}", s.progress), |g| format!("{}/{}", s.progress, g.count));
        let cooldown = if s.cooldown_until > now { format!(", again in {} s", s.cooldown_until - now) } else { String::new() };
        lines.push(format!("{name}: step {} {progress}, done {} times{cooldown}", s.step + 1, s.completions));
    }
    lines
}

/// Test targets: each area quest's area (label, playfield, its middle).
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    let index = INDEX.read().unwrap();
    let mut out: Vec<_> = index.quests.iter().map(|q| (q.quest.name.clone(), q.playfield, q.area.centre())).collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    out
}

// ------------------------------------------------------------------ records

/// `MainQuestUpdateIIR_c` add: u32 1, main, current quest, its quests with their order (a
/// countdown: the first the highest), the name, u32 1, MainQuestType, u32 1.
fn main_quest_added(character: u32, q: &AreaQuest, step: u32) -> Bytes {
    let steps = &q.quest.steps;
    let current = steps.get(step as usize).map_or(0, |s| s.quest);
    iir::record("MainQuestUpdateIIR_c", player(character), true, |w| {
        let w = w.u32(1).u32(q.main).u32(current).container_len(steps.len());
        let w = steps.iter().enumerate().fold(w, |w, (i, s)| w.u32(s.quest).u8((steps.len() - 1 - i) as u8));
        let kind = if q.quest.kind == QuestKind::Story { MAIN_TYPE_STORY } else { MAIN_TYPE_AREA };
        cstr(w, &localized(TEXT_MAIN_QUEST, q.main)).u32(1).u32(kind).u32(1)
    })
}

/// `MainQuestUpdateIIR_c` remove.
fn main_quest_removed(character: u32, main: u32) -> Bytes {
    iir::record("MainQuestUpdateIIR_c", player(character), true, |w| cstr(w.u32(2).u32(main).u32(0).container_len(0), "").u32(0).u32(0).u32(0))
}

/// One reward entry of a quest (64 bytes), if its key is known.
fn reward(w: Writer, character: u32, quest: u32, id: u32) -> Writer {
    let key = REWARD_KEYS.iter().find(|(r, _)| *r == id).map(|(_, k)| *k).unwrap_or_default();
    w.u32(character).u32(quest).u32(REWARD_RESOURCE).u32(id).raw(&key).u32(REWARD_RESOURCE).u32(0).raw(&REWARD_KEY2).u32(1).u32(1)
}

/// `QuestFullUpdateIIR`: u32 1, owner, the quest (quests::operator<<), u8 show the "new quest"
/// popup.
fn full_update(character: u32, q: &AreaQuest, state: &QuestState, popup: bool) -> Bytes {
    let step = &q.quest.steps[state.step as usize];
    let first = q.quest.steps[0].quest;
    let rewards: Vec<u32> = q.quest.rewards.iter().map(|r| r[0]).filter(|id| REWARD_KEYS.iter().any(|(r, _)| r == id)).collect();
    iir::record("QuestFullUpdateIIR", player(character), false, |w| {
        let w = w.u32(1).u32(character);
        // the quest
        let w = w.u32(1).u32(step.quest).u32(1).u32(0).u32(1).u32(FLAGS_AREA).u32(1).u32(q.main).u32(first).u8(1);
        let w = w.u32(0).u32(0).u32(0).u32(0).u32(1).u32(0).u64(0).u32(0);
        let w = rewards.iter().fold(w.u32(rewards.len() as u32), |w, &id| reward(w, character, step.quest, id));
        let w = w.u32(0).u32(0).u64(0).u32(0).u32(0);
        // its goals
        let w = step.goals.iter().fold(w.u32(step.goals.len() as u32), |w, g| {
            let counted = g.counts.is_some() && step.goals.iter().find(|x| x.counts.is_some()).is_some_and(|x| x.goal == g.goal);
            let solved = if counted { (state.progress * g.step()).min(g.bar_max()) } else { 0 };
            let w = w.u32(1).u32(g.kind.client_type()).u32(character).u32(step.quest).u32(g.goal).u32(1).u32(0).u32(g.bar_max()).u32(solved);
            let w = (0..3).fold(w, |w, _| w.u32(0).u32(0));
            let w = match &g.marker {
                Some(m) => w.u32(0).u32(q.playfield).f32(m.pos[0]).f32(m.pos[2]).f32(m.radius).f32(m.pos[1]),
                None => w.u32(0).u32(0).u32(0).u32(0).u32(0).u32(0),
            };
            let w = w.u8(0).u8(0).u8(0);
            let text = format!(r#"<remoteformat id="{}" category="{TEXT_GOAL}" knubot="0"  ></remoteformat>"#, g.goal);
            cstr(cstr(w, &text), NO_NAME).u32(0)
        });
        w.u8(popup as u8)
    })
}

/// `QuestGoalIIR`: u32 quest, goal, event (1 done, 2 add to the bar), 0, amount.
fn goal_record(character: u32, quest: u32, goal: u32, event: u32, amount: u32) -> Bytes {
    iir::record("QuestGoalIIR", player(character), true, |w| w.u32(quest).u32(goal).u32(event).u32(0).u32(amount))
}

/// `QuestIIR_c`: u32 1, event (2 completed, 4 removed), quest, 0.
fn quest_record(character: u32, event: u32, quest: u32) -> Bytes {
    iir::record("QuestIIR_c", player(character), false, |w| w.u32(1).u32(event).u32(quest).u32(0))
}

/// `QuestUpdateIIR`: u32 quest, type (3 pause, 4 resume), 0.
fn quest_update(character: u32, quest: u32, kind: u32) -> Bytes {
    iir::record("QuestUpdateIIR", player(character), true, |w| w.u32(quest).u32(kind).u32(0))
}

/// `QuestCooldownIIR`: u32 2, [(cooldown id, seconds left)].
fn cooldown_record(character: u32, id: u32, seconds: u32) -> Bytes {
    iir::record("QuestCooldownIIR", player(character), true, |w| w.u32(2).container_len(1).u32(id).u32(seconds))
}

/// `JournalUpdateIIR` for a completed quest: a new main quest the first time (the journal at
/// login lists the earlier ones), else the quest added to it.
fn journal_record(character: u32, q: &AreaQuest, quest: u32, done_before: u32) -> Bytes {
    use lmoxide_net::proto::{
        JournalUpdateIirProto,
        journal_update_iir_proto::{AddedMainQuestType, AddedQuestListType, AddedQuestType},
    };
    let entry = AddedQuestType {
        quest_id: Some(quest),
        failed: Some(false),
        repeat_count: Some(done_before),
        solved_text: Some(localized(TEXT_SOLVED, quest)),
        ..Default::default()
    };
    let msg = if done_before == 0 && q.quest.steps.first().is_some_and(|s| s.quest == quest) {
        JournalUpdateIirProto { added_main_quest: vec![AddedMainQuestType { main_quest_id: Some(q.main), quest: vec![entry], ..Default::default() }], ..Default::default() }
    } else {
        JournalUpdateIirProto { added_quest: vec![AddedQuestListType { main_quest_id: Some(q.main), quest: vec![entry] }], ..Default::default() }
    };
    iir::record("JournalUpdateIIR", player(character), false, |w| w.proto(&msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmoxide_content::QuestGoal;

    fn quest(goal: QuestGoal) -> Quest {
        serde_json::from_value(serde_json::json!({
            "id": "quest/test", "name": "Free the pirate prisoners", "main": 41, "kind": "area",
            "area": "area/4200/quest-native", "repeatable": true, "cooldown": "1h", "cooldown_id": 14,
            "rewards": [[160904, 30046]],
            "steps": [{ "quest": 55, "goals": [goal] }],
        }))
        .unwrap()
    }

    fn goal() -> QuestGoal {
        serde_json::from_value(serde_json::json!({
            "goal": 93, "kind": "trigger", "counts": "smashes", "count": 8, "max": 100,
            "marker": { "pos": [176.8, 74.0, 398.3], "radius": 20.0 },
        }))
        .unwrap()
    }

    fn setup() -> (Tracker, Character) {
        let square = Shape::Polygon { points: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], heights: [0.0, 10.0] };
        let index = build(std::iter::once(quest(goal())), &|_| Some(square.clone()));
        *INDEX.write().unwrap() = index;
        (Tracker::default(), crate::game::new_character("t".into(), 4200))
    }

    fn smash(name: &str) -> Event {
        Event::Defeated { id: Id32::default(), name: name.into(), smashable: true, boss: false, champion: false, level: 1, playfield: 4200 }
    }

    fn names(records: &[Bytes]) -> Vec<String> {
        records.iter().map(|r| iir::name(u32::from_be_bytes(r[..4].try_into().unwrap())).unwrap_or("?").to_string()).collect()
    }

    // (INDEX is shared: one test runs the whole flow)
    #[test]
    fn area_quest_flow() {
        let (mut t, mut c) = setup();
        // outside: nothing; inside: started
        assert!(moved(&mut t, &mut c, 7, 4200, [20.0, 5.0, 20.0]).is_empty());
        let r = moved(&mut t, &mut c, 7, 4200, [5.0, 5.0, 5.0]);
        assert_eq!(names(&r), ["MainQuestUpdateIIR_c", "QuestFullUpdateIIR"]);
        // as recorded (character 0x01000cb8) up to the goal's texts
        const RECORDED: &str = "0000000101000cb8000000010000003700000001000000000000000140000002000000010000002900000037010000000000000000000000000000000000000001000000000000000000000000000000000000000101000cb800000037000f44ac00027488b12b4df61592210445d6871dde89468e000f44ac00000000b290805c29e627cab290805c29e627ca000000010000000100000000000000000000000000000000000000000000000000000001000000010000001601000cb8000000370000005d0000000100000000000000640000000000000000000000000000000000000000000000000000000000000000000010684330cccd43c7266641a0000042940000000000";
        let mut c2 = crate::game::new_character("t".into(), 4200);
        let r2 = moved(&mut Tracker::default(), &mut c2, 0x0100_0cb8, 4200, [5.0, 5.0, 5.0]);
        let hex: String = r2[1][13..13 + RECORDED.len() / 2].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, RECORDED);
        // the main quest: as recorded up to its name
        let hex: String = r2[0][13..13 + 21].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "000000010000002900000037000007e20000003700");
        // a smash: 13 of 100
        let r = on_event(&mut t, &mut c, 7, &smash("Cage"), 1);
        assert_eq!(names(&r), ["QuestGoalIIR"]);
        assert_eq!(&r[0][13..], &[0, 0, 0, 55, 0, 0, 0, 93, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 13][..]);
        // a kill doesn't count
        let kill = Event::Defeated { id: Id32::default(), name: "Cage".into(), smashable: false, boss: false, champion: false, level: 1, playfield: 4200 };
        assert!(on_event(&mut t, &mut c, 7, &kill, 1).is_empty());
        // leaving pauses, progress stays; smashes outside don't count
        assert_eq!(names(&moved(&mut t, &mut c, 7, 4200, [50.0, 5.0, 5.0])), ["QuestUpdateIIR"]);
        assert!(on_event(&mut t, &mut c, 7, &smash("Cage"), 1).is_empty());
        assert_eq!(names(&moved(&mut t, &mut c, 7, 4200, [5.0, 5.0, 5.0])), ["QuestUpdateIIR"]);
        assert_eq!(c.quests[&41].progress, 1);
        // a new session (or playfield): shown again with its progress
        let mut t2 = Tracker::default();
        let r = moved(&mut t2, &mut c, 7, 4200, [5.0, 5.0, 5.0]);
        assert_eq!(names(&r), ["MainQuestUpdateIIR_c", "QuestFullUpdateIIR"]);
        // the other 7: done, cooldown, removed
        let r = on_event(&mut t2, &mut c, 7, &smash("Cage"), 6);
        assert_eq!(names(&r), ["QuestGoalIIR"]);
        let r = on_event(&mut t2, &mut c, 7, &smash("Cage"), 1);
        assert_eq!(
            names(&r),
            ["QuestGoalIIR", "QuestIIR_c", "JournalUpdateIIR", "QuestIIR_c", "QuestCooldownIIR", "MainQuestUpdateIIR_c"]
        );
        let s = &c.quests[&41];
        assert_eq!((s.completions, s.step, s.progress), (1, 0, 0));
        assert!(s.cooldown_until > unix_now() + 3500);
        // cooling down: not started again
        assert!(moved(&mut t2, &mut c, 7, 4200, [5.0, 5.0, 5.0]).is_empty());
        c.quests.get_mut(&41).unwrap().cooldown_until = 0;
        assert_eq!(names(&moved(&mut t2, &mut c, 7, 4200, [5.0, 5.0, 5.0])), ["MainQuestUpdateIIR_c", "QuestFullUpdateIIR"]);
        assert_eq!(completed(&c), [(41, vec![55], 1)]);
    }

    #[test]
    fn steps() {
        let g = goal();
        assert_eq!((g.bar_max(), g.step()), (100, 13));
        let three: QuestGoal = serde_json::from_value(serde_json::json!({ "goal": 1, "count": 3, "max": 100 })).unwrap();
        assert_eq!(three.step(), 34);
        let kills: QuestGoal = serde_json::from_value(serde_json::json!({ "goal": 1, "count": 25 })).unwrap();
        assert_eq!((kills.bar_max(), kills.step()), (25, 1));
    }
}
