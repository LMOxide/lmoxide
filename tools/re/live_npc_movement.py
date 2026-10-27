#!/usr/bin/env python3
"""Decode the server -> client NPC movement of a live capture: MoveTaskIIR_c, CharDCMoveIIR_t,
StopIIR_c, LookAtIIR_t (and the BehaviorControlCenterIIR_c action byte) per NPC, with a
per-NPC and per-kind summary (home, range, patrol loops, speed, chasing).

Usage:
    python3 tools/re/live_npc_movement.py <capture.cap> [--jsonl rows.jsonl]
        [--objects data/live-capture/lmo-captures/playfield-objects.json]
        [--out data/live-capture/lmo-captures/npc-movement.json]

The capture is decoded with tools/re/livecap.py (`--jsonl`), unless --jsonl names an existing
dump (then the capture argument may be omitted). The NPCs (name, monster_data, spawn position)
and the zone visits come from tools/re/live_playfield.py's playfield-objects.json: a record
belongs to the zone whose PlayfieldFIIR_t its TCP flow carried last (`playfields[].rpc` =
1-based ordinal of the PassBlob among all S->C PassBlobs). An NPC is a type-50000 character with
instance < 16,000,000; above that it is a player.

Record framing: GameserverToClientRPC::PassBlob = u32 BE length, then IIR records, each u32 type
id (data/iir_types.json), ID32 (u32 type, u32 instance) = the dynel the record is about, u8
pass-on, class data. All big-endian (BinaryStream with the swap flag set). A record's length is
not stored; a parse is accepted ("exact") only if it ends exactly at the blob end or where
another record of a type parsed here starts. All layouts below are from the native Linux client
(symbol names; tools/re/linux_fn.sh). Names in quotes come from the client's own log/ToString
strings or accessor names; "?" marks a field whose meaning is not verified (its size and
position are).

MoveTaskIIR_c (0x96c46740) — MoveTaskIIR_c::ReadSubClass:
    u8 bool (?), f32 x, y, z ("start": see the note at the end), then MoveTaskFactory::Read:
    u8 MoveTaskType_e + the task's Read (vtable +0x90).
    MoveTaskType_e (MoveTaskFactory::Initialize, and a static initialiser at 0x17781d9 for 12
    and 13, register each class's Instantiate()): 0 Wait, 1 Path, 2 Rotate, 3 Jump, 4 List,
    5 Ballistic, 6 Animated, 7 Formation, 8 Track, 9 Link, 10 SetPos, 11 GroundAlign,
    12 ParameterizedBallistic, 13 KnockUp, 14 ActiveDodge, 15 Attack.
  Wait (WaitMoveTask::Read; also the base part of Path, List, Link, Ballistic, KnockUp): u8
    flags; bit0 set: "duration" = "remaining" = FLT_MAX (forever), no floats follow; bit0 clear:
    f32 "duration", f32 "remaining" (the client rejects remaining > duration or negatives);
    bit1: a bool (?).
  Path (PathMoveTask::Read): Wait part; u16 flags; bit4: f32 "minbasespeed" follows; bit5: f32
    "maxspeed" follows (else FLT_MAX; PathMoveTask::SetMaxSpeed writes this field); speedup
    (MoveTaskSpeedup_e, GetSpeedup) = 2 if bit2, else 1 if bit1, else 3 if bit9, else 0; facing
    (PathMoveTaskFacing_e) = 4 if bit3, else 1 if bit6, else 2 if bit7, else 3 if bit8, else 0.
    bit0 set: u16, u16 (?: the PathMoveTask(SplinePath, float, u16, u16) constructor; no points);
    bit0 clear: u32 n, n vec3 points (SplinePath::SetPoints). Then a SplinePath.
  SplinePath (operator>>(BinaryStream&, SplinePath&)): u8 "loop" (SplinePath::IsComplete is never
    true while set; PathMoveTask::ToString prints " loop"), u32 current point index
    (IsComplete: index >= points - 1), u8 (?), f32 (? progress in the current segment:
    SplinePath::Update resets it to 0 per segment), f32 (? segment length: Update divides by it).
  List (ListMoveTask::Read): Wait part, u8 "idx" (current child), u8 "size" (idx <= size), then
    size child tasks, each u8 type + body (MoveTaskFactory::Read, recursive).
  Track (TrackMoveTask::Read): ID32 target (n3DynelPtrProxy), u8 bool (?), f32 (?), f32 (?),
    u8 bool (?).
  Attack (AttackMoveTask::Read): SplinePath, u32 n, n vec3 points, u32 (?), ID32 target, u32 (?).
    The client's own click-to-move is this type (target 0).
  Link (LinkMoveTask::Read): Wait part, ID32 (? linked dynel).
  Rotate (RotateMoveTask::Read): f32 x3 (?), quaternion x, y, z, w ("rot"), u8 bool, u8 bool (?).
  SetPos (SetPosMoveTask::Read): 7 f32 (? position + quaternion; the client checks nothing).
  Formation (FormationMoveTask::Read): ID32 (? leader), f32 x, y, z "offset", f32 (?).
  Ballistic (BallisticMoveTask::Read): Wait part, f32 (?), vec3 "from", vec3 "to", f32 x3 (?),
    u8 bool (?). ParameterizedBallistic: Ballistic + u8, f32, f32, u8 bool, u8 bool (?);
    KnockUp: Wait part, 7 f32 (?), u8 bool (?).
  Jump: u8 (?). GroundAlign: u8 bool (?). Animated: nothing. ActiveDodge: Path + f32 (?).
  Note: the header vec3 is called "start" because it equals the NPC's position when the task is
  sent (the previous task's end point, Stop positions); MoveTaskIIR_c::Activate was not read.

CharDCMoveIIR_t (0x4517aa81) — PlayerMoveUpdateData::Read: u8 flags; vec3 position; if flags
    bit4: f32 rot y, f32 rot w (x = z = 0), else quaternion x, y, z, w; u8 alignment mode (<= 7);
    if flags bit2 clear: vec3 velocity; if flags bit3 clear: vec3 (?) + u8 bool (?); f32 (?
    rotation speed, per game.rs move_record); if alignment mode == 1: u16 (?); u8 bool, u8 bool
    (?). flags bit5: a bool (?).
StopIIR_c (0xd751ebb4) — StopIIR_c::ReadSubClass: 7 f32 (position + quaternion: what the
    server's dash.rs stop_record writes, and the recorded values fit), u8 bool, u8 bool (?).
LookAtIIR_t (0x0d155695) — LookAtIIR_t::ReadSubClass: ID32 target, u32 (?), u32 (?)
    (constructor LookAtIIR_t(CoreCharacter const&, ID32 const&, int, int)).
BehaviorControlCenterIIR_c (0x642cd3d6) — BehaviorControlCenterIIR_c::ReadSubClass: u8 action
    (BccIIRAction_e; switch table 0x2f324b0): 0, 1, 6, 8, 9, 13, 17, 19 and > 20: nothing more;
    11, 12: u32 (the substate: data/bcc_states.json); 14: u32 (?) and, if non-zero, a command
    as below; 2, 3, 4, 5, 7, 10, 15, 16, 18, 20: u32 CommandType_e and, if non-zero, a command:
    Command_c::InstantiateCommand(type), Command_c::ReadBaseClass (BinaryStream::ReadProtobuf:
    u32 length + CommandMessage protobuf, proto/CommandMessage.proto), then the class's
    ReadSubClass (vtable +0x100). CommandType_e -> class from the Command_c::RegisterCommand
    calls (COMMAND_TYPES below). Subclass layouts decoded here:
      BCCActionCommand_c (2, 7), LimboCommand_c (31): vtable +0x100 is Command_c::ReadSubClass,
        which reads nothing.
      TransitionCommand_c (1): u32 x4 (?), u32 (== 1 sets a bool) (?).
      MagicCommand_c (4) (MagicCommand_c::ReadSubClass): u64 whose low u32 (the second on the
        wire) is the spell (passed to SpellTemplate_t::GetMagicStatSpell); u8: unless 1, 3 x
        4 bytes (?) follow; u32 x3 (?); u8 bool x4 (?); ID32 (?); u32 (?); u32: if 1, 7 u32
        (?) follow.
      UseDynelCommand (18): ID32 (? the dynel used), u32 (?). ReviveCommand_c (37): u32 x3 (?).
      CycleToNextWeaponCommand (48): u8 bool (?), u32 (?).
    (Each class's vtable +0x100 entry was checked to be the ReadSubClass named.)
    Other classes (EquipItemCommand_c = CoreEquipItemCommand_c::ReadSubClass,
    ObjectInteractionCommand with strings, ...) are not decoded: their records count as
    "body_not_decoded", with the byte count kept.

Output (npc-movement.json): per zone visit, per NPC, the time-ordered events (t = seconds from
capture start, from the jsonl ts_raw in 100 ns ticks), then a per-NPC summary and an aggregate
per (name, monster_data). Observations the summary rests on (from the records, not the client
code): the server re-issues a patrolling NPC's whole MoveTask about every 1.5 s, "start" = where
the NPC is then, spline state advanced; a chasing NPC gets a new List[Path, Track] (path from
where it is towards the player, then Track on the player) every ~50 ms; after a LimboCommand
(smashed) the NPC gets List[Animated, GroundAlign]. A task sent when an NPC comes into view
keeps its original start (Monster Laboratory waves: the first List starts at the spawn point
and the next record, 0.36 s later, has the NPC 16 m on), so "start" = where the task began.
Summary terms:
  home: the spawn position from playfield-objects.json (else the first position seen).
  phases: per MoveTask record: chase (a task targets a player), track_npc (a task targets
    another NPC: Monster Laboratory waves attack an NPC), smashed (List[Animated, GroundAlign]),
    patrol (a Path with the SplinePath "loop" flag), move (another Path), other.
  range_m / range_m_not_chasing: max horizontal (x, z) distance from home of any start, path
    point, Stop or CharDCMove position (of all records / of patrol and move records only).
  patrol_routes: per distinct looping Path: its points (the loop; the client never completes
    it), closed length, how many records re-sent it, the approach path (List child before it,
    from home to the first loop point), and speed_lower_bound_mps: median over consecutive
    re-sends 1-4 s apart of straight displacement / time. A lower bound: the NPC can turn at a
    loop point between two samples. (The SplinePath index could tell the segment, but its
    meaning did not fit all NPCs, so it is not used.)
  move_speed_by_arrival_mps: length of a non-looping path / time until the NPC's next
    MoveTask/Stop starts at its end point, for paths >= 2 m and >= 0.5 s (an underestimate
    when it paused there). No chase speed is given: chase steps between ~50 ms records came out
    at 25-45 m/s against run-speed stats of 2.8-5.6, so arrival times of those bursts do not
    measure movement (TCP arrival; the server stream had stalls).
  move_destinations, went_home: distinct end points of move paths; whether one is home (1.5 m).
  targeted_by / targeted_players: Track/Attack/Link/Formation/LookAt targets that are players
    (instance >= 16,000,000).
  behaviour: patrol (has a looping path), "chase only" / "path + chase" (targeted a player or
    NPC and moved > 1 m from home; without / with other paths), "path only" (moved > 1 m on
    non-looping paths), stationary.
"""
import argparse
import json
import math
import os
import re
import statistics
import struct
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LIVECAP = ROOT / "tools" / "re" / "livecap.py"
DEFAULT_OBJECTS = ROOT / "data" / "live-capture" / "lmo-captures" / "playfield-objects.json"
DEFAULT_OUT = ROOT / "data" / "live-capture" / "lmo-captures" / "npc-movement.json"

