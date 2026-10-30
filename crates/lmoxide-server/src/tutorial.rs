//! Character creation and the tutorial (playfield 1300), as the recorded session shows them: it
//! is a new character being created and playing the whole tutorial (docs/PROTOCOL.md
//! "Character creation and tutorial").
//!
//! Creation (with progression): the launcher's creation screen keeps the chosen name and
//! minifigure set in the client; CreateCharacterWithData sends neither. The character goes to
//! the creation playfield 4000, where the server starts client script 156092
//! (`GameActionListIIR_t`), which makes the client send `SetCharacterCreationData(nick, first,
//! last, set)`. The answer: the set's minifigure, `SetNameIIR_t`,
//! `SignalCreatePlayerCharacterResult(0)`, `TeleportStarted` and a teleport to the tutorial.
//!
//! The tutorial: walking past six invisible trigger NPCs ("Cinematic Start ...") starts Video
//! Game Guy's dialogues; their tags run server script 599417 steps that spawn the Scallywags
//! (`DreadlegSpawnPirate`, `SpawnAmbush`, `SpawnAmbush2`). Video Game Guy escorts the player
//! (live: escort script 597462) and fights along. The last trigger starts the boss music;
//! Captain Dreadleg's defeat stops it and moves the player to the cinematic spot for the last
//! dialogue, which gives tag 4121: the client then runs its world map tutorial, and clicking
//! Pirate World (TeleportWindowBypass 74, 4600) ends the tutorial there.
//!
//! The Crystal Compass (the portal door to Pirate World by Dreadleg's spot) isn't there from
//! the start: Dreadleg's line 767 runs `SpawnCompass`, which shows it to that player and opens
//! its portals for them; until then the door and the portal points do nothing. Players with
//! tag 4121 have it whenever they are back.

use crate::instance::Pf;
use std::{collections::HashSet, sync::LazyLock, time::Duration};

use bytes::Bytes;
use lmoxide_net::{iir, types::Id32};
use serde::Deserialize;
use tracing::info;

use crate::npc::SpawnDef;

pub const PLAYFIELD: u32 = 1300;
/// Where new characters pick their name and minifigure (the client hides its GUI there).
pub const CREATION_PLAYFIELD: u32 = 4000;
/// The tutorial's start (recorded PlayfieldFIIR after creation).
pub const START: [f32; 3] = [67.1, 47.6, 217.9];
/// Where the player watches Dreadleg escape (Cine_Player1).
pub const AFTER_BOSS: [f32; 3] = [260.97, 29.0, 266.24];
/// "Dreadleg Escapes" (story): given by the last dialogue; whoever has it is done.
pub const DONE_TAG: u32 = 4121;
pub const BOSS: &str = "Captain Dreadleg";
/// The Crystal Compass: the door of the portals to Pirate World (spawn/1300/crystal-compass).
pub const COMPASS_DOOR: u32 = 635_924;
const GUIDE: &str = "Video Game Guy";
/// Client script group "Login_SetupChar_Client" (RDB 1010028): opens the creation result path.
const LOGIN_SETUP_SCRIPT: u32 = 156092;
/// The starter minifigure sets (RDB 1010700/1): Bumblebee Girl, Roman Commander, Plumber.
const STARTER_SETS: [u32; 3] = [195928, 317609, 50715];
/// How close a trigger fires (m); the first one (Video Game Guy's door) only right at it.
const TRIGGER_RADIUS: f32 = 6.0;
const DOOR_RADIUS: f32 = 3.5;
/// How long the scripted enemies and the escort stay.
const SPAWN_LIFETIME: Duration = Duration::from_secs(15 * 60);
const ESCORT_LIFETIME: Duration = Duration::from_secs(3 * 60 * 60);

/// Who speaks a trigger's dialogue.
#[derive(Clone, Copy, PartialEq)]
enum Speaker {
    Guide,
    /// a cinematic without a spawned speaker
    Player,
}

