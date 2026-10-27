#!/usr/bin/env python3
"""Decode the client->server half of an LMO live capture into structured data.

Reads a Microsoft Network Monitor capture through tools/re/livecap.py (TCP reassembly, LMO
framing, RPC header) and writes three files under data/live-capture/:

  client-actions.json     non-movement client RPCs (teleports, cinematics, answers, loot, ...),
                          the playfield per event, and counts per method
  player-movement.json    position samples from CharDCMoveIIR_t and MoveTaskIIR_c, grouped by
                          the playfield the player was on (or "unknown")
  client-actions-summary.md is written by hand from this output.

Usage:
  python3 tools/re/live_client_actions.py CAPTURE [--out-dir DIR] [--print-timeline]

Output notes: positions are the three float32 values as sent, in order x, y, z; which axis is
height is unverified. ID32 values are written "type:instance" (decimal).

Only client->server data is used. Server->client traffic is mostly lost in the capture (zlib
break), so playfield changes come from the client's own requests plus the ctrack analytics GETs
(`/track?...action=enter_splashscreen|exit_splashscreen&value=51100:<playfield>`) that the client
sends to 37.18.192.73:80. Those analytics lines are plain HTTP and carry the playfield the
client loaded, which is the confirmation the state machine waits for.

Envelope checks: each SignalPassBlob argument is a BinaryStream (u32 big-endian length +
bytes). The length must equal the rest of the argument (checked for every message). The IIR
record inside is u32 type id, ID32 identity, bool isToBePassedOn, then the class body. Bodies
are only parsed for classes with a layout verified here; a body is accepted only when the
parse consumes exactly the record (checked per message, counted in the output).

Not decoded: server replies, IIR bodies of unlisted classes (counted only), and the content of
the login challenge (AnswerChallenge is recorded as length only, never written out).
"""
import argparse
import collections
import json
import os
import re
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import livecap  # noqa: E402

REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
DEFAULT_OUT = os.path.join(REPO, "data", "live-capture")
TICKS = 1e7
PLAYFIELD_TYPE = 51100          # ID32 type of playfields (docs/PROTOCOL.md)
CTRACK_HOST = "37.18.192.73"    # analytics funnel, port 80

# IIR record type ids (Java string hash of the class name, data/iir_types.json)
IIR_NAMES = {}


def iir_type(name):
    h = 0
    for c in name.encode():
        h = (h * 31 + c) & 0xFFFFFFFF
    return h


IIR_KNOWN = ["CharInPlayIIR_t", "MoveTaskIIR_c", "CharDCMoveIIR_t", "CoreProjectileIIR",
             "BehaviorControlCenterIIR_c", "LookAtIIR_t", "KnubotAnswerIIR", "QuestCooldownIIR",
             "CoreClaimIIR", "QuestRewardSelectionIIR_c", "CharacterActionIIR_t"]
for _n in IIR_KNOWN:
    IIR_NAMES[iir_type(_n)] = _n
# Cross-check against data/iir_types.json when present.
try:
    with open(os.path.join(REPO, "data", "iir_types.json")) as _fh:
        _table = {int(e["type_id"], 16): e["name"] for e in json.load(_fh)}
    for _t, _n in list(IIR_NAMES.items()):
        assert _table.get(_t) == _n, (_n, _table.get(_t))
except FileNotFoundError:
    pass


class Unparsed(Exception):
    pass


class Reader:
    """Big-endian BinaryStream reader (docs/PROTOCOL.md, Argument encoding)."""

    def __init__(self, data):
        self.b = data
        self.p = 0

    def take(self, n):
        if self.p + n > len(self.b):
            raise Unparsed("short read at %d (need %d)" % (self.p, n))
        v = self.b[self.p:self.p + n]
        self.p += n
        return v

    def u8(self):
        return self.take(1)[0]

    def u16(self):
        return struct.unpack(">H", self.take(2))[0]

    def u32(self):
        return struct.unpack(">I", self.take(4))[0]

    def i32(self):
        return struct.unpack(">i", self.take(4))[0]

    def u64(self):
        return struct.unpack(">Q", self.take(8))[0]

    def f32(self):
        return struct.unpack(">f", self.take(4))[0]

    def boolean(self):
        return bool(self.u8())

    def vec3(self):
        return [round(self.f32(), 3) for _ in range(3)]

    def string(self):
        n = self.u16()
        if n >= 0x8000:
            # long form: ffff 01ff + u32 (not seen in this capture; kept for completeness)
            raise Unparsed("long string")
        return self.take(n).decode("latin-1")

    def id32(self):
        return (self.u32(), self.u32())

    def binary_stream(self):
        n = self.u32()
        return self.take(n)

    def done(self):
        return self.p == len(self.b)

    def rest(self):
        return self.b[self.p:]