T_MOVE_TASK = 0x96C46740
T_CHAR_DC_MOVE = 0x4517AA81
T_STOP = 0xD751EBB4
T_LOOK_AT = 0x0D155695
T_BCC = 0x642CD3D6
RECORD_NAMES = {T_MOVE_TASK: "MoveTaskIIR_c", T_CHAR_DC_MOVE: "CharDCMoveIIR_t", T_STOP: "StopIIR_c",
                T_LOOK_AT: "LookAtIIR_t", T_BCC: "BehaviorControlCenterIIR_c"}
CHARACTER = 50000
PLAYER_MIN = 16_000_000
FLT_MAX = 3.4028234663852886e38
TASK_NAMES = {0: "Wait", 1: "Path", 2: "Rotate", 3: "Jump", 4: "List", 5: "Ballistic",
              6: "Animated", 7: "Formation", 8: "Track", 9: "Link", 10: "SetPos",
              11: "GroundAlign", 12: "ParameterizedBallistic", 13: "KnockUp",
              14: "ActiveDodge", 15: "Attack"}
# BehaviorControlCenterIIR_c::ReadSubClass switch (table 0x2f324b0)
BCC_SUBSTATE = {11, 12}
BCC_COMMAND = {2, 3, 4, 5, 7, 10, 15, 16, 18, 20}
BCC_U32_THEN_COMMAND = {14}


