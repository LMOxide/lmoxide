//! NPC dialogues ("Knubot"), as the original server ran them (recorded session, client
//! ReadSubClass layouts). The texts are the client's TDB category 50002 (data/dialogues.json,
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
//! (`gameactiononnpc` 599417) go to tutorial.rs; quest tags (`spawn`, `resolvegoal`) and other
//! server scripts are logged, not run.

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
    /// where the speaker stood (live capture)
    #[serde(default)]
    pos: Option<[f32; 3]>,
}

static DATA: LazyLock<Data> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../data/dialogues.json")).expect("dialogues.json"));

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
const ARRIVALS: &[(u32, Option<u32>, Duration)] = &[(79, None, Duration::from_millis(880)), (822, Some(322), Duration::ZERO)];
/// How long after a dialogue's close the one that follows it starts (live: 0.46 s).
const FOLLOW_DELAY: Duration = Duration::from_millis(460);

/// Players in a dialogue: its generation, where their answers go, and who speaks.
static TALKING: LazyLock<Mutex<HashMap<Id32, (u64, mpsc::UnboundedSender<u32>, Id32)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Per player: how many of each NPC's dialogues they've had (this session).
static PROGRESS: LazyLock<Mutex<HashMap<(Id32, String, u32), usize>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The recorded dialogues (for `!dialog`).
pub fn dialogues() -> &'static [Dialogue] {
    &DATA.dialogues
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
fn tags_given(d: &Dialogue) -> Vec<u32> {
    d.texts
        .iter()
        .filter_map(|t| DATA.texts.get(&t.to_string()))
        .flat_map(|text| tags(text))
        .filter_map(|tag| tag.strip_prefix("addtagtoplayer ").and_then(|t| t.trim().parse().ok()))
        .collect()
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
pub fn arrived(player: Id32, player_name: &str, playfield: u32, has_tag: &dyn Fn(u32) -> bool) {
    let Some((d, delay)) = arrival(playfield, has_tag) else { return };
    let (d, name) = (d.clone(), player_name.to_string());
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        if world::whereabouts(player).is_some_and(|(pf, _)| pf == d.playfield) {
            let owner = crate::npc::find(&d.npc, d.playfield).unwrap_or(player);
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
pub fn talk(player: Id32, player_name: &str, npc: Id32, name: &str, playfield: u32) {
    let list = of(name, playfield);
    if list.is_empty() {
        return;
    }
    let n = {
        let mut progress = PROGRESS.lock().unwrap();
        let n = progress.entry((player, name.to_string(), playfield)).or_insert(0);
        let i = (*n).min(list.len() - 1);
        *n += 1;
        i
    };
    start(player, player_name, npc, list[n].clone());
}

/// Starts `dialogue` for `player` with `owner` as the speaking NPC (the player itself for the
/// cinematics whose owner isn't spawned).
pub fn start(player: Id32, player_name: &str, owner: Id32, dialogue: Dialogue) {
    let (tx, rx) = mpsc::unbounded_channel();
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if TALKING.lock().unwrap().insert(player, (generation, tx, owner)).is_some() {
        info!(?player, "dialogue replaced");
    }
    info!(?player, npc = %dialogue.npc, texts = ?dialogue.texts, "dialogue");
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
            if !TALKING.lock().unwrap().contains_key(&player) && world::whereabouts(player).is_some_and(|(pf, _)| pf == next.playfield) {
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
    let speaker = Speaker { player, owner, name, npc_name: d.npc_name, generation };
    let send = |records: Vec<Bytes>| speaker.send(records);
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
        cleared = run_text(&speaker, d.texts[i]).await;
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
                    cleared = run_text(&speaker, t).await;
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
    send(records);
    ended
}

struct Speaker {
    player: Id32,
    owner: Id32,
    name: String,
    npc_name: Option<u32>,
    generation: u64,
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

/// Runs text `id`'s tags; returns whether its last tag cleared the cinematic.
async fn run_text(s: &Speaker, id: u32) -> bool {
    let Some(text) = DATA.texts.get(&id.to_string()) else {
        warn!(id, "dialogue text missing");
        return false;
    };
    let send = |records: Vec<Bytes>| s.send(records);
    if !text.split('{').next().unwrap_or("").trim().is_empty() {
        send(vec![append(s, id, 0)]);
    }
    let mut cleared = false;
    for (n, tag) in tags(text).into_iter().enumerate() {
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
            ["setactiveactor", a, ..] => records.push(actor(s.player, msg(Kind::SetActiveActor, a))),
            ["moveactortopos", a, p, ..] => {
                records.push(actor(s.player, KnubotActorMessage { instant_teleport: Some(false), ..placed(Kind::MoveActorToPos, a, p) }));
            }
            ["startfxpackageonactor", a, package, ..] => {
                records.push(actor(s.player, KnubotActorMessage { effect_package: package.parse().ok(), ..msg(Kind::StartEffectPackage, a) }));
            }
            ["clearcinematic", ..] => {
                records.push(actor(s.player, msg(Kind::ClearCinematicState, "")));
                cleared = true;
            }
            ["addcinematicactor", alias, template, point, ..] => records.push(spawn_client_npc(s.player, alias, template.parse().unwrap_or(0), point)),
            ["delay", secs, ..] => tokio::time::sleep(Duration::from_secs_f32(secs.parse().unwrap_or(1.0))).await,
            ["addtagtoplayer", tag, ..] => {
                if let Ok(tag) = tag.parse() {
                    world::send_to(s.player, world::Out::Progress(crate::achievements::Event::Tag(tag)));
                }
            }
            [cmd @ ("teleportplayer" | "teleportplayernosplash"), args @ ..] => match teleport_args(args) {
                Some((playfield, at, rot)) => world::send_to(s.player, world::Out::Teleport(playfield, at, rot)),
                None => warn!(%cmd, ?args, "dialogue teleport not understood"),
            },
            // the tutorial's server script (599417) with its step name
            ["gameactiononnpc", _, "gameaction_value", "599417", "gameaction_string0", step, ..] if step.starts_with("String1=") => {
                crate::tutorial::script(s.player, &step["String1=".len()..]);
            }
            _ => info!(?s.player, %tag, "dialogue tag not run"),
        }
        records.push(append(s, id, n as u32 + 1));
        send(records);
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
        assert_eq!(DATA.dialogues.len(), 15);
        assert!(DATA.dialogues[..8].iter().all(|d| d.source == "demo" && d.start.is_none()));
        assert!(DATA.dialogues[8..].iter().all(|d| d.source == "live" && d.start.is_some()));
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
    fn teleport_tags() {
        assert_eq!(teleport_args(&["220.531", "255.793", "y", "16", "4610"]), Some((Some(4610), [220.531, 16.0, 255.793], 0.0)));
        let (pf, at, _) = teleport_args(&["88.3", "256.0", "y", "24.5", "r", "250"]).unwrap();
        assert_eq!((pf, at), (None, [88.3, 24.5, 256.0]));
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
