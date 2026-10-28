#!/usr/bin/env python3
"""Decode the NPC dialogues (Knubot) of a live capture: what the server said, the answers it
offered and the answers the player picked. Writes data/live-capture/lmo-captures/dialogues.json.

Usage:
    python3 tools/re/live_dialogue.py <capture.cap> [--out FILE] [--jsonl rows.jsonl]

The capture is decoded with tools/re/livecap.py (`--jsonl`), unless --jsonl names an existing
dump (one RPC per line: flow, dir, ts_raw in microseconds, method,
args_hex). Texts are resolved with tools/re/tdc.py (the client's TDB), NPCs with
playfield-objects.json (tools/re/live_playfield.py).

Records (each Knubot record is the only record of its PassBlob in LMO_Captures.cap, and every
layout below consumes its record exactly; a record that does not is listed in "unparsed"):

* S->C GameserverToClientRPC::PassBlob(BinaryStream) and C->S
  ClientToGameserverRPC::SignalPassBlob(BinaryStream): u32 BE length, then IIR records: u32 BE
  type id (data/iir_types.json), ID32 identity (u32 type, u32 instance; the player character),
  u8 pass-on, class data (big-endian). Strings are u16 length + bytes (0xffff, u16, u32 length
  for long ones).
* The Knubot records' class data starts with u16 version (1) and ID32 npc (the dynel that
  speaks, "knubotOwnerActor"):
    - KnubotOpenChatWindowIIR (pass-on 0): u8 0.
    - KnubotAppendTextIIR (pass-on 0): u32 NPCChatTextType (1), string XML (a <remoteformat> of
      category 50002 with `antitag` = how many of the text's {tags} the server has run), u8 1,
      VoiceParameters (u32, i32, f32, f32: 0, -1, 4, 20 in every record).
    - KnubotAnswerListIIR (pass-on 0): u32 n, n x string XML (<remoteformat> of category 50001).
    - KnubotCloseChatWindowIIR (pass-on 0): u32 1, string XML (<remoteformat> 100/0).
    - KnubotAnswerIIR (C->S): u32 answer index into the last answer list.
* KnubotActorIIR (pass-on 1): u32 length + KnubotActorIIRProto::KnubotActorMessage
  (proto/KnubotActorIIR.proto), the cinematic steps of the {tags}.
* SpawnClientNPCIIR (pass-on 1): u32 length + SpawnClientNPCProto::SpawnClientNPCMessage
  (proto/SpawnClientNPC.proto), a client-side cinematic actor ({addcinematicactor}).
* CharInPlayIIR_t (C->S, no class data: the client is in the world after an arrival) and
  QuestGoalIIR (S->C, not decoded): only their time, for what started a dialogue.
* PlayfieldFIIR_t: 3 f32, u32 length + PlayfieldProxyData; its model_id {51100, playfield} gives
  the playfield of the flow (one zone server connection) from then on.

A dialogue is Open ... Close on one flow. The player's answers (C->S, on the client's flow) are
paired by time with the open dialogue of the same player and NPC; the picked answer is the
index into the answer list sent last before it. Actor and client NPC records between Open and
Close belong to the dialogue; the ones sent within 0.1 s before an Open (the "playerActor" /
"knubotOwnerActor" references) are its setup. References with no Open after them are listed in
"outside_dialogues". Answers with no dialogue open are
listed in "orphan_answers" (their server records were not captured).

Output: {source, dialogues: [{index, t, ts_raw, flow, player, playfield, playfield_name, npc:
{identity, name, monster_data, template_id, pos, name_id, name_text}, texts: [{id, text,
answer_id (the 50001 answer that led to it), antitags}], start (what preceded the Open:
"arrival", "after_dialogue" (+ follows: that dialogue's first text), "quest_goal" or "server"),
start_evidence, answer_lists: [{t, after_text,
answers: [{id, text}]}], picked: [{t, index, id, text}], close, actors: [...], steps: [...]}],
npcs: summary per playfield, orphan_answers, unparsed, stats}.
"""
import argparse
import html
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "data"
LIVECAP = ROOT / "tools" / "re" / "livecap.py"
OUT = DATA / "live-capture" / "lmo-captures" / "dialogues.json"
OBJECTS = DATA / "live-capture" / "lmo-captures" / "playfield-objects.json"

