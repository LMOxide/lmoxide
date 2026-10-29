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
/// Copies per world: the key's top bit stays clear.
pub const MAX_COPY: u32 = (1 << (31 - MODEL_BITS)) - 1;

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

/// The copy of playfield `model` a player arriving there (logging in, teleporting) joins. For
/// now always the main copy; later the open-world player cap and team copies (pocket
/// adventures, Lost Creations) are chosen here.
pub fn arrival(model: u32) -> Pf {
    Pf::main(model)
}

/// The copy a player in `from` going to playfield `to` ends up in: the same copy within its own
/// world (a teleport spot, a script moving them), else the one [`arrival`] picks.
pub fn towards(from: Pf, to: u32) -> Pf {
    if from.model() == to { from } else { arrival(to) }
}


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
