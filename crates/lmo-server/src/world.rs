//! The shared game world: characters in play and the IIR traffic between them.
//!
//! Each game connection registers its character here once it is in play and receives the
//! other characters' IIRs (spawn, movement, despawn) through a channel. Records are complete
//! IIRs (see `lmo_net::iir::record`); receivers wrap them in a `PassBlob`.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use bytes::Bytes;
use lmo_net::types::Id32;
use tokio::sync::mpsc;

/// What a member's connection is asked to send to its client.
#[derive(Clone, Debug)]
pub enum Out {
    /// IIR records, sent as one `PassBlob`.
    Records(Vec<Bytes>),
}

pub type Outbox = mpsc::UnboundedSender<Out>;
pub type Position = Arc<Mutex<[f32; 3]>>;

struct Member {
    playfield: u32,
    pos: Position,
    /// Records that make this character appear for a newcomer (see `game::spawn_records`).
    spawn: Box<dyn Fn() -> Vec<Bytes> + Send>,
    outbox: Outbox,
}

static MEMBERS: LazyLock<Mutex<HashMap<Id32, Member>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Adds a character to `playfield`: it receives everyone else's spawn records and everyone
/// else receives its own. Dropping the returned guard removes it again (see [`Presence`]).
pub fn join(
    id: Id32,
    playfield: u32,
    pos: Position,
    spawn: impl Fn() -> Vec<Bytes> + Send + 'static,
    outbox: Outbox,
) -> Presence {
    let mut members = MEMBERS.lock().unwrap();
    let mine = spawn();
    let mut theirs = Vec::new();
    for other in members.values().filter(|m| m.playfield == playfield) {
        theirs.extend((other.spawn)());
        let _ = other.outbox.send(Out::Records(mine.clone()));
    }
    if !theirs.is_empty() {
        let _ = outbox.send(Out::Records(theirs));
    }
    members.insert(id, Member { playfield, pos, spawn: Box::new(spawn), outbox });
    Presence { id }
}

/// Position of some other character on `playfield` (the test bot follows it).
pub fn someone_else(me: Id32, playfield: u32) -> Option<[f32; 3]> {
    let members = MEMBERS.lock().unwrap();
    members
        .iter()
        .find(|(id, m)| **id != me && m.playfield == playfield)
        .map(|(_, m)| *m.pos.lock().unwrap())
}

/// The other characters on `from`'s playfield.
pub fn neighbours(from: Id32) -> Vec<Id32> {
    let members = MEMBERS.lock().unwrap();
    let Some(playfield) = members.get(&from).map(|m| m.playfield) else { return vec![] };
    members.iter().filter(|(id, m)| **id != from && m.playfield == playfield).map(|(id, _)| *id).collect()
}

/// Sends records to every other character on the sender's playfield.
pub fn broadcast(from: Id32, records: Vec<Bytes>) {
    send_playfield(from, false, Out::Records(records));
}

/// Sends `out` to the characters on `from`'s playfield (including `from` if `to_self`).
pub fn send_playfield(from: Id32, to_self: bool, out: Out) {
    let members = MEMBERS.lock().unwrap();
    let Some(playfield) = members.get(&from).map(|m| m.playfield) else { return };
    for (id, m) in members.iter() {
        if (to_self || *id != from) && m.playfield == playfield {
            let _ = m.outbox.send(out.clone());
        }
    }
}

/// A character's membership; dropping it despawns the character for everyone else.
pub struct Presence {
    id: Id32,
}

impl Presence {
    pub fn id(&self) -> Id32 {
        self.id
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        MEMBERS.lock().unwrap().remove(&self.id);
        broadcast_all(self.id, vec![crate::game::despawn_record(self.id)]);
    }
}

fn broadcast_all(about: Id32, records: Vec<Bytes>) {
    let members = MEMBERS.lock().unwrap();
    for (id, m) in members.iter() {
        if *id != about {
            let _ = m.outbox.send(Out::Records(records.clone()));
        }
    }
}
