//! Running copies of worlds (docs/SERVER-ARCHITECTURE.md, "Instances").
//!
//! The client keys a playfield by `PlayfieldProxyData.running_instance` and loads the map named by
//! its `model_id`, so one map can run several times at once (live made a new copy of a pocket
//! adventure per visit). [`Pf`] is one running copy: everything that lives in a world (members,
//! NPCs, encounters, loot bags, ...) is kept per `Pf`, while game data (spawns, portals, navmesh,
//! content records) is looked up by its [`Pf::model`].
//!
//! The key holds the model, so a `Pf` needs no registry: `copy << 16 | model`. Copy 0 is the
//! world's main copy, whose key is the playfield id itself (what we sent before instances).

use std::fmt;

/// Playfield ids fit in 16 bits (the largest is 33000).
const MODEL_BITS: u32 = 16;
const MODEL_MASK: u32 = (1 << MODEL_BITS) - 1;
/// Copies per world: a fixture's dynel instance carries its copy in its top byte (`fixture`).
pub const MAX_COPY: u32 = 255;

/// One running copy of a world. In JSON it is its key, so a main copy reads and writes as the
/// plain playfield id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Pf(u32);


impl Pf {
    /// The main copy of playfield `model`.
    pub const fn main(model: u32) -> Self {
        Self(model & MODEL_MASK)
    }

    /// Copy `copy` of playfield `model` (0: the main copy). `copy` is capped at [`MAX_COPY`].
    pub fn copy_of(model: u32, copy: u32) -> Self {
        Self(copy.min(MAX_COPY) << MODEL_BITS | model & MODEL_MASK)
    }

    /// The copy a client key names (`running_instance`, an UpdateMessage's playfield key).
    pub const fn from_key(key: u32) -> Self {
        Self(key)
    }

    /// The key the client knows this copy by.
    pub const fn key(self) -> u32 {
        self.0
    }

    /// The playfield (map) this is a copy of: what game data is looked up by.
    pub const fn model(self) -> u32 {
        self.0 & MODEL_MASK
    }

    /// Which copy of its world this is (0: the main one).
    pub const fn copy(self) -> u32 {
        self.0 >> MODEL_BITS
    }

    pub const fn is_main(self) -> bool {
        self.copy() == 0
    }
}

/// A game mode: the rules a character plays by. Each mode has its own characters and its own
/// copies of the worlds (a character never changes mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum Mode {
    /// As live: characters start at level 1, earn XP and stars, rank minifigures up and buy
    /// upgrades; the collection grows from drops.
    Progression,
    /// Everything unlocked: level 30, every minifigure owned and fully ranked.
    Maxed,
    /// The world editor's worlds: nothing moves or attacks; attacking something selects it, and
    /// everyone may use the test commands (docs/SURVEY.md).
    WorldEditor,
}

/// What a mode changes.
#[derive(Debug)]
pub struct Rules {
    /// Levels, XP, minifigure ranks, upgrade purchases and a collection that grows; otherwise
    /// everything is unlocked and at its maximum.
    pub progression: bool,
    /// The world editor's: nothing moves or attacks, interactions select things for the editor,
    /// names and headings show as placed.
    pub editor: bool,
    /// The lowest GM level a player has here (world editors teleport and use test commands).
    pub min_gm_level: u32,
}

const RULES: [Rules; 3] = [
    Rules { progression: true, editor: false, min_gm_level: 0 },
    Rules { progression: false, editor: false, min_gm_level: 0 },
    Rules { progression: false, editor: true, min_gm_level: 1 },
];

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Progression, Mode::Maxed, Mode::WorldEditor];

    const fn index(self) -> usize {
        match self {
            Mode::Progression => 0,
            Mode::Maxed => 1,
            Mode::WorldEditor => 2,
        }
    }

    /// Its id in the database, the API and configuration.
    pub const fn id(self) -> &'static str {
        match self {
            Mode::Progression => "progression",
            Mode::Maxed => "maxed",
            Mode::WorldEditor => "world-editor",
        }
    }

    /// Its name for players (the patcher's Game Mode list).
    pub const fn name(self) -> &'static str {
        match self {
            Mode::Progression => "Progression",
            Mode::Maxed => "Maxxed",
            Mode::WorldEditor => "World Editor",
        }
    }

    pub fn parse(id: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.id() == id)
    }

    /// A character's mode from its saved `mode` (unknown: progression, the mode of every
    /// character made before modes existed).
    pub fn of_character(c: &lmoxide_db::Character) -> Mode {
        Mode::parse(&c.mode).unwrap_or(Mode::Progression)
    }

    pub fn rules(self) -> &'static Rules {
        &RULES[self.index()]
    }

    /// The mode's main copy of playfield `model`: copy 0, 1, 2 for the modes in [`Mode::ALL`]
    /// order (copy 0, the playfield id as key, is progression's: the characters made before
    /// modes keep the copy they had).
    pub fn main(self, model: u32) -> Pf {
        Pf::copy_of(model, self.index() as u32)
    }
}

