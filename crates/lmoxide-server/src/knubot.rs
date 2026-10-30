//! NPC dialogues ("Knubot"), as the original server ran them (recorded session, client
//! ReadSubClass layouts). The texts are the client's TDB category 50002 (content texts.json,
//! tools/re/extract_dialogues.py); their `{tags}` are the server's script: for each tag in
//! order the server does it (a cinematic actor record, a spawn, a real delay) and then sends
//! the text again with `antitag` = the number of tags done, so the client shows the words up
//! to there. Text before the first tag goes out first with antitag 0. After a text the player
//! gets the answers: the next text's line (50001, the answer whose wording is that line) and
//! "Goodbye"; a following text without a line of its own is the epilogue run on goodbye.
//!
//! Which NPC says what, and when, was server-side: only the recorded session's dialogues are
//! known (Video Game Guy in the tutorial and Pirate World, two cinematics), and the live
//! capture's (tools/re/live_dialogue.py: the same ones, and the LEGO Center's and Monster
//! Laboratory's Monster Scientist, Swashbuckler in the jail, the jail escape on Isle of Yarr and
//! the Island Warrior's rescue). The live server started those itself, none on a click:
//! `ARRIVALS` are the ones it started on the player's arrival, and a dialogue that followed
//! another (`follows`) runs when that one ends. The others are only for `!dialog`. `addtagtoplayer`
//! gives the tag (achievements), `teleportplayer` teleports, the tutorial's script steps
//! (`gameactiononnpc` 599417) go to tutorial.rs (and the story's cinematics), quest tags
//! (`spawn`, `resolvegoal`) to story.rs; other server scripts are logged, not run.
//!
//! The rest of the texts were rebuilt into conversations (`source: guess`,
//! tools/content/story_dialogues.py, docs/TUNABLES.md "Rebuilt conversations"): story.rs starts
//! them from trigger areas, a boss's defeat (`defeated`) or a cinematic's end, under their
//! `requires` / `unless` / `goals`; they run as the recorded ones.

use crate::instance::Pf;
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use bytes::Bytes;
use lmoxide_net::{
    iir,
    proto::{
        knubot_actor_iir_proto::{KnubotActorIirType as Kind, KnubotActorMessage},
        spawn_client_npc_proto::SpawnClientNpcMessage,
        ProtoQuat, ProtoVec3,
    },
    types::Id32,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::world;

#[derive(Deserialize)]
struct Data {
    texts: HashMap<String, String>,
    answers: HashMap<String, u32>,
    keys: HashMap<String, String>,
    dialogues: Vec<Dialogue>,
}

#[derive(Deserialize, Clone)]
pub struct Dialogue {
    #[serde(default)]
    npc_name: Option<u32>,
    pub npc: String,
    pub playfield: u32,
    texts: Vec<u32>,
    /// "demo" (the recorded session) or "live" (the live capture)
    #[serde(default)]
    pub source: String,
    /// for the live capture's own: what the server started it on ("arrival", "after_dialogue",
    /// "quest_goal", "server"); the demo's are started by their triggers or a click
    #[serde(default)]
    start: Option<String>,
    /// the first text of the dialogue this one followed
    #[serde(default)]
    follows: Option<u32>,
    /// where the speaker stood (live capture); for the rebuilt ones where they start
    #[serde(default)]
    pos: Option<[f32; 3]>,
    /// its content id (`dialogue/...`)
    #[serde(default)]
    pub id: String,
    /// rebuilt ones started by a defeat (`start: boss`): the NPC's name, or `boss`
    #[serde(default)]
    pub defeated: Option<String>,
    /// tags the player must have / must not have, story goals resolved
    #[serde(default)]
    pub requires: Vec<u32>,
    #[serde(default)]
    pub unless: Vec<u32>,
    #[serde(default)]
    pub goals: Vec<String>,
}

/// The dialogues and the client's texts (content/dialogues, texts.json).
static DATA: LazyLock<Data> =
    LazyLock::new(|| serde_json::from_value(lmoxide_content::compat::dialogues_file(crate::gamedata::get())).expect("dialogues"));

/// "Goodbye" (50001/0).
const GOODBYE: u32 = 0;
const ANSWER_CATEGORY: u32 = 50001;
const TEXT_CATEGORY: u32 = 50002;
const NAME_CATEGORY: u32 = 51000;
/// How long the server waits for an answer before closing the window.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);
/// The live capture's dialogues the server started on the player's arrival (CharInPlay): the
/// first text, the tag the player needs first, and how long after the arrival it started. Each
/// runs until the player has the tags it gives. Left out: the LEGO Center's Monster Scientist
/// (1025), whose text runs "if it HAS ticket" (a Monster Laboratory ticket, checked how is
/// unknown). The jail escape (822) needs the jail's last tag (322, text 229): the capture
/// arrived on Isle of Yarr from the jail.
const ARRIVALS: &[(u32, Option<u32>, Duration)] = &[(79, None, Duration::from_millis(8800)), (822, Some(322), Duration::ZERO)];
/// How long after a dialogue's close the one that follows it starts (live: 4.6 s).
const FOLLOW_DELAY: Duration = Duration::from_millis(4600);
/// A text that spawns a cinematic actor (`addcinematicactor`) and then makes it the speaker
/// (`setactiveactor`, as 206's Officer, 822's Swashbuckler, 598's Hades) sends both at once; the
/// client hasn't built the actor yet and the window's portrait falls back to the owner's (the
/// default "DJ" icon). The `setactiveactor` is sent again this long after, without holding up
/// the text's other tags.
const ACTIVE_ACTOR_RESEND: Duration = Duration::from_millis(300);

/// Players in a dialogue: its generation, where their answers go, and who speaks.
static TALKING: LazyLock<Mutex<HashMap<Id32, (u64, mpsc::UnboundedSender<u32>, Id32)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Per player: how many of each NPC's dialogues they've had (this session).
static PROGRESS: LazyLock<Mutex<HashMap<(Id32, String, u32), usize>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The recorded dialogues (for `!dialog`).
pub fn dialogues() -> &'static [Dialogue] {
    &DATA.dialogues
}