class Short(Exception):
    """The record does not fit the layout (ran past the blob end or failed a client check)."""


class Reader:
    """Big-endian BinaryStream reader with a hard end."""

    def __init__(self, b, p, end):
        self.b, self.p, self.end = b, p, end

    def take(self, n):
        if self.p + n > self.end:
            raise Short()
        v = self.b[self.p:self.p + n]
        self.p += n
        return v

    def u8(self):
        return self.take(1)[0]

    def u16(self):
        return struct.unpack(">H", self.take(2))[0]

    def u32(self):
        return struct.unpack(">I", self.take(4))[0]

    def f32(self):
        return struct.unpack(">f", self.take(4))[0]

    def vec3(self):
        return [self.f32(), self.f32(), self.f32()]

    def id32(self):
        return [self.u32(), self.u32()]


def r4(v):
    if isinstance(v, list):
        return [r4(x) for x in v]
    if isinstance(v, float):
        if v >= FLT_MAX:
            return "FLT_MAX"
        if not math.isfinite(v):
            return str(v)
        return round(v, 4)
    return v


# ------------------------------------------------------------------ move tasks

def read_wait(r):
    flags = r.u8()
    d = {"wait_flags": flags}
    if flags & 1:
        d["duration"] = d["remaining"] = "forever"
    else:
        dur, rem = r.f32(), r.f32()
        if not (dur >= 0 and 0 <= rem <= dur):  # WaitMoveTask::Read's check
            raise Short()
        d["duration"], d["remaining"] = r4(dur), r4(rem)
    if flags & 2:
        d["wait_bit1"] = True
    return d


def read_spline(r):
    return {"loop": r.u8() != 0, "index": r.u32(), "u8_unknown": r.u8(),
            "seg_f32_a": r4(r.f32()), "seg_f32_b": r4(r.f32())}


def read_points(r):
    n = r.u32()
    if n * 12 > r.end - r.p:
        raise Short()
    return [r4(r.vec3()) for _ in range(n)]