/// The triggers (recorded "Cinematic Start" NPCs), the first text of their dialogue and its
/// speaker; the boss trigger has no dialogue.
const TRIGGERS: &[(&str, Option<u32>, Speaker)] = &[
    ("Cinematic Start Knock Knock", Some(741), Speaker::Guide),
    ("Cinematic Start Lets Smash Things", Some(744), Speaker::Guide),
    ("Cinematic Start Dreadleg Summons Pirate", Some(749), Speaker::Player),
    ("Cinematic Start Ambush", Some(758), Speaker::Guide),
    ("XX_Cinematic Start Lets Build Together", Some(769), Speaker::Guide),
    ("XX_Cinematic Start Dreadleg Escapes", None, Speaker::Guide),
];
/// The last dialogue (after the boss).
const LAST_DIALOGUE: u32 = 765;
/// Jump pads that stay shut until the player beat the waves before them, as live did (the
/// recording: blocker 596 hidden and pad 597 shown once the summoned pirate fell, 601/599 after
/// the ambush's two waves): (pad instance, script steps whose enemies must be beaten).
const GATED_PADS: &[(u32, &[&str])] = &[(195_056, &["DreadlegSpawnPirate"]), (195_055, &["SpawnAmbush", "SpawnAmbush2"])];

/// Per player in the tutorial: the enemies each script step spawned, and the pads opened.
#[derive(Default)]
struct Progress {
    spawned: std::collections::HashMap<&'static str, Vec<Id32>>,
    open: HashSet<u32>,
}

/// The characters `SpawnCompass` gave the compass (in memory: tag 4121 follows in the same
/// dialogue).
static COMPASS: LazyLock<std::sync::Mutex<HashSet<u32>>> = LazyLock::new(Default::default);

/// Whether the player can use the tutorial's portals (always elsewhere, and in the world
/// editor); `done`: the player has tag 4121.
pub fn compass_open(character: u32, playfield: Pf, done: bool) -> bool {
    playfield.model() != PLAYFIELD
        || done
        || crate::instance::mode_of(playfield).rules().editor
        || COMPASS.lock().unwrap().contains(&character)
}

/// The compass for a player arriving in copy `pf` of the tutorial who has it (`compass_open`).
pub fn compass_records(character: u32, pf: Pf, done: bool) -> Vec<Bytes> {
    if pf.model() != PLAYFIELD || !compass_open(character, pf, done) {
        return Vec::new();
    }
    crate::teleport::door_records_of(pf, COMPASS_DOOR)
}

/// `SpawnCompass`: the compass appears for the player and its portals open.
fn spawn_compass(player: Id32) {
    COMPASS.lock().unwrap().insert(player.instance);
    let records = crate::teleport::door_records_of(copy_of(player), COMPASS_DOOR);
    crate::world::send_to(player, crate::world::Out::Records(records));
    info!(?player, "tutorial compass");
}

static PROGRESS: LazyLock<std::sync::Mutex<std::collections::HashMap<Id32, Progress>>> = LazyLock::new(Default::default);

/// Whether the player may use jump pad `guid` (always outside the tutorial or after it).
pub fn pad_open(player: Id32, playfield: Pf, guid: u32) -> bool {
    if playfield.model() != PLAYFIELD || !GATED_PADS.iter().any(|(g, _)| *g == guid) {
        return true;
    }
    PROGRESS.lock().unwrap().get(&player).is_none_or(|p| p.open.contains(&guid))
}

/// Whether every enemy the player's script steps spawned so far is beaten.
fn all_beaten(player: Id32) -> bool {
    let ids: Vec<Id32> = PROGRESS.lock().unwrap().get(&player).map_or(vec![], |p| p.spawned.values().flatten().copied().collect());
    ids.into_iter().all(|id| crate::npc::health_of(id).is_none())
}

#[derive(Deserialize)]
struct Trigger {
    name: String,
    pos: [f32; 3],
}

#[derive(Deserialize)]
struct Content {
    triggers: Vec<Trigger>,
    enemies: Vec<SpawnDef>,
    npcs: Vec<SpawnDef>,
}

