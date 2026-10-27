//! Friends and teams: the client's community server, whose RPCs travel on the game connection
//! (`CSPlayerAgent` / `CSPlayerInterface`, friend codes on `GameCharAgentProject`, leave/kick
//! on `GameCharAgent`). Formats and the client's handling: docs/PROTOCOL.md "Friends and teams".
//!
//! Friendships are kept in the database (`friendships`; both characters may be offline). Teams
//! live in memory: a member who disconnects stays in the team (shown offline) until everyone
//! has left.

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
};

use bytes::Bytes;
use lmoxide_net::{
    Endpoint, Reader, Writer, iir,
    methods::{cs_player_agent, cs_player_interface as cs, game_char_interface, game_char_interface_project},
    proto::{FriendsData, GroupElementData, ProtoId32, friends_data::Friend},
    types::Id32,
};
use tracing::info;

use crate::{state, territory::CHARACTER_TYPE, world};

/// ID32 type of teams (57016 would be a raid), from `AddToGroupIIR_c::Activate`.
const TEAM_TYPE: u32 = 57001;
/// The TeamGui has four member colours (`member_slot_index` 0..3).
const MAX_TEAM_SIZE: usize = 4;

struct Online {
    name: String,
    playfield: u32,
    outbox: world::Outbox,
}

struct Team {
    leader: u32,
    /// Character instances in slot order.
    members: Vec<u32>,
}

#[derive(Default)]
struct State {
    online: HashMap<u32, Online>,
    teams: HashMap<u32, Team>,
    team_of: HashMap<u32, u32>,
    /// invitee -> inviter (team invites)
    invites: HashMap<u32, u32>,
    /// invitee -> inviters (friend invites from the player context menu)
    friend_invites: HashMap<u32, HashSet<u32>>,
    /// (requester, target) resolved from a friend code: the follow-up add needs no invite
    by_code: HashSet<(u32, u32)>,
    next_team: u32,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State { next_team: 1, ..Default::default() }));

fn friends_of(character: u32) -> Vec<u32> {
    state::or_log(state::db().friends_of(character), "friends", Vec::new())
}

fn char_id(instance: u32) -> Id32 {
    Id32::new(CHARACTER_TYPE, instance)
}

fn name_of(st: &State, instance: u32) -> String {
    if let Some(o) = st.online.get(&instance) {
        return o.name.clone();
    }
    state::or_log(state::db().character_name(instance), "character name", None)
        .unwrap_or_else(|| format!("Player {instance}"))
}

fn exists(instance: u32) -> bool {
    state::or_log(state::db().character_exists(instance), "character lookup", false)
}

fn send(st: &State, to: u32, endpoint: Endpoint, method: u32, args: Bytes) {
    if let Some(o) = st.online.get(&to) {
        let _ = o.outbox.send(world::Out::Rpc(endpoint, method, args));
    }
}

fn send_cs(st: &State, to: u32, method: u32, args: Bytes) {
    send(st, to, Endpoint::CSPlayerInterface, method, args);
}

// ------------------------------------------------------------------ encoding

/// `GroupElement_c`: u32 length + GroupElementData. The client uses character_instance (with
/// type 50000), name, dimension, playfield and member_slot_index (map marker / list colour).
fn group_element(st: &State, instance: u32, slot: Option<usize>) -> GroupElementData {
    GroupElementData {
        character_instance: Some(instance),
        dimension: Some(1),
        playfield: st.online.get(&instance).map(|o| crate::teleport::proxy(o.playfield)),
        name: Some(name_of(st, instance)),
        max_team_size: Some(MAX_TEAM_SIZE as u32),
        member_slot_index: slot.map(|s| s as u32),
        ..Default::default()
    }
}

fn ge_args(st: &State, instance: u32) -> Writer {
    let slot = st.team_of.get(&instance).and_then(|t| st.teams[t].members.iter().position(|&m| m == instance));
    Writer::new().proto(&group_element(st, instance, slot))
}

/// `TeamInfo_c`: ID32 team, ID32 leader, vector<GroupElement_c>, u32 loot mode, u8 need/greed,
/// u8, ID32 master looter, u32 loot threshold.
fn team_info(st: &State, team: u32) -> Writer {
    let t = &st.teams[&team];
    let w = char_id(t.leader).write(Id32::new(TEAM_TYPE, team).write(Writer::new()));
    let w = t.members.iter().enumerate().fold(w.container_len(t.members.len()), |w, (slot, &m)| {
        w.proto(&group_element(st, m, Some(slot)))
    });
    char_id(t.leader).write(w.u32(0).bool(false).bool(false)).u32(0)
}