def read_path(r):
    d = read_wait(r)
    fl = r.u16()
    d["path_flags"] = fl
    d["speedup"] = 2 if fl & 4 else 1 if fl & 2 else 3 if fl & 0x200 else 0
    d["facing"] = 4 if fl & 8 else 1 if fl & 0x40 else 2 if fl & 0x80 else 3 if fl & 0x100 else 0
    if fl & 0x10:
        d["minbasespeed"] = r4(r.f32())
    if fl & 0x20:
        d["maxspeed"] = r4(r.f32())
    if fl & 1:
        d["u16_pair"] = [r.u16(), r.u16()]
    else:
        d["points"] = read_points(r)
    d["spline"] = read_spline(r)
    return d


def read_task(r, depth=0):
    if depth > 8:
        raise Short()
    t = r.u8()
    name = TASK_NAMES.get(t)
    if name is None:
        raise Short()
    d = {"task": name}
    if t == 0:
        d.update(read_wait(r))
    elif t == 1:
        d.update(read_path(r))
    elif t == 14:
        d.update(read_path(r))
        d["f32_unknown"] = r4(r.f32())
    elif t == 2:
        d["f32_unknown"] = r4([r.f32(), r.f32(), r.f32()])
        d["rot"] = r4([r.f32(), r.f32(), r.f32(), r.f32()])
        d["u8_unknown"] = [r.u8(), r.u8()]
    elif t == 3:
        d["u8_unknown"] = r.u8()
    elif t == 4:
        d.update(read_wait(r))
        d["idx"], n = r.u8(), r.u8()
        if d["idx"] > n:
            raise Short()
        d["children"] = [read_task(r, depth + 1) for _ in range(n)]
    elif t in (5, 12):
        d.update(read_wait(r))
        d["f32_unknown"] = r4(r.f32())
        d["from"], d["to"] = r4(r.vec3()), r4(r.vec3())
        d["vec3_unknown"] = r4(r.vec3())
        d["u8_unknown"] = r.u8()
        if t == 12:
            d["param_unknown"] = [r.u8(), r4(r.f32()), r4(r.f32()), r.u8(), r.u8()]
    elif t == 6:
        pass
    elif t == 7:
        d["target"] = r.id32()
        d["offset"] = r4(r.vec3())
        d["f32_unknown"] = r4(r.f32())
    elif t == 8:
        d["target"] = r.id32()
        d["u8_a"] = r.u8()
        d["f32_unknown"] = r4([r.f32(), r.f32()])
        d["u8_b"] = r.u8()
    elif t == 9:
        d.update(read_wait(r))
        d["target"] = r.id32()
    elif t == 10:
        d["f32x7_unknown"] = r4([r.f32() for _ in range(7)])
    elif t == 11:
        d["u8_unknown"] = r.u8()
    elif t == 13:
        d.update(read_wait(r))
        d["f32x7_unknown"] = r4([r.f32() for _ in range(7)])
        d["u8_unknown"] = r.u8()
    elif t == 15:
        d["spline"] = read_spline(r)
        d["points"] = read_points(r)
        d["u32_a"] = r.u32()
        d["target"] = r.id32()
        d["u32_b"] = r.u32()
    return d


def read_move_task(r):
    return {"u8_unknown": r.u8(), "start": r4(r.vec3()), "task": read_task(r)}


def read_dc_move(r):
    fl = r.u8()
    d = {"flags": fl, "pos": r4(r.vec3())}
    if fl & 0x10:
        y, w = r.f32(), r.f32()
        d["rot"] = r4([0.0, y, 0.0, w])
    else:
        d["rot"] = r4([r.f32() for _ in range(4)])
    mode = r.u8()
    if mode > 7:
        raise Short()
    d["align_mode"] = mode
    if not fl & 4:
        d["vel"] = r4(r.vec3())
    if not fl & 8:
        d["vec3_unknown"] = r4(r.vec3())
        d["u8_unknown"] = r.u8()
    d["f32_rot_speed"] = r4(r.f32())
    if mode == 1:
        d["u16_unknown"] = r.u16()
    d["tail_u8"] = [r.u8(), r.u8()]
    return d


def read_stop(r):
    return {"pos": r4(r.vec3()), "rot": r4([r.f32() for _ in range(4)]), "u8": [r.u8(), r.u8()]}


def read_look_at(r):
    return {"target": r.id32(), "u32_a": r.u32(), "u32_b": r.u32()}


def protobuf(b, fields):
    """Minimal protobuf decoder: {name: value} for varint and length-delimited fields."""
    out, p = {}, 0

    def varint():
        nonlocal p
        v = s = 0
        while True:
            if p >= len(b):
                raise Short()
            c = b[p]
            p += 1
            v |= (c & 0x7F) << s
            s += 7
            if not c & 0x80:
                return v
    while p < len(b):
        key = varint()
        num, wt = key >> 3, key & 7
        name = fields.get(num, f"field_{num}")
        if wt == 0:
            out[name] = varint()
        elif wt == 2:
            n = varint()
            if p + n > len(b):
                raise Short()
            sub = b[p:p + n]
            p += n
            out[name] = protobuf(sub, ID32_FIELDS) if name == "target" else sub.hex()
        else:
            raise Short()
    return out


