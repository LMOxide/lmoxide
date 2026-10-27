//! The shared game world: characters in play and the IIR traffic between them.
//!
//! Each game connection registers its character here once it is in play and receives the
//! other characters' IIRs (spawn, movement, despawn) through a channel. Records are complete
//! IIRs (see `lmoxide_net::iir::record`); receivers wrap them in a `PassBlob`.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Instant,
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
    /// Development tour: teleport next to (playfield, position), announcing `label`.
    DevTeleport(u32, [f32; 3], String),
    /// A test command (devcmd.rs) typed into a chat-server channel (e.g. the Global tab) or sent
    /// through the control interface; the sender, if any, is dropped once it has been handled.
    Command(String, Option<std::sync::Arc<tokio::sync::oneshot::Sender<()>>>),
    /// Teleport the player (a server script, e.g. an NPC dialogue): playfield (`None`: the
    /// current one), position, facing.
    Teleport(Option<u32>, [f32; 3], f32),
    /// Open teleport window `n` for the player, as a portal would (a trigger wall, walls.rs).
    Window(u32),
    /// The player's minifigure (item template) was smashed (smash.rs).
    Smashed(u32),
    /// The player's smashed minifigure may be rebuilt now (smash.rs).
    Rebuilt(u32),
}

pub type Outbox = mpsc::UnboundedSender<Out>;
pub type Position = Arc<Mutex<[f32; 3]>>;

struct Member {
    playfield: u32,
    pos: Position,
    /// How the player last said they move (`set_motion`).
    motion: Option<Motion>,
    /// Records that make this character appear for a newcomer (see `game::spawn_records`).
    spawn: Spawn,
    outbox: Outbox,
    /// Players: the way they face (x, z), as their movement last said (`set_facing`).
    facing: [f32; 2],
}

/// How long a keyboard-moving player is assumed to keep their reported velocity: the client
/// reports every ~0.2 s while it moves and once when it stops (`Vehicle_t::UpdateServer`).
const STEER_AHEAD: f32 = 0.3;

/// How a player last said they move (game.rs), to tell where they are between their reports
/// (`live_position`): `pos` only holds what they reported, which for click-to-move is where they
/// are going.
#[derive(Clone, Debug)]
pub enum Motion {
    /// Keyboard movement (`CharDCMoveIIR_t`): at `at` with velocity `vel` (m/s) at `since`.
    Steer { at: [f32; 3], vel: [f32; 3], since: Instant },
    /// Click-to-move (`MoveTaskIIR_c`): along `points` at `speed` (m/s) from `since`.
    Path { points: Vec<[f32; 3]>, speed: f32, since: Instant },
}

impl Motion {
    /// Where it puts the character at `now`.
    pub fn at(&self, now: Instant) -> [f32; 3] {
        match self {
            Motion::Steer { at, vel, since } => {
                let t = now.saturating_duration_since(*since).as_secs_f32().min(STEER_AHEAD);
                [0, 1, 2].map(|i| at[i] + vel[i] * t)
            }
            Motion::Path { points, speed, since } => {
                let mut left = speed * now.saturating_duration_since(*since).as_secs_f32();
                for w in points.windows(2) {
                    let len = (0..3).map(|i| (w[1][i] - w[0][i]).powi(2)).sum::<f32>().sqrt();
                    if left < len {
                        let t = left / len;
                        return [0, 1, 2].map(|i| w[0][i] + (w[1][i] - w[0][i]) * t);
                    }
                    left -= len;
                }
                points.last().copied().unwrap_or_default()
            }
        }
    }