sys.path.insert(0, str(Path(__file__).resolve().parent))

IIR_NAMES = {int(r["type_id"], 16): r["name"] for r in json.loads((DATA / "iir_types.json").read_text())}
KNUBOT = {"KnubotOpenChatWindowIIR", "KnubotAppendTextIIR", "KnubotAnswerListIIR",
          "KnubotCloseChatWindowIIR", "KnubotAnswerIIR", "KnubotActorIIR", "SpawnClientNPCIIR"}
ACTOR_KIND = ["SetActiveActor", "MoveCameraToActor", "MoveCameraToStaticPoint", "StartEffectPackage",
              "StartAnimation", "RemoveActor", "AddCinematicActorReference", "ClearCinematicState",
              "MoveActorToPos", "MoveActorToActor", "ChangeGuiVisibility", "StopEffectPackage"]
# what may have started a dialogue: the player's arrival, a quest goal (not decoded further)
CONTEXT = {"PlayfieldFIIR_t", "CharInPlayIIR_t", "QuestGoalIIR"}
PLAYFIELD_TYPE = 51100
# capture timestamps (ts_raw) are microseconds (checked against the HTTP Date headers). The
# windows below were tuned when they were read as 100-ns ticks; they keep those raw values, so
# they are 10x the seconds first intended (and dialogues.json stays as it was)
TICKS = 1_000_000
# actor references this close before an Open are its setup
SETUP_TICKS = TICKS
AFTER_TICKS = TICKS * 10
ARRIVAL_TICKS = TICKS * 15
GOAL_TICKS = TICKS


class Short(Exception):
    pass


class Reader:
    """Big-endian reader of an IIR record."""

    def __init__(self, b, p=0):
        self.b, self.p = b, p

    def take(self, n):
        if self.p + n > len(self.b):
            raise Short(f"need {n} bytes at {self.p}, have {len(self.b) - self.p}")
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

    def f32(self):
        return struct.unpack(">f", self.take(4))[0]

    def id32(self):
        return [self.u32(), self.u32()]

    def string(self):
        n = self.u16()
        if n == 0xFFFF:
            self.u16()
            n = self.u32()
        return self.take(n).decode("utf-8", "replace")

    def done(self):
        return self.p == len(self.b)


# ---------------------------------------------------------------- protobuf

def varint(b, p):
    v = shift = 0
    while True:
        c = b[p]
        p += 1
        v |= (c & 0x7F) << shift
        shift += 7
        if not c & 0x80:
            return v, p


def proto_fields(b):
    """Generic protobuf decode: [(field, wire type, value)]; raises on a malformed message."""
    out, p = [], 0
    while p < len(b):
        key, p = varint(b, p)
        f, wt = key >> 3, key & 7
        if wt == 0:
            v, p = varint(b, p)
        elif wt == 1:
            v, p = b[p:p + 8], p + 8
        elif wt == 2:
            n, p = varint(b, p)
            v, p = b[p:p + n], p + n
        elif wt == 5:
            v, p = struct.unpack("<f", b[p:p + 4])[0], p + 4
        else:
            raise ValueError(f"wire type {wt}")
        if p > len(b):
            raise ValueError("truncated")
        out.append((f, wt, v))
    return out


def vec(b):
    d = {f: round(v, 4) for f, _, v in proto_fields(b)}
    return [d.get(i, 0.0) for i in range(1, 1 + max(3, len(d)))]


def actor_message(b):
    """KnubotActorMessage -> dict (field names of proto/KnubotActorIIR.proto)."""
    names = {1: "version", 2: "iir_type", 3: "actor_string_alias", 4: "effect_package",
             5: "ai_static_point", 6: "position", 7: "animation", 8: "actor_reference",
             9: "gui_visibility", 10: "instant_teleport", 11: "follow_actor"}
    m = {}
    for f, _, v in proto_fields(b):
        k = names.get(f, f"field{f}")
        if f == 2:
            v = ACTOR_KIND[v] if v < len(ACTOR_KIND) else v
        elif f in (3, 5):
            v = v.decode("utf-8", "replace")
        elif f == 6:
            v = vec(v)
        elif f == 8:
            d = {g: x for g, _, x in proto_fields(v)}
            v = [d.get(1, 0), d.get(2, 0)]
        elif f == 9:
            v = v - (1 << 64) if v >= 1 << 63 else v
        elif f in (10, 11):
            v = bool(v)
        m[k] = v
    return m