COMMAND_MESSAGE_FIELDS = {1: "bcc_action_id", 2: "commandqueue_client_id",
                          3: "commandqueue_server_id", 4: "anim_random", 5: "flags",
                          6: "execution_status", 10: "target", 11: "recharge_time",
                          12: "activation_time", 13: "failure_description"}
ID32_FIELDS = {1: "type", 2: "instance"}
# CommandType_e -> class (Command_c::RegisterCommand calls at 0x136970f.. and 0x174fb99..)
COMMAND_TYPES = {1: "TransitionCommand_c", 2: "BCCActionCommand_c", 4: "MagicCommand_c",
                 5: "SocialCommand_c", 7: "BCCActionCommand_c", 9: "EquipItemCommand_c",
                 14: "UseItemCommand_c", 18: "UseDynelCommand", 22: "UseCharacterCommand_c",
                 31: "LimboCommand_c", 36: "ClimbingCommand_c", 37: "ReviveCommand_c",
                 41: "LeaveKnockoutCommand_c", 42: "ObjectInteractionCommand",
                 46: "SwapAlternateEquipmentCommand_c", 48: "CycleToNextWeaponCommand",
                 49: "CraftingCommand"}


def read_magic_command(r):
    """MagicCommand_c::ReadSubClass."""
    d = {"u32_hi": r.u32(), "spell": r.u32()}  # one u64; the low half is the spell
    no_vec = r.u8()
    d["u8_no_vec"] = no_vec
    if no_vec != 1:
        d["vec3_unknown"] = r4([r.f32(), r.f32(), r.f32()])
    d["u32_unknown"] = [r.u32(), r.u32(), r.u32()]
    d["u8_unknown"] = [r.u8(), r.u8(), r.u8(), r.u8()]
    d["id32_unknown"] = r.id32()
    d["u32_c"] = r.u32()
    mode = r.u32()
    d["u32_mode"] = mode
    if mode == 1:
        d["u32x7_unknown"] = [r.u32() for _ in range(7)]
    return d


def read_command(r, ct):
    n = r.u32()
    d = {"command_class": COMMAND_TYPES.get(ct), "message": protobuf(r.take(n), COMMAND_MESSAGE_FIELDS)}
    if ct in (2, 7, 31):  # vtable +0x100 = Command_c::ReadSubClass (reads nothing)
        pass
    elif ct == 1:  # TransitionCommand_c::ReadSubClass: 4 u32, u32 (== 1 -> a bool)
        d["u32_unknown"] = [r.u32() for _ in range(5)]
    elif ct == 4:
        d.update(read_magic_command(r))
    elif ct == 18:  # UseDynelCommand::ReadSubClass
        d["dynel"] = r.id32()
        d["u32_unknown"] = r.u32()
    elif ct == 37:  # ReviveCommand_c::ReadSubClass
        d["u32_unknown"] = [r.u32() for _ in range(3)]
    elif ct == 48:  # CycleToNextWeaponCommand::ReadSubClass
        d["u8_unknown"] = r.u8()
        d["u32_unknown"] = r.u32()
    else:
        d["subclass_bytes_not_decoded"] = r.end - r.p
        r.p = r.end
    return d


def read_bcc(r):
    a = r.u8()
    d = {"action": a}
    if a in BCC_SUBSTATE:
        d["substate"] = r.u32()
    elif a in BCC_COMMAND or a in BCC_U32_THEN_COMMAND:
        if a in BCC_U32_THEN_COMMAND:
            d["u32_unknown"] = r.u32()
            if d["u32_unknown"] == 0:
                return d
        ct = r.u32()
        d["command_type"] = ct
        if ct:
            d["command"] = read_command(r, ct)
    return d


READERS = {T_MOVE_TASK: read_move_task, T_CHAR_DC_MOVE: read_dc_move, T_STOP: read_stop,
           T_LOOK_AT: read_look_at, T_BCC: read_bcc}


def parse_blob(b):
    """(type, ID32, decoded or None, exact) for the records of one PassBlob that start with a
    type this tool reads; stops at the first record it can not frame. exact is None for a BCC
    command record (the command subclass is not decoded, so its end can not be checked)."""
    end = 4 + struct.unpack_from(">I", b, 0)[0]
    p = 4
    out = []
    while p + 13 <= end:
        t, ty, inst = struct.unpack_from(">III", b, p)
        if t not in READERS:
            break
        r = Reader(b, p + 13, end)
        try:
            d = READERS[t](r)
        except Short:
            out.append((t, [ty, inst], None, False))
            break
        nxt = r.p
        exact = nxt == end or (nxt + 13 <= end and struct.unpack_from(">I", b, nxt)[0] in READERS)
        if t == T_BCC and "subclass_bytes_not_decoded" in d.get("command", {}):
            exact = None
        out.append((t, [ty, inst], d if exact is not False else None, exact))
        if not exact:
            break
        p = nxt
    return out


# ------------------------------------------------------------------ analysis

def hdist(a, b):
    return math.hypot(a[0] - b[0], a[2] - b[2])


def is_vec(p):
    return isinstance(p, list) and len(p) == 3 and all(isinstance(x, (int, float)) for x in p)