    /// The position the report itself gave (what `pos` was set to).
    fn reported(&self) -> Option<[f32; 3]> {
        match self {
            Motion::Steer { at, .. } => Some(*at),
            Motion::Path { points, .. } => points.last().copied(),
        }
    }
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
    let others: Vec<(Id32, Spawn, Outbox)> = {
        let mut members = MEMBERS.lock().unwrap();
        let others = members.iter().filter(|(_, m)| m.playfield == playfield).map(|(i, m)| (*i, m.spawn.clone(), m.outbox.clone())).collect();
        members.insert(id, Member { playfield, pos, motion: None, spawn: spawn.clone(), outbox: outbox.clone(), facing: [0.0, 1.0] });
        others
    };
    let mut theirs = Vec::new();
    // (survey mode: what a surveyor hid isn't sent to them)
    for (their_id, their_spawn, their_outbox) in others {
        if !crate::survey::hides(id, their_id) {
            theirs.extend(their_spawn());
        }
        if !crate::survey::hides(their_id, id) {
            let _ = their_outbox.send(Out::Records(mine.clone()));
        }
    }
    if !theirs.is_empty() {
        let _ = outbox.send(Out::Records(theirs));
    }
    Presence { id }
}

/// The records that make member `id` appear (to show it again to one player).
pub fn spawn_records_of(id: Id32) -> Option<Vec<Bytes>> {
    let spawn = MEMBERS.lock().unwrap().get(&id).map(|m| m.spawn.clone())?;
    Some(spawn())
}

/// Records the way player `id` faces (x, z).
pub fn set_facing(id: Id32, facing: [f32; 2]) {
    if let Some(m) = MEMBERS.lock().unwrap().get_mut(&id) {
        m.facing = facing;
    }
}

/// The way player `id` faces (x, z).
pub fn facing_of(id: Id32) -> Option<[f32; 2]> {
    MEMBERS.lock().unwrap().get(&id).map(|m| m.facing)
}

/// Where a member is.
pub fn position_of(id: Id32) -> Option<[f32; 3]> {
    MEMBERS.lock().unwrap().get(&id).map(|m| *m.pos.lock().unwrap())
}

/// Records how player `id` moves, as their latest movement report says (after `pos` was set
/// from it).
pub fn set_motion(id: Id32, motion: Motion) {
    if let Some(m) = MEMBERS.lock().unwrap().get_mut(&id) {
        m.motion = Some(motion);
    }
}

impl Member {
    /// Where the member is now: along its motion while that is what last set `pos` (a
    /// teleport, jump or knockback since then moved it elsewhere), else `pos`.
    fn live(&self, now: Instant) -> [f32; 3] {
        let pos = *self.pos.lock().unwrap();
        match &self.motion {
            Some(m) if m.reported() == Some(pos) => m.at(now),
            _ => pos,
        }
    }
}

/// Where a member is now, between its movement reports (see `Motion`).
pub fn live_position(id: Id32) -> Option<[f32; 3]> {
    MEMBERS.lock().unwrap().get(&id).map(|m| m.live(Instant::now()))
}

/// Every character on `playfield` with where it is now (`live_position`).
pub fn live_positions(playfield: u32) -> Vec<(Id32, [f32; 3])> {
    let now = Instant::now();
    let members = MEMBERS.lock().unwrap();
    members.iter().filter(|(_, m)| m.playfield == playfield).map(|(id, m)| (*id, m.live(now))).collect()
}

/// The shared position of every player character on `playfield` (to move them, e.g. a jump).
pub fn position_handles(playfield: u32) -> Vec<(Id32, Position)> {
    let members = MEMBERS.lock().unwrap();
    members.iter().filter(|(_, m)| m.playfield == playfield).map(|(id, m)| (*id, m.pos.clone())).collect()
}

/// Playfield and position of a character in play.
pub fn whereabouts(id: Id32) -> Option<(u32, [f32; 3])> {
    MEMBERS.lock().unwrap().get(&id).map(|m| (m.playfield, *m.pos.lock().unwrap()))
}

/// Every member with its playfield.
pub fn members() -> Vec<(Id32, u32)> {
    MEMBERS.lock().unwrap().iter().map(|(id, m)| (*id, m.playfield)).collect()
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
        let Some(me) = MEMBERS.lock().unwrap().remove(&self.id) else { return };
        // only to the playfield it leaves: a client told to remove a character it doesn't have
        // ignores that character's spawn later (a player teleporting to someone stayed
        // invisible to them)
        let members = MEMBERS.lock().unwrap();
        for m in members.values().filter(|m| m.playfield == me.playfield) {
            let _ = m.outbox.send(Out::Records(vec![crate::game::despawn_record(self.id)]));
        }
    }
}