/// Copies beyond the modes' main ones start here.
pub const FIRST_EXTRA_COPY: u32 = Mode::ALL.len() as u32;

/// The mode copy `pf` belongs to.
pub fn mode_of(pf: Pf) -> Mode {
    match Mode::ALL.get(pf.copy() as usize) {
        Some(&m) => m,
        None => extra_mode(pf).unwrap_or(Mode::Progression),
    }
}

/// The rules of copy `pf`.
pub fn rules(pf: Pf) -> &'static Rules {
    mode_of(pf).rules()
}

/// The mode of an extra copy (beyond the main ones), as it was made.
fn extra_mode(pf: Pf) -> Option<Mode> {
    EXTRAS.lock().unwrap().get(&pf).map(|e| e.mode)
}

/// An extra copy of a world: who it is for.
struct Extra {
    mode: Mode,
}

static EXTRAS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<Pf, Extra>>> = std::sync::LazyLock::new(Default::default);

/// Makes an extra copy of playfield `model` for players of `mode`: the lowest copy number not in
/// use. `None` when every copy is (`MAX_COPY`).
pub fn new_copy(model: u32, mode: Mode) -> Option<Pf> {
    let mut extras = EXTRAS.lock().unwrap();
    let copy = (FIRST_EXTRA_COPY..=MAX_COPY).find(|c| !extras.contains_key(&Pf::copy_of(model, *c)) && !crate::worlds::is_spawned(Pf::copy_of(model, *c)))?;
    let pf = Pf::copy_of(model, copy);
    extras.insert(pf, Extra { mode });
    Some(pf)
}

/// Forgets extra copy `pf` (dropped, worlds.rs): its number is free again.
pub fn forget_copy(pf: Pf) {
    EXTRAS.lock().unwrap().remove(&pf);
}

/// The copy of playfield `model` a player of `mode` arriving there (logging in, teleporting)
/// joins: the mode's main copy. Later the open-world player cap and team copies (pocket
/// adventures, Lost Creations) are chosen here.
pub fn arrival(mode: Mode, model: u32) -> Pf {
    mode.main(model)
}

/// The copy a player of `mode` in `from` going to playfield `to` ends up in: the same copy within
/// its own world (a teleport spot, a script moving them), else the one [`arrival`] picks.
pub fn towards(mode: Mode, from: Pf, to: u32) -> Pf {
    if from.model() == to { from } else { arrival(mode, to) }
}



/// Fixture ids: a world's fixed objects (doors, scenery, jump pads, puzzle markers) have one
/// dynel per copy, the base instance (their guid, < 2^24) with the copy in the top byte. Copy
/// 0's are the bare guids. Fixtures are never characters, so they can't meet a player's id.
pub const fn fixture(pf: Pf, base: u32) -> u32 {
    base & FIXTURE_MASK | pf.copy() << 24
}

/// The base instance (guid) of a fixture's dynel instance.
pub const fn fixture_base(instance: u32) -> u32 {
    instance & FIXTURE_MASK
}

const FIXTURE_MASK: u32 = 0x00ff_ffff;

/// `4200`, or `4200#3` for copy 3. No `Display` on purpose: game data is keyed by playfield
/// number strings, and `pf.to_string()` would silently miss for copies; use `pf.model()`.
impl fmt::Debug for Pf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.copy() {
            0 => write!(f, "{}", self.model()),
            c => write!(f, "{}#{c}", self.model()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_copy_key_is_the_playfield_id() {
        assert_eq!(Pf::main(4200).key(), 4200);
        assert_eq!(Pf::copy_of(4200, 0), Pf::main(4200));
        assert_eq!(Pf::main(33000).model(), 33000);
    }

    #[test]
    fn copies_keep_their_model() {
        let pf = Pf::copy_of(4200, 3);
        assert_eq!((pf.model(), pf.copy()), (4200, 3));
        assert_ne!(pf, Pf::main(4200));
        assert_eq!(Pf::from_key(pf.key()), pf);
        assert_eq!(format!("{pf:?}"), "4200#3");
        assert!(Pf::copy_of(4200, u32::MAX).key() < 1 << 31);
    }
}