def id32_str(t, i):
    return "%d:%d" % (t, i)


# ---------------------------------------------------------------- protobuf (PlayfieldProxyData)

def parse_playfield_proxy(blob):
    """PlayfieldProxy argument: u32 length + PlayfieldProxyData (proto/PlayfieldProxyData.proto).
    Returns (playfield instance, decoded fields) where the playfield is model_id.instance."""
    r = Reader(blob)
    body = r.binary_stream()
    if not r.done():
        raise Unparsed("trailing bytes after PlayfieldProxy")
    fields = {}
    model = None
    for fno, wt, v in livecap._pb_fields(body):
        if fno == 2 and wt == 2:
            mt = {}
            for f2, w2, v2 in livecap._pb_fields(v):
                mt[f2] = v2
            model = (mt.get(1), mt.get(2))
            fields["model_id"] = {"type": model[0], "instance": model[1]}
        elif wt == 0:
            fields[str(fno)] = v
        else:
            fields[str(fno)] = v.hex()
    if model is None or model[0] != PLAYFIELD_TYPE:
        raise Unparsed("PlayfieldProxy without a playfield model_id")
    return model[1], fields


# ---------------------------------------------------------------- client RPC decoders

def dec_none(r):
    return {}


def _ping(r):
    return {"sequence": r.u32()}


def _ss(r):
    return {"x": 0}


DECODERS = {
    "Ping": lambda r: {"sequence": r.u32()},
    "UpdateSpellTargetPosition": lambda r: {"commandID": r.u32(), "position": r.vec3()},
    "AddShortcut": lambda r: {"type": r.i32(), "position": r.i32(), "data": r.u32(),
                              "inventory_type": r.u32()},
    "RemoveShortcut": lambda r: {"position": r.i32()},
    "ClientRequestGuildInfo": lambda r: {"character": id32_str(*r.id32())},
    "RequestItemShopData": lambda r: {"language": r.string(), "platform": r.string(),
                                      "bundle_id": r.string()},
    "CinematicTransition": lambda r: {"client_id": id32_str(*r.id32()), "entering": r.boolean()},
    "ScryMessageBoxAnswer": lambda r: {"answer": r.i32(), "ctx_id": r.i32(),
                                       "cookie": r.u64()},
    "RequestItemShopTransaction": lambda r: {"request_id": r.u32(), "template_id": r.u32(),
                                             "shop_id": r.u32(), "cost_type": r.u32(),
                                             "amount": r.u32()},
    "SetPlayerGUILayoutMode": lambda r: {"layout_mode": r.u32()},
    "SetHideInspectOption": lambda r: {"enabled": r.boolean()},
    "TutorialShown": lambda r: {"tag_id": r.u32()},
    "TeleportMe": lambda r: {"playfield_id": r.u32(), "spawn_point_id": r.u32()},
    "TeleportWindowBypass": lambda r: {"window_id": r.u32(), "playfield_id": r.u32()},
    "TeleportMessageBoxAnswer": lambda r: _teleport_answer(r),
    "EndlessPlayfieldsTeleportMessageBoxAnswer": lambda r: {"answer": r.i32(), "ctx_id": r.i32()},
    "TryLootItem": lambda r: {"bag": id32_str(*r.id32()), "item_pos": r.u32(),
                              "char": id32_str(*r.id32()), "desired_pos": r.u32()},
    "Authenticate2": lambda r: _auth2(r),
    "Authenticate": lambda r: _auth(r),
    "InitiateAuthentication": lambda r: {"username_len": len(r.string()), "game_id": r.u32(),
                                         "redacted": True},
    "AnswerChallenge": lambda r: {"redacted": True, "bytes": len(r.rest())},
    "GetStartupData": lambda r: {"md5": r.string()},
    "FinishGetStartupData": lambda r: _checksum_map(r),
    "LoginCharacter": lambda r: _login_character(r),
    "CreateCharacterWithData": lambda r: _create_character(r),
    "SetCharacterCreationData": lambda r: _creation_data(r),
    "ClientRequestDestinationPlayfieldStatus": lambda r: _destination_status(r),
    "RequestResurrectionSpots": lambda r: {"playfield_id": r.u32()},
    "CinematicTransition2": None,
}