def paths_in(task):
    """Sub-tasks with path points (Path, ActiveDodge, Attack) of a task tree, in order."""
    if task["task"] in ("Path", "ActiveDodge", "Attack") and task.get("points"):
        yield task
    for c in task.get("children", []):
        yield from paths_in(c)


def targets_in(task):
    if "target" in task:
        yield task["task"], task["target"]
    for c in task.get("children", []):
        yield from targets_in(c)


def positions_of(ev):
    if ev["record"] == "MoveTask":
        yield ev["start"]
        for p in paths_in(ev["task"]):
            yield from p["points"]
    elif "pos" in ev:
        yield ev["pos"]


def key05(p):
    return (round(p[0] * 2) / 2, round(p[2] * 2) / 2)


def current_path(task):
    """The path-like task being run: the task itself, or a List's child at idx."""
    if task["task"] in ("Path", "ActiveDodge", "Attack") and task.get("points"):
        return task
    ch = task.get("children")
    if ch and task["idx"] < len(ch):
        return current_path(ch[task["idx"]])
    return None


def route_key(points):
    return tuple((round(p[0], 1), round(p[1], 1), round(p[2], 1)) for p in points)


def phase(ev):
    """What a MoveTask record shows the NPC doing (see the module docstring)."""
    t = ev["task"]
    tgts = [tg for _, tg in targets_in(t)]
    if any(tg[1] >= PLAYER_MIN for tg in tgts):
        return "chase"
    if any(tg[1] for tg in tgts):
        return "track_npc"
    if [c["task"] for c in t.get("children", [])] == ["Animated", "GroundAlign"]:
        return "smashed"
    if any(p.get("spline", {}).get("loop") for p in paths_in(t)):
        return "patrol"
    if current_path(t):
        return "move"
    return "other"


def closed_length(pts):
    return sum(math.dist(pts[i], pts[(i + 1) % len(pts)]) for i in range(len(pts)))


def summarize(npc):
    evs = npc["events"]
    poss = [p for e in evs for p in positions_of(e) if is_vec(p)]
    home = npc.get("spawn_pos") or (poss[0] if poss else None)
    s = {"home": home, "records": dict(Counter(e["record"] for e in evs))}
    moves = [e for e in evs if e["record"] == "MoveTask"]
    for e in moves:
        e["_phase"] = phase(e)
    phases = Counter(e["_phase"] for e in moves)
    s["phases"] = dict(phases)
    s["range_m"] = round(max((hdist(home, p) for p in poss), default=0.0), 2) if home else None
    calm = [p for e in moves if e["_phase"] in ("patrol", "move") for p in positions_of(e)]
    if calm and home:
        s["range_m_not_chasing"] = round(max(hdist(home, p) for p in calm), 2)

    # patrol: looping splines; speed from the spline position of consecutive resends
    routes = {}
    prev = None
    for e in moves:
        cp = current_path(e["task"])
        if e["_phase"] != "patrol":
            prev = None
            continue
        loop = next(p for p in paths_in(e["task"]) if p.get("spline", {}).get("loop"))
        k = route_key(loop["points"])
        r = routes.setdefault(k, {"points": loop["points"], "length_m": round(closed_length(loop["points"]), 2),
                                  "records": 0, "first_t": e["t"], "last_t": e["t"], "_speeds": []})
        if e["task"]["task"] == "List" and e["task"]["children"][0] is not loop:
            r["approach"] = e["task"]["children"][0]["points"]
        r["records"] += 1
        r["last_t"] = e["t"]
        if cp is loop:
            if prev and prev[0] == k:
                dt = e["t"] - prev[1]
                if 1.0 <= dt <= 4.0:
                    r["_speeds"].append(math.dist(prev[2], e["start"]) / dt)
            prev = (k, e["t"], e["start"])
        else:
            prev = None
    if routes:
        out = []
        for r in sorted(routes.values(), key=lambda r: -r["records"]):
            sp = r.pop("_speeds")
            if sp:
                r["speed_lower_bound_mps"] = round(statistics.median(sp), 2)
                r["speed_lower_bound_max_mps"] = round(max(sp), 2)
                r["speed_samples"] = len(sp)
            r["max_from_home_m"] = round(max(hdist(home, p) for p in r["points"]), 2)
            out.append(r)
        s["patrol_routes"] = out

    # a non-looping path followed by the NPC's next MoveTask/Stop at its end point: arrived
    later = [e for e in evs if e["record"] in ("MoveTask", "Stop")]
    arr = []
    for i, e in enumerate(later[:-1]):
        if e["record"] != "MoveTask" or e["_phase"] != "move":
            continue
        cp = current_path(e["task"])
        pts = [e["start"]] + cp["points"]
        length = sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))
        nx = later[i + 1]
        at = nx["start"] if nx["record"] == "MoveTask" else nx["pos"]
        dt = nx["t"] - e["t"]
        if length >= 2.0 and dt >= 0.5 and math.dist(at, pts[-1]) < 0.5:
            arr.append(length / dt)
    if arr:
        s["move_speed_by_arrival_mps"] = round(statistics.median(arr), 2)
        s["move_speed_by_arrival_samples"] = len(arr)
    dests = [current_path(e["task"])["points"][-1] for e in moves if e["_phase"] == "move"]
    if dests:
        s["move_destinations"] = len({key05(d) for d in dests})
        s["went_home"] = any(hdist(d, home) < 1.5 for d in dests)
    maxspeeds = sorted({p["maxspeed"] for e in moves for p in paths_in(e["task"])
                        if isinstance(p.get("maxspeed"), float)})
    if maxspeeds:
        s["maxspeed_values"] = maxspeeds
    chased = set()
    for e in evs:
        if e["record"] == "MoveTask":
            tl = list(targets_in(e["task"]))
        elif e["record"] == "LookAt":
            tl = [("LookAt", e["target"])]
        else:
            tl = []
        for kind, tgt in tl:
            if tgt[1] >= PLAYER_MIN:
                chased.add((kind, tgt[1]))
    if chased:
        s["targeted_by"] = sorted({k for k, _ in chased})
        s["targeted_players"] = sorted({i for _, i in chased})
    for e in moves:
        del e["_phase"]
    fights = phases["chase"] or phases["track_npc"]
    if routes:
        s["behaviour"] = "patrol"
    elif fights and (s["range_m"] or 0) > 1.0:
        s["behaviour"] = "path + chase" if phases["move"] else "chase only"
    elif (s["range_m"] or 0) > 1.0:
        s["behaviour"] = "path only"
    else:
        s["behaviour"] = "stationary"
    return s