def spawn_client_npc(b):
    """SpawnClientNPCMessage -> dict (proto/SpawnClientNPC.proto)."""
    names = {1: "version", 2: "item_template", 3: "ai_point_name", 4: "position", 5: "rotation",
             6: "cinematic_actor_alias", 7: "is_cinematic"}
    m = {}
    for f, _, v in proto_fields(b):
        k = names.get(f, f"field{f}")
        if f in (3, 6):
            v = v.decode("utf-8", "replace")
        elif f in (4, 5):
            v = vec(v)
        elif f == 7:
            v = bool(v)
        m[k] = v
    return m


def playfield_of(b):
    """PlayfieldProxyData: the submessage {1: 51100, 2: playfield}."""
    for f, wt, v in proto_fields(b):
        if wt == 2:
            try:
                d = {g: x for g, w, x in proto_fields(v) if w == 0}
            except (ValueError, IndexError, struct.error):
                continue
            if d.get(1) == PLAYFIELD_TYPE and 2 in d:
                return d[2]
    return None


# ---------------------------------------------------------------- XML

def remoteformat(xml):
    """The top <remoteformat>'s attributes and its parameters (values unescaped once)."""
    m = re.match(r"<remoteformat ([^>]*)>", xml)
    attrs = dict(re.findall(r'(\w+)="([^"]*)"', m.group(1))) if m else {}
    params = {html.unescape(n): html.unescape(v) for v, n in
              re.findall(r'<parameter value="([^"]*)" name="([^"]*)" />', xml)}
    out = {k: html.unescape(v) for k, v in attrs.items()}
    for k in ("id", "category", "knubot", "antitag"):
        if k in out and out[k].lstrip("-").isdigit():
            out[k] = int(out[k])
    out["parameters"] = params
    return out


def npc_name_id(params):
    """The 51000 id of the `npcname` parameter (`7;<localized id="N" category="51000" ...>`)."""
    m = re.search(r'id="(\d+)" category="51000"', params.get("npcname", ""))
    return int(m.group(1)) if m else None


def keys_of(xml):
    """{"category/id" or "category/token": key} of the localized references in an XML."""
    keys = {}
    for level in (xml, html.unescape(xml)):
        for m in re.finditer(r'<(?:remoteformat|localized) (?:id="(\d+)"|token="([^"]+)") '
                             r'category="(\d+)"[^>]*? key="([^"]*)"', level):
            keys.setdefault(f"{m.group(3)}/{m.group(1) or m.group(2)}", html.unescape(m.group(4)))
    return keys


# ---------------------------------------------------------------- records

def decode_record(name, r):
    """Class data of a Knubot record. Raises Short/ValueError when the layout doesn't fit."""
    if name in ("KnubotActorIIR", "SpawnClientNPCIIR"):
        body = r.take(r.u32())
        return actor_message(body) if name == "KnubotActorIIR" else spawn_client_npc(body)
    f = {"version": r.u16(), "npc": r.id32()}
    if name == "KnubotOpenChatWindowIIR":
        f["flag"] = r.u8()
    elif name == "KnubotAppendTextIIR":
        f["chat_text_type"] = r.u32()
        f["xml"] = r.string()
        f["flag"] = r.u8()
        f["voice"] = [r.u32(), r.i32(), r.f32(), r.f32()]
    elif name == "KnubotAnswerListIIR":
        f["xml"] = [r.string() for _ in range(r.u32())]
    elif name == "KnubotCloseChatWindowIIR":
        f["close_type"] = r.u32()
        f["xml"] = r.string()
    elif name == "KnubotAnswerIIR":
        f["answer_index"] = r.u32()
    return f