def _auth2(r):
    inst = r.u32()
    r.u32()  # cookie: session secret, not written out
    return {"client_instance": inst, "cookie": "redacted", "client_version": r.string()}


def _auth(r):
    acct = r.u32()
    r.u32()  # cookie: session secret, not written out
    return {"account_instance": acct, "cookie": "redacted", "territory_version": r.u32()}


def _teleport_answer(r):
    # teleport.rs: the value is the destination playfield (high bit = enter existing), -1 cancel
    a = r.i32()
    return {"answer": a, "ctx_id": r.i32(),
            "playfield": (a & 0x7FFFFFFF) if (a > 0 or a < -1) else None,
            "cancel": a == -1}


def _checksum_map(r):
    # container u32 (n+1)*1009, then (ID32::Instance key, String md5) pairs
    count = r.u32() // 1009 - 1
    entries = []
    for _ in range(count):
        entries.append({"character_instance": r.u32(), "md5": r.string()})
    return {"entries": entries}


def _login_character(r):
    inst = r.u32()
    cnt = r.u32() // 1009 - 1
    if cnt != 0:
        raise Unparsed("non-empty StrStrMap in LoginCharacter")
    lang = r.string()
    region, age, gender = r.u16(), r.u16(), r.u16()
    igr = r.string()
    return {"character_instance": inst, "language": lang, "region_code": region,
            "age_code": age, "gender_code": gender, "igr_id": igr,
            "user_type": r.u32(), "service_type": r.u32()}


def _create_character(r):
    dim = r.u32()
    cnt = r.u32() // 1009 - 1
    if cnt != 0:
        raise Unparsed("non-empty StrStrMap in CreateCharacterWithData")
    return {"dimension": dim, "region_code": r.u16(), "age_code": r.u16(),
            "gender_code": r.u16(), "igr_id": r.string(), "user_type": r.u32(),
            "service_type": r.u32()}


def _creation_data(r):
    # CreatePlayerCharacter layout not verified beyond these fields: three strings + u32
    return {"strings": [r.string(), r.string(), r.string()], "u32": r.u32(),
            "layout": "unverified"}


def _destination_status(r):
    blob = r.rest()
    pf, fields = parse_playfield_proxy(blob)
    return {"playfield_id": pf, "playfield_proxy_fields": fields}


# GameCharAgent/Project RPCs without arguments are only counted.
NO_ARG = {"RequestGearBuildLoad", "GuildRequestMemberList", "ExitCinematic",
          "StartEndlessWarmup", "ConvertMinifigureParts", "ReceivedCharacterSettings",
          "RequestMyFriendKey"}


# ---------------------------------------------------------------- IIR bodies

def iir_body(name, r, identity):
    """Parse the body of a known IIR class. Raises Unparsed if the layout does not match."""
    if name == "CharInPlayIIR_t":
        return {}
    if name == "MoveTaskIIR_c":
        task = r.u8()
        if task != 0:
            raise Unparsed("MoveTask type %d not decoded" % task)
        start = r.vec3()
        flag = r.u8()
        pad = r.take(10)
        speed = r.f32()
        n = r.u32()
        if n > 1000:
            raise Unparsed("waypoint count")
        wps = [r.vec3() for _ in range(n)]
        # 16-byte tail: zeros in most records; otherwise an ID32 + f32 (e.g. a dynel of type
        # 51321), meaning unverified. The record length check still has to pass.
        tail = r.take(16)
        if any(pad) or flag != 0x0F:
            raise Unparsed("MoveTask padding")
        return {"task_type": 0, "start": start, "waypoints": wps, "speed_f32": speed,
                "flag": flag, "tail_hex_unverified": tail.hex() if any(tail) else None}
    if name == "CharDCMoveIIR_t":
        flags = r.u8()
        pos = r.vec3()
        rot = [round(r.f32(), 4), round(r.f32(), 4)]   # 8 bytes, layout unverified
        align = r.u8()
        vel = None
        second = None
        if not flags & 0x04:
            vel = r.vec3()
        if not flags & 0x08:
            second = (r.vec3(), r.u8())
        rspeed = r.f32()
        b1, b2 = r.u8(), r.u8()
        return {"flags": flags, "pos": pos, "rot_unverified": rot, "align": align,
                "velocity": vel, "second_vector": second, "rot_speed": rspeed,
                "tail": [b1, b2]}
    if name == "KnubotAnswerIIR":
        base = r.u16()
        npc = r.id32()
        answer = r.u32()
        return {"knubot_base": base, "npc": id32_str(*npc), "answer_index": answer}
    # other classes: keep raw body, no claim about its fields
    raise Unparsed("no layout")