fn friend_entry(st: &State, instance: u32) -> Friend {
    let online = st.online.get(&instance);
    Friend {
        instance: Some(instance),
        name: Some(name_of(st, instance)),
        online: Some(online.is_some()),
        faction: Some(0),
        proxy: online.map(|o| crate::teleport::proxy(o.playfield)),
        dimension: Some(1),
    }
}

/// SignalFriendsListUpdate: an upsert of these friends on the client.
fn friends_update(st: &State, to: u32, friends: &[u32]) {
    if friends.is_empty() {
        return;
    }
    let data = FriendsData { friends: friends.iter().map(|&f| friend_entry(st, f)).collect() };
    send_cs(st, to, cs::SIGNAL_FRIENDS_LIST_UPDATE, Writer::new().proto(&data).finish());
}

fn friends_removed(st: &State, to: u32, removed: u32) {
    let data = FriendsData { friends: vec![Friend { instance: Some(removed), ..Default::default() }] };
    send_cs(st, to, cs::SIGNAL_FRIENDS_REMOVED, Writer::new().proto(&data).finish());
}

/// Group membership on the character dynel, so every client treats teammates as such
/// (`SimpleChar_t::SameTeamAsClient`): AddToGroupIIR_c (ID32 group, ID32 leader) /
/// RemoveFromGroupIIR_c (ID32 group), to everyone on the member's playfield.
fn group_records(st: &State, team: u32, joined: bool, members: &[u32]) {
    let leader = st.teams.get(&team).map_or(0, |t| t.leader);
    for &m in members {
        let rec = if joined {
            iir::record("AddToGroupIIR_c", char_id(m), false, |w| char_id(leader).write(Id32::new(TEAM_TYPE, team).write(w)))
        } else {
            iir::record("RemoveFromGroupIIR_c", char_id(m), false, |w| Id32::new(TEAM_TYPE, team).write(w))
        };
        PENDING.lock().unwrap().push((char_id(m), rec));
    }
}

/// Group records waiting for STATE to be released: sending takes the world lock, and the
/// world calls `team_ids` (STATE) while holding it.
static PENDING: Mutex<Vec<(Id32, Bytes)>> = Mutex::new(Vec::new());

fn flush() {
    let pending = std::mem::take(&mut *PENDING.lock().unwrap());
    for (member, rec) in pending {
        world::send_playfield(member, true, world::Out::Records(vec![rec]));
    }
}