def records(rows):
    """Knubot and PlayfieldFIIR records in time order: dicts with t, flow, dir, name, identity,
    pass_on, fields."""
    out, unparsed = [], []
    for row in rows:
        if "PassBlob" not in row.get("method", "") or not row.get("args_hex"):
            continue
        b = bytes.fromhex(row["args_hex"])
        if len(b) < 17:
            continue
        t = struct.unpack_from(">I", b, 4)[0]
        name = IIR_NAMES.get(t)
        if name not in KNUBOT and name not in CONTEXT:
            continue
        base = {"ts_raw": row["ts_raw"], "t": round(row["ts_raw"] / TICKS, 4), "flow": row["flow"],
                "dir": row["dir"], "name": name}
        r = Reader(b)
        end = 4 + r.u32()
        r.u32()
        ident, pass_on = r.id32(), r.u8()
        if name in ("CharInPlayIIR_t", "QuestGoalIIR"):
            out.append({**base, "identity": ident})
            continue
        if name == "PlayfieldFIIR_t":
            r.take(12)
            out.append({**base, "playfield": playfield_of(r.take(r.u32()))})
            continue
        try:
            fields = decode_record(name, r)
            ok = r.done() and end == len(b)
            err = None if ok else f"{len(b) - r.p} bytes left"
        except (Short, ValueError, IndexError, struct.error) as e:
            fields, ok, err = None, False, str(e)
        if not ok:
            unparsed.append({**base, "identity": ident, "note": err, "hex": b.hex()})
            continue
        out.append({**base, "identity": ident, "pass_on": pass_on, "fields": fields})
    out.sort(key=lambda x: x["ts_raw"])
    return out, unparsed


# ---------------------------------------------------------------- dialogues

def load_npcs(path):
    """(playfield, instance) and instance -> NPC info, from playfield-objects.json."""
    by_pf, by_inst, names = {}, {}, {}
    if not path.exists():
        return by_pf, by_inst, names
    d = json.loads(path.read_text())
    for v in d.get("playfields", []):
        names[v["playfield"]] = v["name"]
    for o in d.get("objects", []):
        if o.get("type") != 50000 or not o.get("core"):
            continue
        c = o["core"]
        info = {"name": c.get("name"), "monster_data": (c.get("stats") or {}).get("monster_data"),
                "template_id": (c.get("npc_stats") or {}).get("template_id"), "pos": o.get("pos"),
                "rot": o.get("rot"), "objects_playfield": o.get("playfield")}
        by_pf.setdefault((o["playfield"], o["instance"]), info)
        by_inst.setdefault(o["instance"], info)
    return by_pf, by_inst, names