impl Dialogue {
    /// Its first text: what names it (`starting_with`).
    pub fn first_text(&self) -> Option<u32> {
        self.texts.first().copied()
    }

    /// Its first line as the player reads it (camera tags and the speaker's name left out).
    pub fn first_line(&self) -> String {
        let text = self.first_text().and_then(|t| DATA.texts.get(&t.to_string())).map_or("", String::as_str);
        let mut out = String::new();
        let mut depth = 0;
        for c in text.chars() {
            match c {
                '{' | '[' => depth += 1,
                '}' | ']' => depth = (depth - 1).max(0),
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// The dialogue with content id `id` (`dialogue/...`).
pub fn by_id(id: &str) -> Option<Dialogue> {
    DATA.dialogues.iter().find(|d| d.id == id).cloned()
}

/// Whether the player is in a dialogue now.
pub fn talking(player: Id32) -> bool {
    TALKING.lock().unwrap().contains_key(&player)
}

/// The rebuilt dialogues a defeat of `name` (`boss`: a boss) on `playfield` starts.
pub fn after_defeat(name: &str, boss: bool, playfield: u32) -> Vec<String> {
    DATA.dialogues
        .iter()
        .filter(|d| d.playfield == playfield && d.defeated.as_deref().is_some_and(|n| n == name || (boss && n == "boss")))
        .map(|d| d.id.clone())
        .collect()
}

/// Whether it's one of the rebuilt story conversations (tools/content/story_dialogues.py).
pub fn rebuilt(d: &Dialogue) -> bool {
    d.source == "guess"
}

/// The rebuilt conversations and where they start (for `!goto rebuilt`).
pub fn rebuilt_points() -> Vec<(String, u32, [f32; 3])> {
    DATA.dialogues
        .iter()
        .filter(|d| rebuilt(d))
        .filter_map(|d| {
            let start = match (&d.start, &d.defeated) {
                (_, Some(n)) => format!("after defeating {n}"),
                (Some(s), _) => s.clone(),
                _ => String::new(),
            };
            Some((format!("{} {} ({start})", d.id.trim_start_matches("dialogue/"), d.first_line().chars().take(40).collect::<String>()), d.playfield, d.pos?))
        })
        .collect()
}

/// The recorded dialogue that starts with text `first`.
pub fn starting_with(first: u32) -> Option<Dialogue> {
    DATA.dialogues.iter().find(|d| d.texts.first() == Some(&first)).cloned()
}

/// The dialogues `npc` (by name) has on `playfield`, in order (the ones a click starts: not the
/// live capture's, which the server started itself).
fn of(npc: &str, playfield: u32) -> Vec<&'static Dialogue> {
    DATA.dialogues.iter().filter(|d| d.npc == npc && d.playfield == playfield && d.start.is_none()).collect()
}

/// The tags a dialogue's texts give (`addtagtoplayer`).
pub fn tags_given(d: &Dialogue) -> Vec<u32> {
    d.texts.iter().flat_map(|&t| text_tags(t)).collect()
}

/// The tags dialogue text `id` gives (`addtagtoplayer`).
pub fn text_tags(id: u32) -> Vec<u32> {
    DATA.texts
        .get(&id.to_string())
        .map(|text| tags(text))
        .unwrap_or_default()
        .iter()
        .filter_map(|tag| tag.strip_prefix("addtagtoplayer ").and_then(|t| t.trim().parse().ok()))
        .collect()
}

/// Every tag a dialogue text gives (`addtagtoplayer`): the story's lore flags.
pub fn all_text_tags() -> std::collections::BTreeSet<u32> {
    DATA.texts.keys().filter_map(|k| k.parse().ok()).flat_map(text_tags).collect()
}

/// The story goals a dialogue's texts resolve (`resolvegoal`).
#[cfg(test)]
pub fn goals_resolved(d: &Dialogue) -> Vec<String> {
    let words = |id: &u32| DATA.texts.get(&id.to_string()).map(|t| tags(t)).unwrap_or_default();
    d.texts.iter().flat_map(words).filter_map(|t| t.strip_prefix("resolvegoal ").map(|h| h.trim().to_string())).collect()
}

/// Every dialogue text that resolves story goals: (the tags it gives, the goals it resolves).
pub fn goal_texts() -> Vec<(Vec<u32>, Vec<String>)> {
    DATA.texts
        .values()
        .filter_map(|text| {
            let words: Vec<String> = tags(text);
            let given: Vec<u32> = words.iter().filter_map(|t| t.strip_prefix("addtagtoplayer ").and_then(|t| t.trim().parse().ok())).collect();
            let goals: Vec<String> = words.iter().filter_map(|t| t.strip_prefix("resolvegoal ").map(|h| h.trim().to_string())).collect();
            (!goals.is_empty()).then_some((given, goals))
        })
        .collect()
}

/// Runs dialogue text `id`'s server side without a window (a story beat whose conversation
/// isn't known): its tags, story goals, teleports and script steps; camera and actor tags are
/// left out.
pub fn beat(player: Id32, id: u32) {
    let Some(text) = DATA.texts.get(&id.to_string()) else { return warn!(id, "story beat: no such text") };
    info!(?player, id, "story beat");
    for tag in tags(text) {
        let words: Vec<&str> = tag.split_whitespace().collect();
        server_tag(player, &tag, &words);
    }
}

/// A text's tag that the server runs (not the client's camera and actors): true if it was one.
fn server_tag(player: Id32, tag: &str, words: &[&str]) -> bool {
    match words {
        ["addtagtoplayer", tag, ..] => {
            if let Ok(tag) = tag.parse() {
                world::send_to(player, world::Out::Progress(crate::achievements::Event::Tag(tag)));
            }
        }
        ["resolvegoal", hash, ..] => crate::story::send(player, crate::story::Order::Goal { hash: hash.to_string(), requires: Vec::new() }),
        // a quest by its hash starts it; other hashes are NPC templates (not known)
        ["spawn", hash, ..] => crate::story::send(player, crate::story::Order::Spawn(hash.to_string())),
        [cmd @ ("teleportplayer" | "teleportplayernosplash"), args @ ..] => match teleport_args(args) {
            Some((playfield, at, rot)) => world::send_to(player, world::Out::Teleport(playfield, at, rot)),
            None => warn!(%cmd, ?args, "dialogue teleport not understood"),
        },
        // the server script 599417 with its step name (the tutorial's, the story's cinematics)
        ["gameactiononnpc", _, "gameaction_value", "599417", "gameaction_string0", step, ..] if step.starts_with("String1=") => {
            crate::tutorial::script(player, &step["String1=".len()..]);
        }
        _ => {
            info!(?player, %tag, "dialogue tag not run");
            return false;
        }
    }
    true
}

/// The dialogue the server starts on arrival at `playfield` for a player with the tags
/// `has_tag` says, and its delay.
fn arrival(playfield: u32, has_tag: &dyn Fn(u32) -> bool) -> Option<(&'static Dialogue, Duration)> {
    ARRIVALS.iter().find_map(|&(first, needs, delay)| {
        let d = DATA.dialogues.iter().find(|d| d.texts.first() == Some(&first) && d.playfield == playfield)?;
        let given = tags_given(d);
        (needs.is_none_or(|t| has_tag(t)) && !given.is_empty() && !given.iter().all(|&t| has_tag(t))).then_some((d, delay))
    })
}

/// The live dialogue that followed the one starting with `first`.
fn following(first: u32) -> Option<&'static Dialogue> {
    DATA.dialogues.iter().find(|d| d.follows == Some(first) && d.start.as_deref() == Some("after_dialogue"))
}

/// `player` arrived on `playfield`: the dialogue the live server started then, if any (its
/// speaker: that NPC if it's here, else the player, as for the cinematics).
pub fn arrived(player: Id32, player_name: &str, playfield: Pf, has_tag: &dyn Fn(u32) -> bool) {
    let Some((d, delay)) = arrival(playfield.model(), has_tag) else { return };
    let (d, name) = (d.clone(), player_name.to_string());
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        if world::whereabouts(player).is_some_and(|(pf, _)| pf == playfield) {
            let owner = crate::npc::find(&d.npc, playfield).unwrap_or(player);
            start(player, &name, owner, d);
        }
    });
}