static CONTENT: LazyLock<Content> = LazyLock::new(|| {
    // the cinematic triggers (world_data.rs) and the enemies and NPCs (spawns.rs)
    let world = &crate::world_data::WORLD_CONTENT[&PLAYFIELD.to_string()];
    let defs = crate::spawns::table().remove(&PLAYFIELD.to_string()).unwrap_or_default();
    Content {
        triggers: serde_json::from_value(world["triggers"].clone()).expect("tutorial triggers"),
        enemies: defs.iter().filter(|d| d.kind == crate::npc::Kind::Enemy).cloned().collect(),
        npcs: defs.iter().filter(|d| d.kind == crate::npc::Kind::Npc).cloned().collect(),
    }
});

/// The copy of the tutorial `player` is in (its spawns go there).
fn copy_of(player: Id32) -> Pf {
    crate::world::whereabouts(player).map(|(pf, _)| pf).filter(|pf| pf.model() == PLAYFIELD).unwrap_or(Pf::main(PLAYFIELD))
}

/// Whether the tutorial spawns this NPC itself (per player) instead of it standing there.

pub fn scripted(playfield: u32, name: &str) -> bool {
    playfield == PLAYFIELD && (name == "Scallywag" || name == GUIDE)
}

/// The minifigure of starter set `set`.
pub fn starter(set: u32) -> Option<u32> {
    STARTER_SETS.get(set as usize).copied()
}

/// `GameActionListIIR_t` starting the creation script (recorded): u32 1, action
/// StartClientSideScryScript (0xcfed), sequence, 4, 0, 4 (key, value) pairs (761 = the script),
/// 0, 0, no strings, u8 0.
pub fn creation_script_record(player: Id32) -> Bytes {
    iir::record("GameActionListIIR_t", player, false, |w| {
        let w = w.u32(1).u32(0xcfed).u32(4).u32(4).u32(0).u32(4);
        let w = [(725, 1), (726, 0), (754, 1), (761, LOGIN_SETUP_SCRIPT)].iter().fold(w, |w, &(k, v)| w.u32(k).u32(v));
        w.u32(0).u32(0).u32(0).u8(0)
    })
}

/// `SetNameIIR_t` (recorded: u32 3, u8 1, u8 1, string).
pub fn set_name_record(player: Id32, name: &str) -> Bytes {
    iir::record("SetNameIIR_t", player, false, |w| w.u32(3).u8(1).u8(1).string(name))
}

/// `ScryScriptIIR_c` script message `message` ("CombatMusic_Boss", "CombatMusic_Stop"), as
/// recorded: u32 length + protobuf {1: 5, 2: 2, 3: 0, 4: message, 5: "Default" or "", 6: {1: 0,
/// 2: 0}}.
pub fn music_record(player: Id32, message: &str) -> Bytes {
    let parameter: &[u8] = if message == "CombatMusic_Boss" { b"Default" } else { b"" };
    let mut proto = vec![0x08, 5, 0x10, 2, 0x18, 0, 0x22, message.len() as u8];
    proto.extend_from_slice(message.as_bytes());
    proto.extend_from_slice(&[0x2a, parameter.len() as u8]);
    proto.extend_from_slice(parameter);
    proto.extend_from_slice(&[0x32, 4, 0x08, 0, 0x10, 0]);
    iir::record("ScryScriptIIR_c", player, false, |w| w.u32(proto.len() as u32).raw(&proto))
}

/// A player's tutorial run (one per visit to 1300): the triggers passed and the escort.
pub struct Run {
    player: Id32,
    name: String,
    fired: HashSet<usize>,
    escort: Option<Id32>,
}