def decode_pass_blob(args):
    """SignalPassBlob(BinaryStream). Returns (list of record dicts, envelope_ok)."""
    r = Reader(args)
    body = r.binary_stream()
    if not r.done():
        return None, False
    recs = []
    rr = Reader(body)
    t = rr.u32()
    ident = rr.id32()
    passed_on = rr.boolean()
    name = IIR_NAMES.get(t)
    rec = {"type_id": "0x%08x" % t, "name": name or "unknown",
           "identity": id32_str(*ident), "passed_on": passed_on}
    if name:
        try:
            body_r = Reader(rr.rest())
            rec["fields"] = iir_body(name, body_r, ident)
            rec["parsed"] = body_r.done()
            if not body_r.done():
                rec["parsed"] = False
        except Unparsed as e:
            rec["fields"] = None
            rec["parsed"] = False
            rec["note"] = str(e)
    else:
        rec["fields"] = None
        rec["parsed"] = False
    rec["record_bytes"] = len(body)
    rec["record_hex"] = body.hex()
    recs.append(rec)
    return recs, True


# ---------------------------------------------------------------- capture walk

def client_messages(capture):
    """All client->server RPCs, in capture order: dicts with frame, t, endpoint, method, args."""
    dirs, _stats = livecap.build_directions(capture)
    tables = livecap.load_method_names()
    msgs = []
    ctrack = []
    for key, d in dirs.items():
        if key[2] == CTRACK_HOST and key[3] == 80:
            for run in d.runs:
                for m in re.finditer(rb"GET (/track\?[^ \r\n]*)", run.data):
                    frame, ts = run.frame_at(m.start())
                    ctrack.append({"frame": frame, "ts": ts,
                                   "query": m.group(1).decode("latin-1")})
        direction, frames, _info = livecap.flow_messages(d)
        if direction != "C->S":
            continue
        for _off, ctl, body, _run, frame_no, ts in frames:
            if ctl:
                continue
            try:
                hdr, args = livecap.parse_rpc_header(body)
            except Exception:
                continue
            ep = livecap.endpoint_name(hdr["endpoint"])
            meth = hdr["method"]
            meth_i = int.from_bytes(meth, "big") if isinstance(meth, (bytes, bytearray)) else meth
            sig = livecap.method_signature(tables, "C->S", ep, meth_i)
            name = sig.split("(")[0] if sig else "0x%08x" % (meth_i or 0)
            msgs.append({"frame": frame_no, "ts": ts or 0, "flow": "%s:%d" % key[2:4],
                         "endpoint": ep, "method": name, "signature": sig or "",
                         "args": args})
    msgs.sort(key=lambda m: (m["ts"], m["frame"] or 0))
    ctrack.sort(key=lambda c: (c["ts"], c["frame"] or 0))
    return msgs, ctrack


def ctrack_playfield_events(ctrack):
    out = []
    for c in ctrack:
        m = re.search(r"action=(\w+)&value=51100:(-?\d+)", c["query"])
        if m:
            out.append({"frame": c["frame"], "ts": c["ts"], "action": m.group(1),
                        "playfield": int(m.group(2))})
    return out


def build_timeline(msgs, ctrack):
    """Merge client RPC playfield changes and ctrack confirmations, in time order."""
    ev = []
    for m in msgs:
        if m["method"] in ("TeleportMe", "TeleportWindowBypass"):
            ev.append((m["ts"], m["frame"], "request", m["method"], m))
        elif m["method"] in ("TeleportMessageBoxAnswer", "EndlessPlayfieldsTeleportMessageBoxAnswer"):
            ev.append((m["ts"], m["frame"], "answer", m["method"], m))
    for c in ctrack_playfield_events(ctrack):
        ev.append((c["ts"], c["frame"], "ctrack", c["action"], c))
    ev.sort(key=lambda x: (x[0], x[1] or 0))
    return ev