/// The live capture's dialogues and where their speakers stood (for `!goto dialogue`).
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    DATA.dialogues
        .iter()
        .filter(|d| d.source == "live")
        .filter_map(|d| {
            let at = d.pos?;
            let start = d.start.as_deref().unwrap_or("");
            Some((format!("{} {} ({start})", d.npc, d.texts.first()?), d.playfield, at))
        })
        .collect()
}

/// Whether `npc` (by name) has dialogues on `playfield`.
pub fn has_dialogues(npc: &str, playfield: u32) -> bool {
    !of(npc, playfield).is_empty()
}

/// `player` talks to NPC `npc` (named `name`): its next dialogue (the last one again once all
/// are done).
pub fn talk(player: Id32, player_name: &str, npc: Id32, name: &str, playfield: Pf) {
    let list = of(name, playfield.model());
    if list.is_empty() {
        return;
    }
    let n = {
        let mut progress = PROGRESS.lock().unwrap();
        let n = progress.entry((player, name.to_string(), playfield.model())).or_insert(0);
        let i = (*n).min(list.len() - 1);
        *n += 1;
        i
    };
    start(player, player_name, npc, list[n].clone());
}

/// Starts `dialogue` for `player` with `owner` as the speaking NPC: an NPC, a cinematic's
/// invisible owner (spawns.rs `cinematic_owners`), or the player itself for the rebuilt
/// cinematics whose owner the content doesn't place.
pub fn start(player: Id32, player_name: &str, owner: Id32, dialogue: Dialogue) {
    let (tx, rx) = mpsc::unbounded_channel();
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if TALKING.lock().unwrap().insert(player, (generation, tx, owner)).is_some() {
        info!(?player, "dialogue replaced");
    }
    info!(?player, npc = %dialogue.npc, ?owner, texts = ?dialogue.texts, "dialogue");
    let name = player_name.to_string();
    tokio::spawn(async move {
        let first = dialogue.texts.first().copied();
        let ended = run(player, owner, name.clone(), dialogue, rx, generation).await;
        let current = {
            let mut talking = TALKING.lock().unwrap();
            let current = talking.get(&player).is_some_and(|(g, ..)| *g == generation);
            if current {
                talking.remove(&player);
            }
            current
        };
        // the live server started the next one itself (the jail: Swashbuckler's second)
        if let Some(next) = first.and_then(following).filter(|_| ended && current) {
            tokio::time::sleep(FOLLOW_DELAY).await;
            if !TALKING.lock().unwrap().contains_key(&player) && world::whereabouts(player).is_some_and(|(pf, _)| pf.model() == next.playfield) {

                start(player, &name, owner, next.clone());
            }
        }
    });
}