impl Run {
    /// The player entered the tutorial at `at`: Video Game Guy comes out of his house. The
    /// route runs west to east: a player back mid-way (a relog) has the steps and pads behind
    /// them done.
    pub fn start(player: Id32, name: &str, at: [f32; 3]) -> Run {
        let escort = CONTENT
            .npcs
            .iter()
            .find(|n| n.name == GUIDE)
            .map(|def| crate::npc::escort(player, copy_of(player), def.clone(), ESCORT_LIFETIME));
        info!(?player, ?escort, "tutorial");
        let behind = |x: f32| x + 5.0 < at[0];
        let pad_x = |g: u32| crate::jumppad::pad_pos(Pf::main(PLAYFIELD), g).map_or(f32::MAX, |p| p[0]);
        let open: HashSet<u32> = GATED_PADS.iter().map(|(g, _)| *g).filter(|g| behind(pad_x(*g))).collect();
        let fired: HashSet<usize> = TRIGGERS
            .iter()
            .enumerate()
            .filter(|(_, (n, ..))| CONTENT.triggers.iter().any(|t| t.name == *n && behind(t.pos[0])))
            .map(|(i, _)| i)
            .collect();
        // the gated pads ahead are shut for this player until their waves are beaten
        let shut: Vec<Bytes> = GATED_PADS.iter().filter(|(g, _)| !open.contains(g)).flat_map(|(g, _)| crate::jumppad::pad_records(copy_of(player), *g, false)).collect();
        crate::world::send_to(player, crate::world::Out::Records(shut));
        PROGRESS.lock().unwrap().insert(player, Progress { spawned: Default::default(), open });
        Run { player, name: name.to_string(), fired, escort }
    }

    /// The player is at `pos`: runs the triggers there (each once). Returns the boss music
    /// record when the boss trigger fired.
    pub fn moved(&mut self, pos: [f32; 3]) -> Option<Bytes> {
        self.open_pads();
        // in order, and only with the enemies so far beaten: nothing past a fight (or the boss)
        // starts before it is won
        let (i, &(name, dialogue, speaker)) = TRIGGERS.iter().enumerate().find(|(i, (name, ..))| {
            let radius = if *i == 0 { DOOR_RADIUS } else { TRIGGER_RADIUS };
            !self.fired.contains(i)
                && (*i == 0 || self.fired.contains(&(i - 1)))
                && CONTENT.triggers.iter().any(|t| t.name == *name && distance(t.pos, pos) <= radius)
        })?;
        if !all_beaten(self.player) {
            return None;
        }
        self.fired.insert(i);
        info!(player = ?self.player, trigger = name, "tutorial trigger");
        match dialogue {
            Some(first) => {
                self.say(first, speaker);
                None
            }
            None => Some(music_record(self.player, "CombatMusic_Boss")),
        }
    }

    /// The boss is defeated: the last dialogue (after the player was moved to watch); no
    /// trigger runs any more.
    pub fn boss_defeated(&mut self) {
        self.fired.extend(0..TRIGGERS.len());
        let (player, name, owner) = (self.player, self.name.clone(), self.speaker(Speaker::Guide));
        tokio::spawn(async move {
            // after the teleport's arrival
            tokio::time::sleep(Duration::from_secs(3)).await;
            if crate::world::whereabouts(player).is_some_and(|(pf, _)| pf.model() == PLAYFIELD)
                && let Some(d) = crate::knubot::starting_with(LAST_DIALOGUE)
            {
                crate::knubot::start(player, &name, owner, d);
            }
        });
    }

    /// Opens the gated pads whose waves the player has beaten.
    fn open_pads(&self) {
        let opened: Vec<u32> = {
            let mut progress = PROGRESS.lock().unwrap();
            let Some(p) = progress.get_mut(&self.player) else { return };
            let due: Vec<u32> = GATED_PADS
                .iter()
                .filter(|(g, steps)| {
                    !p.open.contains(g)
                        && steps.iter().all(|s| {
                            p.spawned.get(s).is_some_and(|ids| ids.iter().all(|id| crate::npc::health_of(*id).is_none()))
                        })
                })
                .map(|(g, _)| *g)
                .collect();
            p.open.extend(&due);
            due
        };
        for g in opened {
            info!(player = ?self.player, pad = g, "tutorial pad opened");
            crate::world::send_to(self.player, crate::world::Out::Records(crate::jumppad::pad_records(copy_of(self.player), g, true)));
        }
    }

    fn say(&self, first: u32, speaker: Speaker) {
        if let Some(d) = crate::knubot::starting_with(first) {
            crate::knubot::start(self.player, &self.name, self.speaker(speaker), d);
        }
    }