# ---------------------------------------------------------------- main

def run(capture, out_dir, print_timeline=False):
    msgs, ctrack = client_messages(capture)
    ctrack_pf = ctrack_playfield_events(ctrack)

    # Playfield state. Sources, in time order:
    #   TeleportMe(p), TeleportWindowBypass(w, p): request p (pending).
    #   TeleportMessageBoxAnswer(a): a > 0 is the playfield (high bit = enter existing), -1 cancel
    #     (teleport.rs: the server teleports on the same value). Request.
    #   EndlessPlayfieldsTeleportMessageBoxAnswer(a): request of a server-chosen instance,
    #     destination unknown (pending None-but-flagged).
    #   ctrack enter_splashscreen X: loading -> "unknown".
    #   ctrack exit_splashscreen X: arrived at X, confirmed.
    #   ctrack enter_game inplay: a playfield was entered. With a pending request, it becomes that
    #     playfield, unconfirmed (no splash line named it). Without one, "unknown".
    # Samples and events carry the state at their own time.
    tl = []
    for m in msgs:
        if m["method"] in ("TeleportMe", "TeleportWindowBypass", "TeleportMessageBoxAnswer",
                           "EndlessPlayfieldsTeleportMessageBoxAnswer"):
            tl.append((m["ts"], m["frame"] or 0, 1, "rpc", m))
    for c in ctrack:
        mm = re.search(r"action=(\w+)&value=([^&\s]+)", c["query"])
        if mm and mm.group(1) in ("exit_splashscreen", "enter_splashscreen", "enter_game"):
            tl.append((c["ts"], c["frame"] or 0, 0, "ctrack", {"action": mm.group(1),
                                                              "value": mm.group(2)}))
    tl.sort(key=lambda x: (x[0], x[1], x[2]))

    st = {"pf": "unknown", "confirmed": False, "pending": None, "src": None}
    changes = []
    visited = []
    confirmed = set()

    def note(pf):
        if isinstance(pf, int) and pf not in visited:
            visited.append(pf)

    def apply(entry):
        ts, frame, _k, kind, obj = entry
        t = round(ts / TICKS, 3)
        if kind == "rpc":
            name = obj["method"]
            args = decode_client(obj) or {}
            if name in ("TeleportMe", "TeleportWindowBypass"):
                st["pending"] = args.get("playfield_id")
            elif name == "TeleportMessageBoxAnswer":
                a = args.get("answer")
                if a is not None and a != -1:
                    st["pending"] = a & 0x7FFFFFFF
            elif name == "EndlessPlayfieldsTeleportMessageBoxAnswer":
                if args.get("answer") == 1:
                    st["pending"] = "endless-unknown"
            to = st["pending"]
            if name == "TeleportMessageBoxAnswer" and args.get("cancel"):
                to = "cancel"
            elif name == "EndlessPlayfieldsTeleportMessageBoxAnswer":
                to = "endless-unknown" if args.get("answer") == 1 else "declined"
            changes.append({"t": t, "frame": frame, "event": "request", "via": name, "to": to})
        else:
            a = obj["action"]
            v = obj["value"]
            if a == "enter_splashscreen":
                st.update(pf="unknown", confirmed=False, src="loading " + v)
                changes.append({"t": t, "frame": frame, "event": "loading",
                                "to": int(v.split(":")[-1])})
            elif a == "exit_splashscreen":
                pf = int(v.split(":")[-1]) if ":" in v else None
                if pf is not None and pf >= 0:
                    st.update(pf=pf, confirmed=True, pending=None, src="ctrack exit")
                    note(pf)
                    confirmed.add(pf)
                changes.append({"t": t, "frame": frame, "event": "arrived", "playfield": pf})
            elif a == "enter_game":
                if st["pending"] is not None and st["pending"] != "endless-unknown":
                    st.update(pf=st["pending"], confirmed=False, src="enter_game after request")
                    note(st["pf"])
                else:
                    st.update(pf="unknown", confirmed=False, src="enter_game, destination unknown")
                st["pending"] = None
                changes.append({"t": t, "frame": frame, "event": "enter_game",
                                "playfield": st["pf"]})

    samples = collections.defaultdict(list)
    events = []
    counts = collections.Counter()
    iir_counts = collections.Counter()
    unknown_types = collections.Counter()
    body_ok = collections.Counter()
    body_fail = collections.Counter()
    envelope_bad = 0
    knubot_answers = collections.Counter()
    loot_bags = set()
    tl_i = 0
    for m in msgs:
        while tl_i < len(tl) and (tl[tl_i][0], tl[tl_i][1]) <= (m["ts"], m["frame"] or 0):
            apply(tl[tl_i])
            tl_i += 1
        t = round(m["ts"] / TICKS, 3)
        pf = st["pf"]
        pf_out = pf if isinstance(pf, int) else "unknown"
        # a teleport request is pending until the splash or enter_game line: the player is
        # still in pf, but the destination is not yet confirmed
        pf_ok = st["confirmed"] and st["pending"] is None
        if m["method"] == "SignalPassBlob" and m["endpoint"] == "ClientToGameserverRPC":
            recs, ok = decode_pass_blob(m["args"])
            if not ok:
                envelope_bad += 1
                continue
            rec = recs[0]
            iir_counts[rec["name"]] += 1
            if rec["name"] == "unknown":
                unknown_types[rec["type_id"]] += 1
            if rec.get("parsed"):
                body_ok[rec["name"]] += 1
            elif rec["name"] != "unknown":
                body_fail[rec["name"]] += 1
            f = rec.get("fields") or {}
            if rec["name"] == "CharDCMoveIIR_t" and rec.get("parsed"):
                samples[pf_out].append({"t": t, "frame": m["frame"], "pos": f["pos"],
                                        "char": rec["identity"], "src": "CharDCMoveIIR_t",
                                        "pf_confirmed": pf_ok})
            elif rec["name"] == "MoveTaskIIR_c" and rec.get("parsed"):
                smp = {"t": t, "frame": m["frame"], "pos": f["start"], "char": rec["identity"],
                       "src": "MoveTaskIIR_c start", "pf_confirmed": pf_ok}
                if f["waypoints"]:
                    smp["waypoints"] = f["waypoints"]
                samples[pf_out].append(smp)
            elif rec["name"] == "KnubotAnswerIIR" and rec.get("parsed"):
                knubot_answers[f["answer_index"]] += 1
                events.append(event(t, m, pf_out, "KnubotAnswerIIR (client answer)", f,
                                    rec["identity"]))
            elif rec["name"] in ("CharacterActionIIR_t", "QuestRewardSelectionIIR_c",
                                 "CoreClaimIIR", "QuestCooldownIIR"):
                events.append(event(t, m, pf_out, rec["name"],
                                    {"parsed": False, "record_hex": rec["record_hex"][:200]},
                                    rec["identity"]))
            continue
        if m["method"] == "Ping":
            counts["Ping"] += 1
            continue
        counts[m["method"]] += 1
        if m["method"] in NO_ARG:
            events.append(event(t, m, pf_out, m["method"], {}, None))
            continue
        args = decode_client(m)
        if args is None:
            events.append(event(t, m, pf_out, m["method"],
                                {"raw_hex": m["args"].hex(), "decode": "unverified"}, None))
            continue
        if m["method"] == "TryLootItem":
            loot_bags.add(args["bag"])
        if m["method"] in EVENT_METHODS:
            events.append(event(t, m, pf_out, m["method"], args, None))

    while tl_i < len(tl):
        apply(tl[tl_i])
        tl_i += 1
    note(st["pf"])

    os.makedirs(out_dir, exist_ok=True)
    source = os.path.basename(capture)
    total_samples = sum(len(v) for v in samples.values())
    counts_out = dict(counts)
    counts_out["_SignalPassBlob_by_IIR_class"] = dict(iir_counts.most_common())
    counts_out["_IIR_bodies_parsed_and_verified"] = dict(body_ok)
    counts_out["_IIR_bodies_not_parsed"] = dict(body_fail)
    counts_out["_IIR_unknown_type_ids"] = dict(unknown_types.most_common())
    counts_out["_SignalPassBlob_envelope_mismatch"] = envelope_bad
    counts_out["_KnubotAnswerIIR_answer_index"] = dict(knubot_answers)
    counts_out["_TryLootItem_distinct_bags"] = len(loot_bags)
    counts_out["_ctrack_playfield_lines"] = len(ctrack_pf)
    counts_out["_playfield_state_changes"] = len(changes)

    act = {
        "source": source,
        "playfields_visited": visited,
        "playfields_confirmed_by_ctrack_exit": sorted(confirmed),
        "playfield_names": playfield_names(visited),
        "playfield_state_changes": changes,
        "events": events,
        "counts": counts_out,
        "notes": ("playfield per event = the state at that time. playfields_visited = every "
                  "playfield the state reached (ctrack exit or enter_game after a request). Sources: the client's teleport "
                  "RPCs (TeleportMe, TeleportWindowBypass, TeleportMessageBoxAnswer > 0) and "
                  "ctrack analytics (enter/exit_splashscreen 51100:<id>, enter_game). 'unknown' "
                  "while loading, after an enter_game with no matching request, and before "
                  "the first arrival. Server replies are not in the capture; Endless answers "
                  "name no destination."),
    }
    with open(os.path.join(out_dir, "client-actions.json"), "w") as fh:
        json.dump(act, fh, indent=1)
        fh.write("\n")
    mov = {
        "source": source,
        "samples_by_playfield": samples,
        "notes": ("Every CharDCMoveIIR_t (pos) and MoveTaskIIR_c (start; waypoints where "
                  "present), unthinned: %d samples. Positions are float32 big-endian x, y, z. "
                  "pf_confirmed false = playfield taken from enter_game after a request, no "
                  "splash line. 'unknown' = loading or no known playfield. The rotation of "
                  "CharDCMoveIIR_t is not included (layout unverified)." % total_samples),
    }
    with open(os.path.join(out_dir, "player-movement.json"), "w") as fh:
        json.dump(mov, fh)
        fh.write("\n")
    return act, mov