/// The player answered (KnubotAnswerIIR: u16 1, ID32 npc, u32 answer index).
pub fn answer(player: Id32, index: u32) {
    if let Some((_, tx, _)) = TALKING.lock().unwrap().get(&player) {
        let _ = tx.send(index);
    }
}

/// Ends the player's dialogue, if any (a teleport: its window and cinematic would stay open in
/// the new world): the records that close it.
pub fn end(player: Id32) -> Vec<Bytes> {
    let Some((_, _, owner)) = TALKING.lock().unwrap().remove(&player) else { return Vec::new() };
    let close = remoteformat(100, "0", &key("100/0"), None, None);
    vec![
        actor(player, msg(Kind::ClearCinematicState, "")),
        knubot_record("KnubotCloseChatWindowIIR", player, owner, |w| w.u32(1).string(&close)),
    ]
}

/// The player left.
pub fn left(player: Id32) {
    TALKING.lock().unwrap().remove(&player);
    PROGRESS.lock().unwrap().retain(|(p, _, _), _| *p != player);
}

/// Returns whether the player answered to the end (not a timeout).
async fn run(player: Id32, owner: Id32, name: String, d: Dialogue, mut answers: mpsc::UnboundedReceiver<u32>, generation: u64) -> bool {
    let speaker = Speaker { player, owner, name, npc_name: d.npc_name, generation, hidden: Mutex::default() };
    let send = |records: Vec<Bytes>| speaker.send(records);
    let send: &(dyn Fn(Vec<Bytes>) + Sync) = &send;
    // the cinematic actors "playerActor" and "knubotOwnerActor", then the window
    send(vec![
        actor(player, KnubotActorMessage { actor_reference: Some(id_proto(player)), ..msg(Kind::AddCinematicActorReference, "playerActor") }),
        actor(player, KnubotActorMessage { actor_reference: Some(id_proto(owner)), ..msg(Kind::AddCinematicActorReference, "knubotOwnerActor") }),
        knubot_record("KnubotOpenChatWindowIIR", player, owner, |w| w.u8(0)),
    ]);
    let mut i = 0;
    let mut cleared = false;
    let mut ended = true;
    while i < d.texts.len() {
        cleared = run_text(&speaker, d.texts[i], send).await;
        let next = d.texts.get(i + 1).copied();
        let next_answer = next.and_then(|t| DATA.answers.get(&t.to_string()).copied());
        let options: Vec<u32> = next_answer.into_iter().chain([GOODBYE]).collect();
        send(vec![answer_list(&speaker, &options)]);
        let chosen = tokio::time::timeout(ANSWER_TIMEOUT, answers.recv()).await.ok().flatten();
        match chosen {
            Some(0) if next_answer.is_some() => i += 1,
            Some(_) => {
                // goodbye: a following text without its own answer is the epilogue
                if let Some(t) = next.filter(|_| next_answer.is_none()) {
                    cleared = run_text(&speaker, t, send).await;
                }
                break;
            }
            None => {
                ended = false;
                break;
            }
        }
    }
    let mut records = Vec::new();
    if !cleared {
        records.push(actor(player, msg(Kind::ClearCinematicState, "")));
    }
    let close = remoteformat(100, "0", &key("100/0"), None, None);
    records.push(knubot_record("KnubotCloseChatWindowIIR", player, owner, |w| w.u32(1).string(&close)));
    // the NPCs hidden for their doubles come back
    let hidden: Vec<Id32> = speaker.hidden.lock().unwrap().drain().collect();
    records.extend(hidden.into_iter().filter_map(world::spawn_records_of).flatten());
    send(records);
    ended
}