    fn speaker(&self, speaker: Speaker) -> Id32 {
        match (speaker, self.escort) {
            (Speaker::Guide, Some(escort)) => escort,
            _ => self.player,
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        PROGRESS.lock().unwrap().remove(&self.player);
        if let Some(escort) = self.escort {
            crate::npc::despawn(escort);
        }
    }
}

/// A step of the tutorial's server script 599417 (from a dialogue tag).
pub fn script(player: Id32, step: &str) {
    // the Scallywags by their recorded spots: AI_Combat1, then 2-4, then 8-10
    let (step, range): (&'static str, _) = match step {
        "DreadlegSpawnPirate" => ("DreadlegSpawnPirate", 0..1),
        "SpawnAmbush" => ("SpawnAmbush", 1..4),
        "SpawnAmbush2" => ("SpawnAmbush2", 4..7),
        "SpawnCompass" => return spawn_compass(player),
        _ => {
            if !crate::story::script_step(player, step) {
                info!(?player, step, "tutorial script step not run");
            }
            return;
        }
    };
    let pirates: Vec<&SpawnDef> = CONTENT.enemies.iter().filter(|e| e.name == "Scallywag").collect();
    let mut ids = Vec::new();
    for def in pirates.get(range).unwrap_or_default() {
        // the recording caught some mid-fight: at least a regular level-1 enemy's health
        let def = SpawnDef { health: def.health.max(75), ..(*def).clone() };
        ids.push(crate::npc::spawn_temporary(def, copy_of(player), SPAWN_LIFETIME));
    }
    if let Some(p) = PROGRESS.lock().unwrap().get_mut(&player) {
        p.spawned.entry(step).or_default().extend(ids);
    }
    info!(?player, step, "tutorial script step");
}

/// Test command targets: the triggers, 6 m west of them (the tour arrives 6 m east of a
/// target, `!goto` 3 m: both inside the trigger).
pub fn points() -> Vec<(String, u32, [f32; 3])> {
    TRIGGERS
        .iter()
        .filter_map(|(name, ..)| CONTENT.triggers.iter().find(|t| t.name == *name))
        .map(|t| (t.name.clone(), PLAYFIELD, [t.pos[0] - 6.0, t.pos[1], t.pos[2]]))
        .collect()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data() {
        assert_eq!(CONTENT.triggers.len(), TRIGGERS.len());
        assert!(TRIGGERS.iter().all(|(n, ..)| CONTENT.triggers.iter().any(|t| t.name == *n)));
        assert_eq!(CONTENT.enemies.iter().filter(|e| e.name == "Scallywag").count(), 7);
        assert!(TRIGGERS.iter().filter_map(|t| t.1).chain([LAST_DIALOGUE]).all(|f| crate::knubot::starting_with(f).is_some()));
        assert_eq!(starter(1), Some(317609));
    }

    #[test]
    fn compass_after_spawn_compass() {
        let (pf, character) = (Pf::main(PLAYFIELD), 0x7e57_0001);
        assert!(!crate::teleport::door_records_of(pf, COMPASS_DOOR).is_empty());
        assert!(!compass_open(character, pf, false) && compass_records(character, pf, false).is_empty());
        assert!(compass_open(character, pf, true) && !compass_records(character, pf, true).is_empty());
        assert!(compass_open(character, Pf::main(4600), false));
        COMPASS.lock().unwrap().insert(character);
        assert!(compass_open(character, pf, false) && !compass_records(character, pf, false).is_empty());
    }

    #[test]
    fn records_match_the_recording() {
        let id = Id32::new(50000, 16777869);
        let body = |r: Bytes| r[4 + 8 + 1..].to_vec();
        let hex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        assert_eq!(
            body(creation_script_record(id)),
            hex("000000010000cfed00000004000000040000000000000004000002d500000001000002d600000000000002f200000001000002f9000261bc00000000000000000000000000")
        );
        assert_eq!(body(set_name_record(id, "Metric")), hex("00000003010100064d6574726963"));
        assert_eq!(
            body(music_record(id, "CombatMusic_Boss")),
            hex("000000270805100218002210436f6d6261744d757369635f426f73732a0744656661756c74320408001000")
        );
        assert_eq!(body(music_record(id, "CombatMusic_Stop")), hex("000000200805100218002210436f6d6261744d757369635f53746f702a00320408001000"));
    }
}