EVENT_METHODS = {
    "TeleportMe", "TeleportWindowBypass", "TeleportMessageBoxAnswer",
    "EndlessPlayfieldsTeleportMessageBoxAnswer", "TryLootItem", "CinematicTransition",
    "ScryMessageBoxAnswer", "TutorialShown", "StartEndlessWarmup", "ExitCinematic",
    "UpdateSpellTargetPosition", "AddShortcut", "RemoveShortcut", "CreateCharacterWithData",
    "SetCharacterCreationData", "LoginCharacter", "ClientRequestDestinationPlayfieldStatus",
    "RequestResurrectionSpots", "RequestItemShopTransaction", "RequestItemShopData",
    "ClientRequestGuildInfo", "SetPlayerGUILayoutMode", "SetHideInspectOption",
    "GetStartupData", "FinishGetStartupData", "Authenticate2", "Authenticate",
    "InitiateAuthentication", "AnswerChallenge", "ConvertMinifigureParts",
    "RequestMyFriendKey",
}


def playfield_names(ids):
    """Names from data/playfields.json (key = instance), for the ids seen."""
    try:
        with open(os.path.join(REPO, "data", "playfields.json")) as fh:
            table = json.load(fh).get("playfields", {})
    except (OSError, ValueError):
        return {}
    return {str(i): table[str(i)] for i in ids if str(i) in table}


def decode_client(m):
    name = m["method"]
    dec = DECODERS.get(name)
    if dec is None:
        return None
    r = Reader(m["args"])
    try:
        out = dec(r)
    except Unparsed:
        return None
    if not r.done() and name not in ("AnswerChallenge", "ClientRequestDestinationPlayfieldStatus"):
        return None
    return out


def event(t, m, pf, name, args, identity):
    e = {"t": round(t, 3), "frame": m["frame"], "method": name, "args": args, "playfield": pf}
    if identity:
        e["char"] = identity
    return e


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("capture")
    ap.add_argument("--out-dir", default=DEFAULT_OUT)
    ap.add_argument("--print-timeline", action="store_true")
    a = ap.parse_args()
    act, mov = run(a.capture, a.out_dir, a.print_timeline)
    print("playfields visited:", act["playfields_visited"])
    print("events:", len(act["events"]))
    print("movement samples:", {k: len(v) for k, v in mov["samples_by_playfield"].items()})


if __name__ == "__main__":
    main()