struct Speaker {
    player: Id32,
    owner: Id32,
    name: String,
    npc_name: Option<u32>,
    generation: u64,
    /// NPCs hidden from the player while their cinematic double is on stage
    hidden: Mutex<std::collections::HashSet<Id32>>,
}

impl Speaker {
    /// Sends to the player while this is its current dialogue (a newer one replaces it).
    fn send(&self, records: Vec<Bytes>) {
        let current = TALKING.lock().unwrap().get(&self.player).is_some_and(|(g, ..)| *g == self.generation);
        if current {
            world::send_to(self.player, world::Out::Records(records));
        }
    }
}

/// Runs text `id`'s tags, its records going to `send`; returns whether its last tag cleared the
/// cinematic.
async fn run_text(s: &Speaker, id: u32, send: &(dyn Fn(Vec<Bytes>) + Sync)) -> bool {
    use tokio::time::{sleep_until, Instant};
    let Some(text) = DATA.texts.get(&id.to_string()) else {
        warn!(id, "dialogue text missing");
        return false;
    };
    // the actors this text spawned, and a `setactiveactor` naming one of them to send again
    // (`ACTIVE_ACTOR_RESEND`)
    let mut spawned = std::collections::HashSet::new();
    let mut resend: Option<(Instant, Bytes)> = None;
    if !text.split('{').next().unwrap_or("").trim().is_empty() {
        send(vec![append(s, id, 0)]);
    }
    let mut cleared = false;
    for (n, tag) in in_play_order(tags(text)).into_iter().enumerate() {
        if let Some((_, r)) = resend.take_if(|(at, _)| *at <= Instant::now()) {
            send(vec![r]);
        }
        let words: Vec<&str> = tag.split_whitespace().collect();
        let mut records = Vec::new();
        cleared = false;
        match words.as_slice() {
            ["changeguivisibility", v, ..] => {
                records.push(actor(s.player, KnubotActorMessage { gui_visibility: v.parse().ok(), ..msg(Kind::ChangeGuiVisibility, "") }));
            }
            ["movecameratoactor", a, rest @ ..] => {
                let follow = rest.first() == Some(&"1");
                records.push(actor(s.player, KnubotActorMessage { follow_actor: Some(follow), ..placed(Kind::MoveCameraToActor, a, "") }));
            }
            ["movecameratopos", p, ..] => records.push(actor(s.player, placed(Kind::MoveCameraToStaticPoint, "", p))),
            ["setactiveactor", a, ..] => {
                let record = actor(s.player, msg(Kind::SetActiveActor, a));
                resend = spawned.contains(*a).then(|| (Instant::now() + ACTIVE_ACTOR_RESEND, record.clone()));
                records.push(record);
            }
            ["moveactortopos", a, p, ..] => {
                // (an NPC placed where the actor walks to, a boss waiting there, is hidden too)
                if let Some(npc) = world::whereabouts(s.player).and_then(|(pf, _)| crate::npc::on_anchor(pf, p, a)) {
                    if s.hidden.lock().unwrap().insert(npc) {
                        records.push(crate::game::despawn_record(npc));
                    }
                }
                records.push(actor(s.player, KnubotActorMessage { instant_teleport: Some(false), ..placed(Kind::MoveActorToPos, a, p) }));
            }
            ["startfxpackageonactor", a, package, ..] => {
                records.push(actor(s.player, KnubotActorMessage { effect_package: package.parse().ok(), ..msg(Kind::StartEffectPackage, a) }));
            }
            ["clearcinematic", ..] => {
                records.push(actor(s.player, msg(Kind::ClearCinematicState, "")));
                cleared = true;
            }
            // a client-side actor at its marker; an NPC the content put on that marker (the
            // client data places story NPCs on the cinematics' markers: the Wizard in the throne
            // room) is hidden from the player meanwhile, else there would be two of them (as an
            // actor reference to the NPC the window loses its portrait and text)
            ["addcinematicactor", alias, template, point, ..] => {
                if let Some(npc) = world::whereabouts(s.player).and_then(|(pf, _)| crate::npc::on_anchor(pf, point, alias)) {
                    if s.hidden.lock().unwrap().insert(npc) {
                        records.push(crate::game::despawn_record(npc));
                    }
                }
                records.push(spawn_client_npc(s.player, alias, template.parse().unwrap_or(0), point));
                spawned.insert(alias.to_string());
            }
            ["removecinematicactor", alias, ..] => records.push(actor(s.player, msg(Kind::RemoveActor, alias))),
            ["delay", secs, ..] => {
                let until = Instant::now() + Duration::from_secs_f32(secs.parse().unwrap_or(1.0));
                if let Some((at, r)) = resend.take_if(|(at, _)| *at <= until) {
                    sleep_until(at).await;
                    send(vec![r]);
                }
                sleep_until(until).await;
            }
            // tags, story goals, teleports, script steps
            _ => {
                server_tag(s.player, &tag, &words);
            }
        }
        records.push(append(s, id, n as u32 + 1));
        send(records);
    }
    if let Some((at, r)) = resend {
        sleep_until(at).await;
        send(vec![r]);
    }
    cleared
}