def build(recs, npcs_pf, npcs_inst, pf_names, texts, answers_db, names_db):
    playfield_by_flow = {}
    dialogues, orphans, outside = [], [], []
    open_by_player = {}          # player instance -> dialogue
    pending = []                  # actor/spawn records not in a dialogue yet
    keys = {}
    arrivals, goals = {}, {}   # player instance -> ts_raw of the last CharInPlay / QuestGoalIIR
    for x in recs:
        if x["name"] == "PlayfieldFIIR_t":
            playfield_by_flow[x["flow"]] = x["playfield"]
            continue
        if x["name"] == "CharInPlayIIR_t":
            if x["dir"] == "C->S":
                arrivals[x["identity"][1]] = x["ts_raw"]
            continue
        if x["name"] == "QuestGoalIIR":
            goals[x["identity"][1]] = x["ts_raw"]
            continue
        player = x["identity"]
        f = x["fields"]
        d = open_by_player.get(player[1])
        step = {"t": x["t"], "record": x["name"]}
        if x["name"] in ("KnubotActorIIR", "SpawnClientNPCIIR"):
            step.update(f)
            if d is not None:
                d["steps"].append(step)
                (d["actors"] if x["name"] == "KnubotActorIIR" else d["client_npcs"]).append(step)
            else:
                pending.append((x, step))
            continue
        if x["name"] == "KnubotOpenChatWindowIIR":
            pf = playfield_by_flow.get(x["flow"])
            inst = f["npc"][1]
            npc = npcs_pf.get((pf, inst)) or npcs_inst.get(inst) or {}
            d = {"index": len(dialogues) + 1, "t": x["t"], "ts_raw": x["ts_raw"], "flow": x["flow"],
                 "player": player, "playfield": pf, "playfield_name": pf_names.get(pf),
                 "npc": {"identity": f["npc"], **{k: v for k, v in npc.items() if k != "objects_playfield"}},
                 "texts": [], "answer_lists": [], "picked": [], "close": None,
                 "setup": [], "actors": [], "client_npcs": [], "steps": [], "_offered": None}
            # the setup: actor references sent right before the Open (same tick, or within 0.1 s)
            for px, ps in pending:
                if 0 <= x["ts_raw"] - px["ts_raw"] <= SETUP_TICKS and px["identity"] == player:
                    d["setup"].append(ps)
                else:
                    outside.append(ps)
            pending = []
            d["steps"] += d["setup"] + [{**step, "flag": f["flag"]}]
            d["start"], d["follows"], d["start_evidence"] = start_of(x, d, dialogues, arrivals, goals)
            dialogues.append(d)
            open_by_player[player[1]] = d
            continue
        if d is None:
            if x["name"] == "KnubotAnswerIIR":
                orphans.append({"t": x["t"], "flow": x["flow"], "player": player, "npc": f["npc"],
                                "answer_index": f["answer_index"]})
            else:
                outside.append({**step, "npc": f["npc"]})
            continue
        if f["npc"] != d["npc"]["identity"]:
            step["other_npc"] = f["npc"]
        if x["name"] == "KnubotAppendTextIIR":
            keys.update(keys_of(f["xml"]))
            rf = remoteformat(f["xml"])
            tid = rf.get("id")
            step.update({"text_id": tid, "antitag": rf.get("antitag"), "knubot": rf.get("knubot"),
                         "chat_text_type": f["chat_text_type"], "flag": f["flag"], "voice": f["voice"]})
            if not d["texts"] or d["texts"][-1]["id"] != tid:
                d["texts"].append({"id": tid, "text": texts.get(tid), "answer_id": d["_offered"],
                                   "antitags": [], "knubot": rf.get("knubot")})
                d["_offered"] = None
            d["texts"][-1]["antitags"].append(rf.get("antitag"))
            name_id = npc_name_id(rf["parameters"])
            if name_id is not None:
                d["npc"]["name_id"] = name_id
                d["npc"]["name_text"] = names_db.get(name_id)
        elif x["name"] == "KnubotAnswerListIIR":
            ids = []
            for xml in f["xml"]:
                keys.update(keys_of(xml))
                ids.append(remoteformat(xml).get("id"))
            lst = {"t": x["t"], "after_text": d["texts"][-1]["id"] if d["texts"] else None,
                   "answers": [{"id": i, "text": answers_db.get(i)} for i in ids]}
            d["answer_lists"].append(lst)
            step["answers"] = ids
        elif x["name"] == "KnubotAnswerIIR":
            lst = d["answer_lists"][-1] if d["answer_lists"] else None
            i = f["answer_index"]
            a = lst["answers"][i] if lst and i < len(lst["answers"]) else {"id": None, "text": None}
            d["picked"].append({"t": x["t"], "index": i, "id": a["id"], "text": a["text"]})
            step.update({"answer_index": i, "answer_id": a["id"]})
            d["_offered"] = a["id"]
        elif x["name"] == "KnubotCloseChatWindowIIR":
            keys.update(keys_of(f["xml"]))
            rf = remoteformat(f["xml"])
            d["close"] = {"t": x["t"], "close_type": f["close_type"], "id": rf.get("id"),
                          "category": rf.get("category")}
            del open_by_player[player[1]]
        d["steps"].append(step)
    outside += [s for _, s in pending]
    for d in dialogues:
        d.pop("_offered", None)
        if d["close"] is None:
            d["note"] = "no close captured"
    return dialogues, orphans, outside, keys