/// (team, leader) of a character, for its full update (ProtoCoreCharacterIIR team_id /
/// team_leader_id).
pub fn team_ids(instance: u32) -> Option<(ProtoId32, ProtoId32)> {
    let st = STATE.lock().unwrap();
    let team = *st.team_of.get(&instance)?;
    let id32 = |kind, instance| ProtoId32 { r#type: Some(kind), instance: Some(instance) };
    Some((id32(TEAM_TYPE, team), id32(CHARACTER_TYPE, st.teams[&team].leader)))
}

/// The team of a character.
pub fn team_of(instance: u32) -> Option<u32> {
    STATE.lock().unwrap().team_of.get(&instance).copied()
}

/// Online teammates of a character (not including it).
pub fn teammates(instance: u32) -> Vec<u32> {
    let st = STATE.lock().unwrap();
    st.team_of
        .get(&instance)
        .map(|t| st.teams[t].members.iter().copied().filter(|&m| m != instance && st.online.contains_key(&m)).collect())
        .unwrap_or_default()
}

// ------------------------------------------------------------------ friend codes

/// Friend codes ("86U-LGL-HDY-LLF" on live): the character instance, scrambled, in 7 base-32
/// characters plus a 5-character check, so a code decodes without a lookup table.
const CODE_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const SCRAMBLE: u32 = 0x9e37_79b1;

fn check(v: u32) -> u32 {
    (v ^ 0x5bd1_e995).wrapping_mul(0x85eb_ca6b).rotate_left(13) & 0x1ff_ffff
}

pub fn friend_code(instance: u32) -> String {
    let v = (instance ^ 0x2545_f491).wrapping_mul(SCRAMBLE);
    let bits = (u64::from(v) << 25) | u64::from(check(v));
    let chars: Vec<u8> = (0..12).rev().map(|i| CODE_ALPHABET[((bits >> (5 * i)) & 31) as usize]).collect();
    chars.chunks(3).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}

pub fn decode_friend_code(code: &str) -> Option<u32> {
    let digits: Vec<u8> = code.bytes().filter(|&b| b != b'-').map(|b| b.to_ascii_uppercase()).collect();
    if digits.len() != 12 {
        return None;
    }
    let bits = digits.iter().try_fold(0u64, |acc, &c| Some((acc << 5) | CODE_ALPHABET.iter().position(|&a| a == c)? as u64))?;
    let v = (bits >> 25) as u32;
    if bits >> 57 != 0 || check(v) != (bits & 0x1ff_ffff) as u32 {
        return None;
    }
    // multiplicative inverse of SCRAMBLE mod 2^32 (Newton's iteration)
    let inv = (0..5).fold(SCRAMBLE, |x, _| x.wrapping_mul(2u32.wrapping_sub(SCRAMBLE.wrapping_mul(x))));
    Some(v.wrapping_mul(inv) ^ 0x2545_f491)
}

// ------------------------------------------------------------------ sessions

/// The character is in play on its game connection; dropping the guard logs it out.
pub fn login(character: u32, name: &str, playfield: u32, outbox: world::Outbox) -> Guard {
    let mut st = STATE.lock().unwrap();
    st.online.insert(character, Online { name: name.to_string(), playfield, outbox });
    let friends = friends_of(character);
    friends_update(&st, character, &friends);
    for &f in &friends {
        friends_update(&st, f, &[character]);
    }
    if let Some(&team) = st.team_of.get(&character) {
        for &m in &st.teams[&team].members {
            if m != character {
                let args = char_id(character).write(Writer::new()).bool(true).finish();
                send_cs(&st, m, cs::TEAM_MEMBER_LINK_DEAD_UPDATE, args);
            }
        }
    }
    info!(character, name, "community: online");
    Guard(character)
}

pub struct Guard(u32);

impl Drop for Guard {
    fn drop(&mut self) {
        let character = self.0;
        let mut st = STATE.lock().unwrap();
        st.online.remove(&character);
        st.invites.retain(|&a, &mut b| a != character && b != character);
        for &f in &friends_of(character) {
            friends_update(&st, f, &[character]);
        }
        if let Some(&team) = st.team_of.get(&character) {
            let members = st.teams[&team].members.clone();
            if members.iter().all(|m| !st.online.contains_key(m)) {
                // nobody left online: the team ends
                for m in &members {
                    st.team_of.remove(m);
                    crate::chat::set_team(*m, None);
                }
                st.teams.remove(&team);
            } else {
                for &m in &members {
                    let args = char_id(character).write(Writer::new()).bool(false).finish();
                    send_cs(&st, m, cs::TEAM_MEMBER_LINK_DEAD_UPDATE, args);
                }
            }
        }
        info!(character, "community: offline");
    }
}

/// After every playfield entry (the client's CharInPlayIIR_t): live sent YouAreNotInTeam when
/// not in a team; in a team the full team (YouJoinedTeam, no feedback text) and the group IIR.
pub fn entered_playfield(character: u32, playfield: u32) {
    entered_playfield_locked(character, playfield);
    flush();
}

fn entered_playfield_locked(character: u32, playfield: u32) {
    let mut st = STATE.lock().unwrap();
    if let Some(o) = st.online.get_mut(&character) {
        o.playfield = playfield;
    }
    match st.team_of.get(&character).copied() {
        None => send_cs(&st, character, cs::YOU_ARE_NOT_IN_TEAM, Bytes::new()),
        Some(team) => {
            send_cs(&st, character, cs::YOU_JOINED_TEAM, team_info(&st, team).bool(false).finish());
            for &m in &st.teams[&team].members {
                if m != character {
                    let ge = ge_args(&st, character).finish();
                    let args = Id32::new(TEAM_TYPE, team).write(Writer::new()).raw(&ge).finish();
                    send_cs(&st, m, cs::TEAM_MEMBER_UPDATE, args);
                }
            }
            // teammates already on this playfield, and this character for them
            let members = st.teams[&team].members.clone();
            group_records(&st, team, true, &members);
        }
    }
    let friends = friends_of(character);
    for &f in &friends {
        friends_update(&st, f, &[character]);
    }
}

// ------------------------------------------------------------------ requests

/// `CSPlayerAgent` requests. Returns false for methods not handled here.
pub fn handle_cs(character: u32, method: u32, r: &mut Reader) -> anyhow::Result<bool> {
    let handled = handle_cs_locked(character, method, r);
    flush();
    handled
}

fn handle_cs_locked(character: u32, method: u32, r: &mut Reader) -> anyhow::Result<bool> {
    match method {
        cs_player_agent::TEAM_INVITE => {
            let target = Id32::read(r)?.instance;
            invite(character, target);
        }
        cs_player_agent::ACCEPT_TEAM_INVITE => accept(character, Id32::read(r)?.instance),
        cs_player_agent::DECLINE_TEAM_INVITE | cs_player_agent::BLOCK_OR_IGNORE_TEAM_INVITE => {
            let inviter = Id32::read(r)?.instance;
            let mut st = STATE.lock().unwrap();
            if st.invites.get(&character) == Some(&inviter) {
                st.invites.remove(&character);
                let args = ge_args(&st, character).finish();
                send_cs(&st, inviter, cs::TARGET_DECLINED_TEAM_INVITE, args);
            }
        }
        cs_player_agent::SET_NEW_TEAM_LEADER => {
            let new = Id32::read(r)?.instance;
            let mut st = STATE.lock().unwrap();
            if let Some(&team) = st.team_of.get(&character)
                && st.teams[&team].leader == character
                && st.teams[&team].members.contains(&new)
            {
                st.teams.get_mut(&team).unwrap().leader = new;
                set_leader(&st, team, new);
            }
        }
        cs_player_agent::SEND_TEAM_INFO => {
            let team = Id32::read(r)?.instance;
            let st = STATE.lock().unwrap();
            if st.team_of.get(&character) == Some(&team) {
                send_cs(&st, character, cs::YOU_JOINED_TEAM, team_info(&st, team).bool(false).finish());
            }
        }
        cs_player_agent::SIGNAL_ADD_FRIEND_BY_ID => add_friend(character, Id32::read(r)?.instance),
        cs_player_agent::ACCEPT_FRIEND_INVITE => {
            let inviter = Id32::read(r)?.instance;
            let invited = STATE.lock().unwrap().friend_invites.get_mut(&character).is_some_and(|s| s.remove(&inviter));
            if invited {
                befriend(character, inviter);
            }
        }
        cs_player_agent::DECLINE_FRIEND_INVITE => {
            let inviter = Id32::read(r)?.instance;
            if let Some(s) = STATE.lock().unwrap().friend_invites.get_mut(&character) {
                s.remove(&inviter);
            }
        }
        cs_player_agent::SIGNAL_REMOVE_FRIEND_BY_ID => {
            let other = Id32::read(r)?.instance;
            let removed = state::or_log(state::db().remove_friendship(character, other), "remove friend", false);
            if removed {
                let st = STATE.lock().unwrap();
                friends_removed(&st, character, other);
                friends_removed(&st, other, character);
                info!(character, other, "friends: removed");
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `GameCharAgentProject` RequestMyFriendKey: the friend code (RequestMyFriendKeyReply).
pub fn my_friend_code(character: u32) {
    let st = STATE.lock().unwrap();
    let args = Writer::new().string(&friend_code(character)).finish();
    send(&st, character, Endpoint::GameCharInterfaceProject, game_char_interface_project::REQUEST_MY_FRIEND_KEY_REPLY, args);
}

/// `GameCharAgentProject` RequestAddFriendWithKey(code): RequestAddFriendWithKeyReply(u32) with
/// the friend's character instance (0: no such code); the client then sends SignalAddFriendByID.
pub fn add_friend_with_code(character: u32, code: &str) {
    let target = decode_friend_code(code).filter(|&t| t != character && exists(t));
    let mut st = STATE.lock().unwrap();
    if let Some(t) = target {
        st.by_code.insert((character, t));
    }
    info!(character, code, ?target, "friends: code");
    let args = Writer::new().u32(target.unwrap_or(0)).finish();
    send(&st, character, Endpoint::GameCharInterfaceProject, game_char_interface_project::REQUEST_ADD_FRIEND_WITH_KEY_REPLY, args);
}

/// SignalAddFriendByID: with a friend code (both sides agreed by sharing it) at once, otherwise
/// (the player context menu) as an invitation the other player accepts.
fn add_friend(character: u32, target: u32) {
    if target == character || friends_of(character).contains(&target) {
        return;
    }
    let mut st = STATE.lock().unwrap();
    if st.by_code.remove(&(character, target)) {
        drop(st);
        befriend(character, target);
    } else if st.online.contains_key(&target) {
        st.friend_invites.entry(target).or_default().insert(character);
        let args = ge_args(&st, character).finish();
        send_cs(&st, target, cs::FRIEND_INVITE_FROM, args);
    }
}

fn befriend(a: u32, b: u32) {
    if !state::or_log(state::db().add_friendship(a, b), "add friend", false) {
        return;
    }
    let st = STATE.lock().unwrap();
    friends_update(&st, a, &[b]);
    friends_update(&st, b, &[a]);
    info!(a, b, "friends: added");
}

fn invite(character: u32, target: u32) {
    let mut st = STATE.lock().unwrap();
    if target == character || !st.online.contains_key(&target) {
        return;
    }
    let my_team = st.team_of.get(&character).copied();
    if let Some(team) = my_team {
        let t = &st.teams[&team];
        if t.leader != character {
            return;
        }
        if t.members.len() >= MAX_TEAM_SIZE {
            send_cs(&st, character, cs::TEAM_IS_FULL, Bytes::new());
            return;
        }
    }
    if st.team_of.contains_key(&target) {
        let args = ge_args(&st, target).finish();
        send_cs(&st, character, cs::TARGET_IS_ALREADY_IN_TEAM, args);
        return;
    }
    st.invites.insert(target, character);
    let args = ge_args(&st, character).string("").finish();
    send_cs(&st, target, cs::TEAM_INVITE, args);
    let args = ge_args(&st, target).finish();
    send_cs(&st, character, cs::YOU_INVITED_TO_TEAM, args);
    info!(character, target, "team: invite");
}

fn accept(character: u32, inviter: u32) {
    let mut st = STATE.lock().unwrap();
    if st.invites.get(&character) != Some(&inviter) || st.team_of.contains_key(&character) {
        return;
    }
    st.invites.remove(&character);
    let (team, fresh) = match st.team_of.get(&inviter).copied() {
        Some(t) if st.teams[&t].members.len() < MAX_TEAM_SIZE => (t, false),
        Some(_) => {
            send_cs(&st, character, cs::TEAM_IS_FULL, Bytes::new());
            return;
        }
        None => {
            let t = st.next_team;
            st.next_team += 1;
            st.teams.insert(t, Team { leader: inviter, members: vec![inviter] });
            st.team_of.insert(inviter, t);
            (t, true)
        }
    };
    st.teams.get_mut(&team).unwrap().members.push(character);
    st.team_of.insert(character, team);
    let info = team_info(&st, team).bool(true).finish();
    send_cs(&st, character, cs::YOU_JOINED_TEAM, info.clone());
    if fresh {
        send_cs(&st, inviter, cs::YOU_JOINED_TEAM, info);
    } else {
        for &m in &st.teams[&team].members {
            if m != character {
                let args = ge_args(&st, character).finish();
                let args = Id32::new(TEAM_TYPE, team).write(Writer::new()).raw(&args).bool(true).finish();
                send_cs(&st, m, cs::TEAM_JOIN, args);
            }
        }
    }
    let members = st.teams[&team].members.clone();
    group_records(&st, team, true, &members);
    for &m in &members {
        crate::chat::set_team(m, Some(team));
    }
    info!(team, ?members, "team: joined");
}

/// What a player did on `playfield` that counts for the teammates there too (any distance):
/// defeats (kill XP, counters, named enemies and bosses), smashes, builds and a pocket world's
/// clear. Visits and finds (location discovery) aren't shared.
pub fn share(player: u32, playfield: u32, event: &crate::achievements::Event) {
    for mate in teammates(player) {
        let mate = Id32::new(CHARACTER_TYPE, mate);
        if world::whereabouts(mate).is_some_and(|(pf, _)| pf == playfield) {
            world::send_to(mate, world::Out::TeamProgress(event.clone()));
        }
    }
}

/// `!team <name>`: the online character of that name joins this character's team (made with
/// this character as the leader) as if it had accepted an invite. Returns its name.
pub fn team_up(character: u32, name: &str) -> Result<String, String> {
    let target = {
        let st = STATE.lock().unwrap();
        let (&target, o) = st
            .online
            .iter()
            .find(|(c, o)| **c != character && o.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no other character named {name} is online"))?;
        if st.team_of.contains_key(&target) {
            return Err(format!("{} is in a team already", o.name));
        }
        if st.team_of.get(&character).is_some_and(|t| st.teams[t].leader != character) {
            return Err("only the team's leader can add members".into());
        }
        target
    };
    invite(character, target);
    accept(target, character);
    flush();
    let st = STATE.lock().unwrap();
    match st.team_of.get(&target) {
        Some(_) => Ok(st.online[&target].name.clone()),
        None => Err("the team is full".into()),
    }
}

fn set_leader(st: &State, team: u32, leader: u32) {
    for &m in &st.teams[&team].members {
        let args = ge_args(st, leader).finish();
        let args = Id32::new(TEAM_TYPE, team).write(Writer::new()).raw(&args).bool(true).finish();
        send_cs(st, m, cs::SET_NEW_TEAM_LEADER, args);
    }
    let members = st.teams[&team].members.clone();
    group_records(st, team, true, &members);
}

/// `GameCharAgent` LeaveTeam.
pub fn leave(character: u32) {
    leave_locked(character);
    flush();
}

fn leave_locked(character: u32) {
    let mut st = STATE.lock().unwrap();
    remove_member(&mut st, character);
}

/// `GameCharAgent` KickFromTeam(ID32): leader only.
pub fn kick(character: u32, target: u32) {
    kick_locked(character, target);
    flush();
}

fn kick_locked(character: u32, target: u32) {
    let mut st = STATE.lock().unwrap();
    let Some(&team) = st.team_of.get(&character) else { return };
    if st.teams[&team].leader != character || target == character || st.team_of.get(&target) != Some(&team) {
        return;
    }
    let args = ge_args(&st, character).finish();
    send_cs(&st, target, cs::YOU_GOT_KICKED_FROM_TEAM, args);
    remove_member(&mut st, target);
}

fn remove_member(st: &mut State, character: u32) {
    let Some(team) = st.team_of.get(&character).copied() else { return };
    let ge = ge_args(st, character).finish();
    for &m in &st.teams[&team].members {
        let args = Id32::new(TEAM_TYPE, team).write(Writer::new()).raw(&ge).bool(true).finish();
        send_cs(st, m, cs::TEAM_LEAVE, args);
    }
    group_records(st, team, false, &[character]);
    let t = st.teams.get_mut(&team).unwrap();
    t.members.retain(|&m| m != character);
    st.team_of.remove(&character);
    crate::chat::set_team(character, None);
    let (members, leader) = (t.members.clone(), t.leader);
    if members.len() <= 1 {
        for &m in &members {
            send_cs(st, m, cs::TEAM_DISBAND, Id32::new(TEAM_TYPE, team).write(Writer::new()).finish());
            st.team_of.remove(&m);
            crate::chat::set_team(m, None);
        }
        group_records(st, team, false, &members);
        st.teams.remove(&team);
        info!(team, "team: disbanded");
    } else if leader == character {
        let new = members[0];
        st.teams.get_mut(&team).unwrap().leader = new;
        set_leader(st, team, new);
    }
    info!(team, character, "team: left");
}

/// `GameCharAgent` RequestTeamMemberPosList(vector<u32>): the map markers,
/// UpdateTeamMemberPosList(vector<{u32 instance, i32 x, y, z}>) of the teammates on the same
/// playfield.
pub fn member_positions(character: u32, r: &mut Reader) -> anyhow::Result<()> {
    let n = r.container_len()?;
    let wanted: Vec<u32> = (0..n).map(|_| r.u32()).collect::<Result<_, _>>()?;
    let st = STATE.lock().unwrap();
    let mates: Vec<(u32, [f32; 3])> = wanted
        .into_iter()
        .filter(|m| st.team_of.get(m).is_some() && st.team_of.get(m) == st.team_of.get(&character))
        // only teammates on this playfield: the map shows positions on its own playfield
        .filter(|m| {
            let here = st.online.get(&character).map(|o| o.playfield);
            here.is_some() && st.online.get(m).map(|o| o.playfield) == here
        })
        .filter_map(|m| world::position_of(char_id(m)).map(|p| (m, p)))
        .collect();
    let w = mates.iter().fold(Writer::new().container_len(mates.len()), |w, (m, p)| {
        w.u32(*m).i32(p[0] as i32).i32(p[1] as i32).i32(p[2] as i32)
    });
    send(&st, character, Endpoint::GameCharInterface, game_char_interface::UPDATE_TEAM_MEMBER_POS_LIST, w.finish());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friend_codes_round_trip() {
        for i in [1u32, 3208616, 93819121, 1270314175, 1974518773, 0x7fff_ffff] {
            let code = friend_code(i);
            assert_eq!(code.len(), 15, "{code}");
            assert_eq!(decode_friend_code(&code), Some(i), "{code}");
            assert_eq!(decode_friend_code(&code.to_lowercase()), Some(i));
        }
        assert_eq!(decode_friend_code("222-222-222-222"), None);
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<world::Out>) -> Vec<(u32, Bytes)> {
        let mut out = Vec::new();
        while let Ok(world::Out::Rpc(_, m, args)) = rx.try_recv() {
            out.push((m, args));
        }
        out
    }

    #[test]
    fn team_invite_accept_leave() {
        crate::state::init_for_tests();
        let (a, b) = (900_001, 900_002);
        let (ta, mut ra) = tokio::sync::mpsc::unbounded_channel();
        let (tb, mut rb) = tokio::sync::mpsc::unbounded_channel();
        let _ga = login(a, "hosttemple", 1200, ta);
        let _gb = login(b, "friendtemple", 1200, tb);
        entered_playfield(a, 1200);
        assert_eq!(drain(&mut ra).iter().map(|m| m.0).collect::<Vec<_>>(), [cs::YOU_ARE_NOT_IN_TEAM]);
        drain(&mut rb);

        let invite_args = char_id(b).write(Writer::new()).string("").bool(true).finish();
        handle_cs(a, cs_player_agent::TEAM_INVITE, &mut Reader::new(&invite_args)).unwrap();
        assert_eq!(drain(&mut rb).iter().map(|m| m.0).collect::<Vec<_>>(), [cs::TEAM_INVITE]);
        assert_eq!(drain(&mut ra).iter().map(|m| m.0).collect::<Vec<_>>(), [cs::YOU_INVITED_TO_TEAM]);

        let accept_args = char_id(a).write(Writer::new()).finish();
        handle_cs(b, cs_player_agent::ACCEPT_TEAM_INVITE, &mut Reader::new(&accept_args)).unwrap();
        let got = drain(&mut rb);
        assert_eq!(got.iter().map(|m| m.0).collect::<Vec<_>>(), [cs::YOU_JOINED_TEAM]);
        // TeamInfo: team, leader a, two members, loot fields, feedback
        let mut r = Reader::new(&got[0].1);
        assert_eq!(Id32::read(&mut r).unwrap().kind, TEAM_TYPE);
        assert_eq!(Id32::read(&mut r).unwrap(), char_id(a));
        assert_eq!(r.container_len().unwrap(), 2);
        for _ in 0..2 {
            let n = r.u32().unwrap() as usize;
            r.bytes(n, "ge").unwrap();
        }
        r.u32().unwrap();
        r.bool().unwrap();
        r.bool().unwrap();
        Id32::read(&mut r).unwrap();
        r.u32().unwrap();
        assert!(r.bool().unwrap());
        assert!(r.remaining().is_empty());
        assert_eq!(drain(&mut ra).iter().map(|m| m.0).collect::<Vec<_>>(), [cs::YOU_JOINED_TEAM]);
        assert_eq!(teammates(a), [b]);
        assert!(team_ids(b).is_some());

        leave(b);
        assert_eq!(drain(&mut rb).iter().map(|m| m.0).collect::<Vec<_>>(), [cs::TEAM_LEAVE]);
        assert_eq!(drain(&mut ra).iter().map(|m| m.0).collect::<Vec<_>>(), [cs::TEAM_LEAVE, cs::TEAM_DISBAND]);
        assert!(team_ids(a).is_none() && team_ids(b).is_none());
    }
}