/// `teleportplayer[nosplash] x z y <height> [r <degrees>] [playfield]` (checked against the
/// Medieval Kingdom's arrival point: `88.3 256.0 y 24.5 r 250` there is (87.9, 24.5, 256.1)).
fn teleport_args(args: &[&str]) -> Option<(Option<u32>, [f32; 3], f32)> {
    let x: f32 = args.first()?.parse().ok()?;
    let z: f32 = args.get(1)?.parse().ok()?;
    let mut height = 0.0;
    let mut rot = 0.0;
    let mut playfield = None;
    let mut rest = args[2..].iter();
    while let Some(word) = rest.next() {
        match *word {
            "y" => height = rest.next()?.parse().ok()?,
            "r" => rot = rest.next()?.parse::<f32>().ok()?.to_radians(),
            n => playfield = n.parse().ok(),
        }
    }
    Some((playfield, [x, height, z], rot))
}

/// A text's tags as they run: a `spawn` before a teleport in the same text runs after it (276
/// gives the Wizard's bag, then teleports out of the throne room: the bag lands at the player's
/// new place, not where they can't go back). The count (`antitag`) stays the text's own.
fn in_play_order(mut tags: Vec<String>) -> Vec<String> {
    let Some(tp) = tags.iter().rposition(|t| t.starts_with("teleportplayer")) else { return tags };
    let spawns: Vec<String> = tags[..tp].iter().filter(|t| t.starts_with("spawn ")).cloned().collect();
    if spawns.is_empty() {
        return tags;
    }
    tags.retain(|t| !(t.starts_with("spawn ") && spawns.contains(t)));
    let tp = tags.iter().rposition(|t| t.starts_with("teleportplayer")).unwrap_or(0);
    tags.splice(tp + 1..tp + 1, spawns);
    tags
}

/// The `{...}` tags of a text, in order.
fn tags(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else { break };
        out.push(rest[start + 1..start + len].trim().to_string());
        rest = &rest[start + len + 1..];
    }
    out
}

fn msg(kind: Kind, alias: &str) -> KnubotActorMessage {
    KnubotActorMessage { version: Some(1), iir_type: Some(kind as i32), actor_string_alias: Some(alias.into()), ..Default::default() }
}

/// An actor message with a static point and a zero position (as recorded).
fn placed(kind: Kind, alias: &str, point: &str) -> KnubotActorMessage {
    KnubotActorMessage { ai_static_point: Some(point.into()), position: Some(zero()), ..msg(kind, alias) }
}

fn zero() -> ProtoVec3 {
    ProtoVec3 { x: Some(0.0), y: Some(0.0), z: Some(0.0) }
}

fn id_proto(id: Id32) -> lmoxide_net::proto::ProtoId32 {
    lmoxide_net::proto::ProtoId32 { r#type: Some(id.kind), instance: Some(id.instance) }
}

/// `KnubotActorIIR` (record flag 1): u32 length + KnubotActorMessage.
fn actor(player: Id32, m: KnubotActorMessage) -> Bytes {
    iir::record("KnubotActorIIR", player, true, |w| w.proto(&m))
}

/// `SpawnClientNPCIIR` (record flag 1): a client-side cinematic actor at an AI point.
fn spawn_client_npc(player: Id32, alias: &str, template: u32, point: &str) -> Bytes {
    let m = SpawnClientNpcMessage {
        version: Some(1),
        item_template: Some(template),
        ai_point_name: Some(point.into()),
        position: Some(zero()),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        cinematic_actor_alias: Some(alias.into()),
        is_cinematic: Some(true),
    };
    iir::record("SpawnClientNPCIIR", player, true, |w| w.proto(&m))
}

/// A Knubot record: u16 version 1, ID32 npc, then `body`.
fn knubot_record(name: &str, player: Id32, npc: Id32, body: impl FnOnce(lmoxide_net::Writer) -> lmoxide_net::Writer) -> Bytes {
    iir::record(name, player, false, |w| body(npc.write(w.u16(1))))
}

/// `KnubotAppendTextIIR`: u32 NPCChatTextType 1, the text's remoteformat (with `antitag`), u8 1,
/// VoiceParameters (u32 0, i32 -1, f32 4, f32 20).
fn append(s: &Speaker, id: u32, antitag: u32) -> Bytes {
    let xml = remoteformat(TEXT_CATEGORY, &id.to_string(), &key(&format!("{TEXT_CATEGORY}/{id}")), Some(antitag), Some(s));
    knubot_record("KnubotAppendTextIIR", s.player, s.owner, |w| w.u32(1).string(&xml).u8(1).u32(0).i32(-1).f32(4.0).f32(20.0))
}

/// `KnubotAnswerListIIR`: u32 count, count × remoteformat (50001 answers).
fn answer_list(s: &Speaker, answers: &[u32]) -> Bytes {
    let xmls: Vec<String> =
        answers.iter().map(|a| remoteformat(ANSWER_CATEGORY, &a.to_string(), &key(&format!("{ANSWER_CATEGORY}/{a}")), None, Some(s))).collect();
    knubot_record("KnubotAnswerListIIR", s.player, s.owner, |w| xmls.iter().fold(w.u32(xmls.len() as u32), |w, x| w.string(x)))
}

fn key(of: &str) -> String {
    DATA.keys.get(of).cloned().unwrap_or_default()
}

/// XML attribute escaping as the original server's (also `'`, `$` and `\`).
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '$' => out.push_str("&#36;"),
            '\\' => out.push_str("&#92;"),
            c => out.push(c),
        }
    }
    out
}