def start_of(x, d, dialogues, arrivals, goals):
    """What preceded the Open: (start, follows, evidence). start: "after_dialogue" (the same
    NPC's previous dialogue with this player closed within 1 s; follows = its first text),
    "arrival" (the player's CharInPlayIIR_t within 1.5 s), "quest_goal" (a QuestGoalIIR within
    0.1 s), else "server" (no cause in the capture: proximity, combat, ...)."""
    player, ts = x["identity"][1], x["ts_raw"]
    evidence = []
    prev = next((p for p in reversed(dialogues) if p["player"][1] == player), None)
    after = (prev is not None and prev["close"] is not None and prev["npc"]["identity"] == d["npc"]["identity"]
             and ts - round(prev["close"]["t"] * TICKS) <= AFTER_TICKS)
    if after:
        evidence.append(f"dialogue {prev['index']} of this NPC closed {x['t'] - prev['close']['t']:.2f} s before")
    if player in arrivals and ts - arrivals[player] <= ARRIVAL_TICKS:
        evidence.append(f"CharInPlayIIR_t (arrival) {(ts - arrivals[player]) / TICKS:.2f} s before")
    if player in goals and ts - goals[player] <= GOAL_TICKS:
        evidence.append(f"QuestGoalIIR {(ts - goals[player]) / TICKS:.3f} s before")
    if after:
        return "after_dialogue", prev["texts"][0]["id"] if prev["texts"] else None, evidence
    if evidence and evidence[0].startswith("CharInPlay"):
        return "arrival", None, evidence
    if evidence:
        return "quest_goal", None, evidence
    return "server", None, evidence


def npc_summary(dialogues):
    out = {}
    for d in dialogues:
        pf = str(d["playfield"])
        e = out.setdefault(pf, {"playfield_name": d["playfield_name"], "npcs": {}})
        n = e["npcs"].setdefault(str(d["npc"]["identity"][1]), {
            "name": d["npc"].get("name"), "name_id": d["npc"].get("name_id"),
            "template_id": d["npc"].get("template_id"), "monster_data": d["npc"].get("monster_data"),
            "pos": d["npc"].get("pos"), "dialogues": []})
        n["dialogues"].append([t["id"] for t in d["texts"]])
    return out


def run_livecap(capture, jsonl):
    subprocess.run([sys.executable, str(LIVECAP), capture, "--jsonl", jsonl], check=True,
                   stdout=subprocess.DEVNULL)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("capture", nargs="?", default=str(ROOT.parent / "LMO_Captures.cap"))
    ap.add_argument("--out", default=str(OUT))
    ap.add_argument("--jsonl", help="existing livecap --jsonl dump (skips decoding the capture)")
    ap.add_argument("--objects", default=str(OBJECTS), help="playfield-objects.json (NPC names)")
    args = ap.parse_args()

    tmpdir = None
    jsonl = args.jsonl
    if not jsonl:
        tmpdir = tempfile.mkdtemp(prefix="livedlg-")
        jsonl = os.path.join(tmpdir, "rpc.jsonl")
        run_livecap(args.capture, jsonl)
    with open(jsonl) as fh:
        rows = [json.loads(line) for line in fh if line.strip()]

    import tdc
    texts, answers_db, names_db = tdc.texts(50002), tdc.texts(50001), tdc.texts(51000)
    npcs_pf, npcs_inst, pf_names = load_npcs(Path(args.objects))
    recs, unparsed = records(rows)
    dialogues, orphans, outside, keys = build(recs, npcs_pf, npcs_inst, pf_names, texts, answers_db, names_db)

    counts = {}
    for x in recs:
        counts[x["name"]] = counts.get(x["name"], 0) + 1
    out = {
        "source": os.path.basename(args.capture),
        "dialogues": dialogues,
        "npcs": npc_summary(dialogues),
        "orphan_answers": orphans,
        "orphan_answers_note": "client answers with no dialogue open: the server records of that "
                               "dialogue are not in the capture (a hole in its zlib stream)",
        "outside_dialogues": outside,
        "keys": dict(sorted(keys.items())),
        "unparsed": unparsed,
        "stats": {"records": counts, "unparsed": len(unparsed), "dialogues": len(dialogues),
                  "orphan_answers": len(orphans)},
    }
    path = Path(args.out)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(f"{len(dialogues)} dialogues, {len(orphans)} orphan answers, {len(unparsed)} unparsed -> {path}")
    for d in dialogues:
        picked = "".join(str(p["index"]) for p in d["picked"])
        print(f"  {d['t']:9.3f} {d['playfield']} {d['npc'].get('name')} ({d['npc']['identity'][1]}): "
              f"{[t['id'] for t in d['texts']]} picked {picked}, {d['start']} {d['start_evidence']}")
    print(json.dumps(out["stats"]))
    if tmpdir:
        for f in os.listdir(tmpdir):
            os.remove(os.path.join(tmpdir, f))
        os.rmdir(tmpdir)


if __name__ == "__main__":
    main()
