//! The shared game world: characters in play and the IIR traffic between them.
//!
//! Each game connection registers its character here once it is in play and receives the
//! other characters' IIRs (spawn, movement, despawn) through a channel. Records are complete
//! IIRs (see `lmoxide_net::iir::record`); receivers wrap them in a `PassBlob`.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use bytes::Bytes;
use lmoxide_net::types::Id32;
use tokio::sync::mpsc;

/// What a member's connection is asked to send to its client.
#[derive(Clone, Debug)]
pub enum Out {
    /// IIR records, sent as one `PassBlob`.
    Records(Vec<Bytes>),
    /// One RPC (endpoint, method id, args), e.g. community messages from another player's
    /// connection.
    Rpc(lmoxide_net::Endpoint, u32, Bytes),
    /// Disconnect the player (kicked by an admin, or logged in again elsewhere), telling them
    /// why in chat first.
    Kick(Option<String>),
    /// Something the player did that achievements (and XP) count.
    Progress(crate::achievements::Event),
}

pub type Outbox = mpsc::UnboundedSender<Out>;
pub type Position = Arc<Mutex<[f32; 3]>>;

struct Member {
    playfield: u32,
    pos: Position,
    /// Records that make this character appear for a newcomer (see `game::spawn_records`).
    spawn: Spawn,
    outbox: Outbox,
}

static MEMBERS: LazyLock<Mutex<HashMap<Id32, Member>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

type Spawn = Arc<dyn Fn() -> Vec<Bytes> + Send + Sync>;

/// Adds a character to `playfield`: it receives everyone else's spawn records and everyone
/// else receives its own. Dropping the returned guard removes it again (see [`Presence`]).
pub fn join(
    id: Id32,
    playfield: u32,
    pos: Position,
    spawn: impl Fn() -> Vec<Bytes> + Send + Sync + 'static,
    outbox: Outbox,
) -> Presence {
    let spawn: Spawn = Arc::new(spawn);
    let mine = spawn();
    // The spawn closures run outside the MEMBERS lock: NPCs' lock NPCS, and code holding NPCS
    // sends through MEMBERS (npc::hit), so calling them under MEMBERS could deadlock.
    let others: Vec<(Spawn, Outbox)> = {
        let mut members = MEMBERS.lock().unwrap();
        let others = members.values().filter(|m| m.playfield == playfield).map(|m| (m.spawn.clone(), m.outbox.clone())).collect();
        members.insert(id, Member { playfield, pos, spawn: spawn.clone(), outbox: outbox.clone() });
        others
    };
    let mut theirs = Vec::new();
    for (their_spawn, their_outbox) in others {
        theirs.extend(their_spawn());
        let _ = their_outbox.send(Out::Records(mine.clone()));
    }
    if !theirs.is_empty() {
        let _ = outbox.send(Out::Records(theirs));
    }
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

/// Where a member is.
pub fn position_of(id: Id32) -> Option<[f32; 3]> {
    MEMBERS.lock().unwrap().get(&id).map(|m| *m.pos.lock().unwrap())
}

/// Playfield and position of a character in play.
pub fn whereabouts(id: Id32) -> Option<(u32, [f32; 3])> {
    MEMBERS.lock().unwrap().get(&id).map(|m| (m.playfield, *m.pos.lock().unwrap()))
}

/// Every character on `playfield` with its position.
pub fn positions(playfield: u32) -> Vec<(Id32, [f32; 3])> {
    let members = MEMBERS.lock().unwrap();
    members.iter().filter(|(_, m)| m.playfield == playfield).map(|(id, m)| (*id, *m.pos.lock().unwrap())).collect()
}

/// The other characters on `from`'s playfield.
pub fn neighbours(from: Id32) -> Vec<Id32> {
    let members = MEMBERS.lock().unwrap();
    let Some(playfield) = members.get(&from).map(|m| m.playfield) else { return vec![] };
    members.iter().filter(|(id, m)| **id != from && m.playfield == playfield).map(|(id, _)| *id).collect()
}

/// Sends `out` to one member (no-op if it isn't in the world).
pub fn send_to(id: Id32, out: Out) {
    if let Some(m) = MEMBERS.lock().unwrap().get(&id) {
        let _ = m.outbox.send(out);
    }
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