/// A `<remoteformat>` reference to a localized text, with the parameters the recorded server
/// gave dialogue texts and answers (the player's name, the NPC's name, "Stranger" for the rest).
fn remoteformat(category: u32, id: &str, key: &str, antitag: Option<u32>, s: Option<&Speaker>) -> String {
    let antitag = antitag.map_or(" ".to_string(), |a| format!("antitag=\"{a}\" "));
    let head = format!("<remoteformat id=\"{id}\" category=\"{category}\" key=\"{}\" knubot=\"0\" {antitag}>", esc(key));
    let Some(s) = s else { return head + "</remoteformat>" };
    let localized_token = |token: &str| format!("<localized token=\"{token}\" category=\"100\" flags=\"\" key=\"{}\" >", esc(&self::key(&format!("100/{token}"))));
    let stranger = localized_token("Stranger");
    let npc_name = match s.npc_name {
        Some(n) => format!("<localized id=\"{n}\" category=\"{NAME_CATEGORY}\" flags=\"\" key=\"{}\" >", esc(&self::key(&format!("{NAME_CATEGORY}/{n}")))),
        None => stranger.clone(),
    };
    let params = [
        (" a", stranger.clone()),
        (" p", stranger.clone()),
        (" t", stranger.clone()),
        ("a", stranger.clone()),
        ("guildname", "<stringentry string=\"\" flags=\"\" >".to_string()),
        ("name", format!("<stringentry string=\"{}\" flags=\"\" >", esc(&s.name))),
        ("npcguildname", localized_token("StrangerGuild")),
        ("npcname", npc_name),
        ("p", stranger.clone()),
        ("t", stranger),
    ];
    let mut xml = head;
    for (name, value) in params {
        xml.push_str(&format!("<parameter value=\"7;{}\" name=\"{name}\" />", esc(&value)));
    }
    xml + "</remoteformat>"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_and_tags() {
        // the demo recording's 8 first (each also in the live capture), then the live capture's 7
        // (then the rebuilt story conversations, source "guess")
        assert!(DATA.dialogues[..8].iter().all(|d| d.source == "demo" && d.start.is_none()));
        assert!(DATA.dialogues[8..15].iter().all(|d| d.source == "live" && d.start.is_some()));
        assert!(DATA.dialogues[15..].len() > 50 && DATA.dialogues[15..].iter().all(|d| rebuilt(d) && d.start.is_some()));
        assert_eq!(tags(&DATA.texts["742"]), ["changeguivisibility 0", "delay 1", "spawn S1M7 10 m", "clearcinematic"]);
        assert_eq!(DATA.answers.get("743"), Some(&510));
    }

    #[test]
    fn live_dialogues() {
        // the answers the live server offered chain the texts (not only the epilogue)
        assert_eq!(DATA.answers.get("1026"), Some(&765));
        assert_eq!(DATA.answers.get("223"), Some(&136));
        // started by the server, not by a click
        assert!(!has_dialogues("Swashbuckler", 4610));
        assert!(has_dialogues("Video Game Guy", 1300));
        // arriving in the jail: Swashbuckler's first, until its tag (308)
        let none = |_| false;
        assert_eq!(arrival(4610, &none).map(|(d, _)| d.texts[0]), Some(79));
        assert!(arrival(4610, &|t| t == 308).is_none());
        // Isle of Yarr: the escape only after the jail (322), once (4182)
        assert!(arrival(4200, &none).is_none());
        assert_eq!(arrival(4200, &|t| t == 322).map(|(d, _)| d.texts[0]), Some(822));
        assert!(arrival(4200, &|t| t == 322 || t == 4182).is_none());
        // no arrival dialogue for the LEGO Center (ticket check unknown) or the tutorial
        assert!(arrival(1200, &none).is_none() && arrival(1300, &none).is_none());
        assert_eq!(following(79).map(|d| d.texts[0]), Some(224));
        assert!(following(741).is_none());
        assert_eq!(points().len(), 7);
    }

    #[test]
    fn rebuilt_conversations() {
        // the calibration: the Jester at the Ruined Fort says 283, 284, 285 (user's footage); 282
        // is the scene's epilogue (run on goodbye); the answers chain them (285's by alignment)
        let jester = by_id("dialogue/the-realm-of-jest-283").expect("the Jester");
        assert_eq!((jester.texts.as_slice(), jester.playfield), ([283, 284, 285, 282].as_slice(), 5300));
        assert_eq!((DATA.answers.get("284"), DATA.answers.get("285"), DATA.answers.get("282")), (Some(&179), Some(&180), None));
        // every middle text has its answer button (else the conversation would end there)
        for d in DATA.dialogues.iter().filter(|d| rebuilt(d)) {
            let n = d.texts.len();
            for t in &d.texts[1..n - 1] {
                assert!(DATA.answers.contains_key(&t.to_string()), "{} text {t}", d.id);
            }
            // nobody's owner can't start them: no click (start set) and not a recorded first text
            assert!(!of(&d.npc, d.playfield).iter().any(|o| o.id == d.id));
        }
        // after a defeat: by name, or any boss of that playfield
        assert_eq!(after_defeat("Hades", true, 2320), ["dialogue/myth-g12-party-598"]);
        assert_eq!(after_defeat("The Jester", true, 5300), ["dialogue/my-kingdom-for-a-crown-288"]);
        assert!(after_defeat("Troll Warlord", true, 5300).is_empty());
        assert_eq!(after_defeat("Pieromancer Elementius", true, 5310), ["dialogue/defeated-elementius-308"]);
        assert_eq!(after_defeat("T-Rex", true, 6300), ["dialogue/dino-d05-t-rex-defeated-662"]);
        assert!(rebuilt_points().iter().any(|(_, pf, _)| *pf == 5300));
    }

    #[test]
    fn spawns_after_the_teleport() {
        let order = in_play_order(tags(&DATA.texts["276"]));
        let at = |p: &str| order.iter().position(|t| t.starts_with(p)).unwrap();
        assert!(at("spawn 8G4X") > at("teleportplayernosplash"));
        assert_eq!(order.len(), tags(&DATA.texts["276"]).len());
    }

    #[test]
    fn teleport_tags() {
        assert_eq!(teleport_args(&["220.531", "255.793", "y", "16", "4610"]), Some((Some(4610), [220.531, 16.0, 255.793], 0.0)));
        let (pf, at, _) = teleport_args(&["88.3", "256.0", "y", "24.5", "r", "250"]).unwrap();
        assert_eq!((pf, at), (None, [88.3, 24.5, 256.0]));
    }

    /// When each `setactiveactor` of `alias` went out running `text` (time paused: the delays
    /// take no time).
    async fn active_actor_times(text: u32, alias: &str) -> Vec<Duration> {
        let player = Id32::new(50000, 0x7fff_fff0);
        let s = Speaker { player, owner: player, name: "Tester".into(), npc_name: None, generation: 0, hidden: Mutex::default() };
        let set = actor(player, msg(Kind::SetActiveActor, alias));
        let start = tokio::time::Instant::now();
        let sent = Mutex::new(Vec::new());
        let sink = |records: Vec<Bytes>| sent.lock().unwrap().extend(records.into_iter().map(|r| (start.elapsed(), r)));
        run_text(&s, text, &sink).await;
        sent.into_inner().unwrap().into_iter().filter(|(_, r)| *r == set).map(|(t, _)| t).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn new_speaker_named_again() {
        let ms = Duration::from_millis;
        // 206: the Officer spawned after `delay 1`, made the speaker at once and again 300 ms on
        // (during the `delay 2` after it, not after the text)
        assert_eq!(active_actor_times(206, "Officer").await, [ms(1000), ms(1000) + ACTIVE_ACTOR_RESEND]);
        // 822: the Swashbuckler, during the `delay 1`; 598: Hades, at the text's end
        assert_eq!(active_actor_times(822, "Swashbuckler").await, [ms(0), ACTIVE_ACTOR_RESEND]);
        assert_eq!(active_actor_times(598, "Hades").await, [ms(0), ACTIVE_ACTOR_RESEND]);
        // 812: the owner, not spawned by the text: once
        assert_eq!(active_actor_times(812, "knubotOwnerActor").await, [ms(0)]);
    }

    #[test]
    fn close_xml_matches_recording() {
        // recorded: <remoteformat id="0" category="100" key=".fzTuEyiu7B&#92;ZYZMoq1-" knubot="0"  ></remoteformat>
        assert_eq!(
            remoteformat(100, "0", &key("100/0"), None, None),
            "<remoteformat id=\"0\" category=\"100\" key=\".fzTuEyiu7B&#92;ZYZMoq1-\" knubot=\"0\"  ></remoteformat>"
        );
    }
}