# ------------------------------------------------------------------ main

def run_livecap(capture, jsonl):
    subprocess.run([sys.executable, str(LIVECAP), capture, "--jsonl", jsonl], check=True,
                   stdout=subprocess.DEVNULL)


def npc_entry(visit, inst, spawned):
    o = spawned.get((visit["key"], inst))
    npc = {"instance": inst}
    if o:
        core = o.get("core") or {}
        st = core.get("stats", {})
        npc.update(name=o.get("name"), monster_data=st.get("monster_data"), level=st.get("level"),
                   template_id=(core.get("npc_stats") or {}).get("template_id"),
                   spawn_pos=o.get("pos"), spawn_t=o.get("t"),
                   walk_speed=st.get("movement_walk_speed"), run_speed=st.get("movement_run_speed"),
                   charge_speed=st.get("movement_charge_speed"))
    else:
        npc.update(name=None, note="no spawn (UpdateMessage) for it in this zone visit")
    npc["events"] = []
    return npc


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("capture", nargs="?")
    ap.add_argument("--jsonl", help="existing livecap --jsonl dump (skips decoding the capture)")
    ap.add_argument("--objects", default=str(DEFAULT_OBJECTS))
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    a = ap.parse_args()

    if a.jsonl and os.path.exists(a.jsonl):
        jsonl = a.jsonl
    else:
        if not a.capture:
            ap.error("give a capture or an existing --jsonl")
        jsonl = a.jsonl or os.path.join(tempfile.mkdtemp(), "rpc.jsonl")
        run_livecap(a.capture, jsonl)

    objs = json.load(open(a.objects))
    visit_at = {(v["flow"], v["rpc"]): v for v in objs["playfields"]}
    spawned = {}
    for o in objs["objects"]:
        if o["kind"] == "character" and o["instance"] < PLAYER_MIN:
            spawned.setdefault((o["playfield_key"], o["instance"]), o)

    cur = {}  # flow -> current zone visit
    counts = defaultdict(Counter)
    task_counts = defaultdict(Counter)
    child_counts = Counter()
    bcc_actions = Counter()
    zones = {}
    pb = 0
    with open(jsonl) as fh:
        for line in fh:
            row = json.loads(line)
            if row.get("dir") != "S->C" or not row.get("args_hex") or not row["method"].startswith("PassBlob"):
                continue
            pb += 1
            v = visit_at.get((row["flow"], pb))
            if v:
                cur[row["flow"]] = v
            b = bytes.fromhex(row["args_hex"])
            if len(b) < 17 or struct.unpack_from(">I", b, 4)[0] not in READERS:
                continue
            t = (row.get("ts_raw") or 0) / 1e7
            visit = cur.get(row["flow"])
            p = 4
            for typ, ident, d, exact in parse_blob(b):
                name = RECORD_NAMES[typ]
                c = counts[name]
                c["total"] += 1
                c["npc" if ident[0] == CHARACTER and ident[1] < PLAYER_MIN else "other"] += 1
                c["exact" if exact else "body_not_decoded" if exact is None else "failed"] += 1
                if typ == T_MOVE_TASK:
                    k = d["task"]["task"] if d else "?"
                    if d is None and len(b) > 30:
                        k = TASK_NAMES.get(b[30], f"type {b[30]}")  # type byte of the first record
                    task_counts[k]["total"] += 1
                    task_counts[k]["exact" if exact else "failed"] += 1
                    if d:
                        for ch in d["task"].get("children", []):
                            child_counts[ch["task"]] += 1
                if typ == T_BCC:
                    k = str(d["action"]) if d else "?"
                    if d and d.get("command_type"):
                        k += "/" + (COMMAND_TYPES.get(d["command_type"]) or str(d["command_type"]))
                    bcc_actions[k] += 1
                if d is None or ident[0] != CHARACTER or ident[1] >= PLAYER_MIN or visit is None:
                    continue
                zk = (visit["flow"], visit["rpc"])
                z = zones.setdefault(zk, {"playfield": visit["playfield"], "name": visit.get("name"),
                                          "playfield_key": visit["key"], "flow": visit["flow"],
                                          "visit_rpc": visit["rpc"], "npcs": {}})
                npc = z["npcs"].get(ident[1])
                if npc is None:
                    npc = z["npcs"][ident[1]] = npc_entry(visit, ident[1], spawned)
                rec = {T_MOVE_TASK: "MoveTask", T_CHAR_DC_MOVE: "CharDCMove", T_STOP: "Stop",
                       T_LOOK_AT: "LookAt", T_BCC: "BCC"}[typ]
                npc["events"].append({"t": round(t, 3), "rpc": pb, "record": rec, **d})

    agg = defaultdict(list)
    out_zones = []
    for zk in sorted(zones, key=lambda k: k[1]):
        z = zones[zk]
        npcs = []
        for inst in sorted(z["npcs"]):
            n = z["npcs"][inst]
            n["summary"] = summarize(n)
            npcs.append(n)
            agg[(n.get("name"), n.get("monster_data"))].append((z, n))
        z["npcs"] = npcs
        out_zones.append(z)

    kinds = []
    for (name, md), lst in sorted(agg.items(), key=lambda kv: (str(kv[0][0]), kv[0][1] or 0)):
        ss = [n["summary"] for _, n in lst]

        def med(vals):
            return round(statistics.median(vals), 2) if vals else None
        routes = [r for s in ss for r in s.get("patrol_routes", [])]
        calm = [s["range_m_not_chasing"] for s in ss if "range_m_not_chasing" in s]
        kinds.append({
            "name": name, "monster_data": md, "npcs": len(lst),
            "playfields": sorted({z["playfield"] for z, _ in lst}),
            "behaviour": dict(Counter(s["behaviour"] for s in ss)),
            "patrol_speed_lower_bound_mps": med([r["speed_lower_bound_mps"] for r in routes
                                                 if "speed_lower_bound_mps" in r]),
            "patrol_route_length_m": med([r["length_m"] for r in routes]),
            "patrol_route_points": sorted({len(r["points"]) for r in routes}) or None,
            "patrol_max_from_home_m": max((r["max_from_home_m"] for r in routes), default=None),
            "move_speed_by_arrival_mps": med([s["move_speed_by_arrival_mps"] for s in ss
                                              if "move_speed_by_arrival_mps" in s]),
            "range_m_not_chasing_median": med(calm),
            "range_m_not_chasing_max": max(calm, default=None),
            "range_m_max": max((s["range_m"] for s in ss if s.get("range_m") is not None), default=None),
            "run_speed_stat": sorted({n["run_speed"] for _, n in lst if n.get("run_speed") is not None}),
            "walk_speed_stat": sorted({n["walk_speed"] for _, n in lst if n.get("walk_speed") is not None}),
            "charge_speed_stat": sorted({n["charge_speed"] for _, n in lst if n.get("charge_speed") is not None}),
        })

    coverage = {name: dict(c) for name, c in sorted(counts.items())}
    for c in coverage.values():
        c["exact_share"] = round(c.get("exact", 0) / c["total"], 4)
    out = {
        "source": {"jsonl": os.path.basename(jsonl), "objects": "playfield-objects.json",
                   "tool": "tools/re/live_npc_movement.py"},
        "notes": __doc__.split("Output (npc-movement.json):")[1].strip(),
        "coverage": {
            "records": coverage,
            "move_task_top_level": {k: dict(v) for k, v in sorted(task_counts.items())},
            "list_children": dict(child_counts),
            "bcc_action_command": dict(sorted(bcc_actions.items())),
            "note": "exact = the parse ended at the blob end or where another record read here "
                    "starts; body_not_decoded = BCC command records (command subclass not "
                    "decoded); counts cover all server records (npc = NPC characters)."},
        "kinds": kinds,
        "zones": out_zones,
    }
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    # one line per event, the rest indented
    compact = {}
    for z in out_zones:
        for n in z["npcs"]:
            marks = []
            for ev in n["events"]:
                m = f"@@ev{len(compact)}@@"
                compact[f'"{m}"'] = json.dumps(ev, separators=(",", ":"))
                marks.append(m)
            n["events"] = marks
    text = json.dumps(out, indent=1)
    text = re.sub(r'"@@ev\d+@@"', lambda mo: compact[mo.group(0)], text)
    with open(a.out, "w") as fh:
        fh.write(text + "\n")
    print(json.dumps(out["coverage"], indent=1))
    print(f"wrote {a.out}: {sum(len(z['npcs']) for z in out_zones)} NPCs in {len(out_zones)} zone visits")


if __name__ == "__main__":
    main()
