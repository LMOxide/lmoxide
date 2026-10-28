#!/usr/bin/env python3
"""Decode the world content (dynels, doors, smashables, playfield) that a live capture
delivered server -> client, and cross-check it against data/*.json.

Usage:
    python3 tools/re/live_playfield.py <capture.cap> [--out data/live-capture] [--jsonl rows.jsonl]

The capture is decoded with tools/re/livecap.py (`--jsonl`), unless --jsonl names an existing
dump. Only server -> client RPCs matter here:

* GameserverToClientRPC::PassBlob(BinaryStream): u32 BE length, then IIR records. Each record
  (client n3InfoItemRemote::Construct): u32 type id (data/iir_types.json), ID32 identity,
  bool pass-on, then class-specific data. Records we decode:
    - UpdateMessage (0x0f5fcade): creates (op 0) or removes (op 1) a dw::GameObject. Body: u8 op,
      ID32 object, u32 playfield key (BE), u32 BE length + blob. The blob is little-endian
      DreamWorld reflection data: u32 length (excluding itself), then (u32 field id, value)*.
      Field ids are ELF/PJW hashes of field names ("Id", "Position", "Rotation", "Scale", and
      the single letters x/y/z/w). Struct fields are (u32 length, pairs). Between Scale and the
      end the blob carries protobuf-style strings: 0x12 len name, 0x1a len XML (remoteformat /
      localized). The XML id is the template (destructibles, buildies) or the GUID (doors).
    - PlayfieldFIIR_t (0x5d85bfc7): header, 3 f32 (spawn position of the arrival), u32 length +
      PlayfieldProxyData protobuf (model_id = playfield, running_instance = playfield key).
    - FullPlayerCharacterIIR (0x737a6df9): the player's own character. Header protobuf
      ProtoFullCharacterIIR (decoded for counts only); the rest is not decoded.
  Other records are listed in "unparsed" with their type name.
* LoginConnectReady / ChatServerConnectReady / OverrideGameTweaks / SendCharacterSettings
  (PlayerInterface, 37.18.192.52:7101): login messages, decoded for the playfield and server
  address only.

Dynel type ids (from the server code): 50000 character, 51016 door, 51320 simple dynel,
51321 destructible (smashable, or buildie when its template is in data/buildies.json),
51322 loot bag.

The playfield key is also cross-checked: every UpdateMessage's playfield key must equal the
PlayfieldFIIR running_instance.

A struct with an unknown field id is reported as an "undecoded" object instead of guessed.
"""
import argparse
import json
import math
import os
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "data"
PROTO = ROOT / "proto"
LIVECAP = ROOT / "tools" / "re" / "livecap.py"

DYNEL_KIND = {
    50000: "character",
    51016: "door",
    51320: "simple_dynel",
    51321: "destructible",
    51322: "loot_bag",
    51341: "projectile",
}


def elf_hash(name):
    """ELF/PJW hash, the DreamWorld reflection field id (verified: "Id", "Position", ...)."""
    h = 0
    for c in name.encode():
        h = ((h << 4) + c) & 0xFFFFFFFF
        g = h & 0xF0000000
        if g:
            h ^= g >> 24
        h &= ~g & 0xFFFFFFFF
    return h


FID_ID = elf_hash("Id")
FID_POS = elf_hash("Position")
FID_ROT = elf_hash("Rotation")
FID_SCALE = elf_hash("Scale")
AXIS = {elf_hash(c): c for c in "xyzw"}


def load_iir_types():
    rows = json.loads((DATA / "iir_types.json").read_text())
    names = {int(r["type_id"], 16): r["name"] for r in rows}
    return names


IIR_NAMES = load_iir_types()
IIR_IDS = {v: k for k, v in IIR_NAMES.items()}
T_UPDATE = IIR_IDS["UpdateMessage"]
T_PLAYFIELD = IIR_IDS["PlayfieldFIIR_t"]
T_FULL_CHAR = IIR_IDS["FullPlayerCharacterIIR"]


def be32(b, p):
    return struct.unpack_from(">I", b, p)[0]


def le32(b, p):
    return struct.unpack_from("<I", b, p)[0]


def f32(b, p):
    return struct.unpack_from("<f", b, p)[0]


def protoc_decode(message, proto_file, payload):
    """Decode a protobuf with the repo's proto/ files via protoc; returns text or None."""
    try:
        out = subprocess.run(
            ["protoc", f"--decode={message}", "-I", str(PROTO), str(PROTO / proto_file)],
            input=payload, capture_output=True, timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if out.returncode != 0:
        return None
    return out.stdout.decode(errors="replace")


def run_livecap(capture, jsonl):
    subprocess.run([sys.executable, str(LIVECAP), capture, "--jsonl", jsonl], check=True,
                   stdout=subprocess.DEVNULL)


def read_rows(jsonl):
    with open(jsonl) as fh:
        return [json.loads(line) for line in fh if line.strip()]


def printable(bs):
    return all(0x20 <= c <= 0x7E for c in bs)


def parse_reflection_object(blob):
    """blob: LE u32 length + fields (DreamWorld reflection). Returns dict with the fields
    decoded, the offset where decoding stopped, and the tail bytes."""
    out = {}
    if len(blob) < 4:
        return {"error": "short blob"}
    total = le32(blob, 0)
    end = 4 + total
    if end > len(blob):
        return {"error": f"blob length {total} exceeds {len(blob) - 4}"}
    p = 4
    while p + 4 <= end:
        fid = le32(blob, p)
        p += 4
        if fid == FID_ID:
            n = le32(blob, p)
            p += 4
            sub = []
            q = p
            while q + 8 <= p + n:
                sub.append(le32(blob, q + 4))
                q += 8
            if len(sub) == 2:
                out["id"] = (sub[0], sub[1])
            else:
                out["id_fields"] = sub
            p += n
        elif fid in (FID_POS, FID_ROT, FID_SCALE):
            n = le32(blob, p)
            p += 4
            vals = {}
            q = p
            while q + 8 <= p + n:
                ax = AXIS.get(le32(blob, q))
                if ax is None:
                    return {**out, "error": f"unknown struct field {le32(blob, q):#x}", "stop": q}
                vals[ax] = round(f32(blob, q + 4), 4)
                q += 8
            p += n
            key = {FID_POS: "pos", FID_ROT: "rot", FID_SCALE: "scale"}[fid]
            if key == "pos":
                out["pos"] = [vals.get("x"), vals.get("y"), vals.get("z")]
            elif key == "rot":
                out["rot"] = [vals.get("x"), vals.get("y"), vals.get("z"), vals.get("w")]
            else:
                out["scale"] = [vals.get("x"), vals.get("y"), vals.get("z")]
        else:
            # Not a known field: the reflection layout past Scale is not decoded. Stop here;
            # the strings are found by scanning below.
            p -= 4
            break
    out["after_fields"] = p
    out["end"] = end
    return out


def find_name_and_xml(blob, start, end):
    """The protobuf-style strings after the reflection fields: 0x12 len name, 0x1a len xml.
    Returns (name, xml, offset) or (None, None, None)."""
    for i in range(start, end - 4):
        if blob[i] != 0x12:
            continue
        n1 = blob[i + 1]
        if n1 == 0 or i + 2 + n1 >= end:
            continue
        name = blob[i + 2:i + 2 + n1]
        if blob[i + 2 + n1] != 0x1A or not printable(name):
            continue
        n2 = blob[i + 3 + n1]
        xml = blob[i + 4 + n1:i + 4 + n1 + n2]
        if n2 and i + 4 + n1 + n2 <= end and printable(xml):
            return name.decode(), xml.decode(), i
    return None, None, None


def xml_id(xml):
    """Template (remoteformat) or GUID (localized) id of the display XML, and its category."""
    m = re.search(r'<(remoteformat|localized) id="(\d+)" category="(\d+)"', xml)
    if not m:
        return None, None, None
    return m.group(1), int(m.group(2)), int(m.group(3))


def decode_character_core(blob, start):
    """Character GameObject blob: after the reflection fields comes u32 BE length +
    ProtoCoreCharacterIIR (proto/CoreCharacterIIR.proto; field 1 = name). Its offset is not
    fixed, so it is found as the first (length, 0x0a) pair that protoc decodes to that length.
    Returns a dict of the decoded fields, or None."""
    for i in range(start, len(blob) - 6):
        if blob[i + 4] != 0x0A:
            continue
        n = be32(blob, i)
        if n < blob[i + 5] + 2 or i + 4 + n > len(blob):
            continue
        text = protoc_decode("ProtoCoreCharacterIIR", "CoreCharacterIIR.proto",
                             blob[i + 4:i + 4 + n])
        if not text or not text.startswith("name:"):
            continue
        # ProtoGameCodeDynelIIR (position, rotation, playfield) earlier in the blob also
        # parses: reject escaped (binary) names and unknown field numbers
        fields = core_fields(text)
        if re.search(r"\\(\d{3}|[rnt])", fields.get("name", "")) or \
                re.search(r"^\s*\d+:", text, re.M):
            continue
        return {"core_offset": i, "core_len": n, **fields}
    return None


def core_fields(text):
    """Flatten the interesting parts of protoc's text output: top-level scalars and the
    stats / npc_stats / player_stats sub-messages (one level deep)."""
    out, section = {}, None
    for line in text.splitlines():
        t = line.strip()
        if t.endswith("{"):
            section = t[:-1].strip() if line.startswith(" ") is False else None
            continue
        if t == "}":
            section = None
            continue
        if ":" not in t:
            continue
        k, v = t.split(":", 1)
        v = v.strip()
        if v.startswith('"'):
            v = re.sub(r"\\(['\"\\])", r"\1", v[1:-1])
        else:
            try:
                v = int(v, 0)
            except ValueError:
                try:
                    v = float(v)
                except ValueError:
                    pass
        if section in ("stats", "npc_stats", "player_stats", "bcc"):
            out.setdefault(section, {})[k] = v
        elif section is None and not line.startswith(" "):
            out[k] = v
    return out


def decode_update_message(b, p):
    """UpdateMessage record at b[p] (p = record type offset, after the PassBlob length):
    type(4) ID32(8) bool(1) op(1) ID32(8) playfield key(4) BE length(4) blob.
    Returns (record dict, next offset)."""
    q = p + 13
    op = b[q]
    q += 1
    obj_type, obj_inst = be32(b, q), be32(b, q + 4)
    q += 8
    if op != 0:  # despawn: type(4) ID32(8) bool(1) op(1) ID32(8), no key or blob
        return {"op": op, "identity": [be32(b, p + 4), be32(b, p + 8)],
                "obj": [obj_type, obj_inst], "kind": "despawn"}, q
    playfield_key = be32(b, q)
    q += 4
    blen = be32(b, q)
    q += 4
    blob = b[q:q + blen]
    next_p = q + blen
    res = {
        "op": op,
        "identity": [be32(b, p + 4), be32(b, p + 8)],
        "obj": [obj_type, obj_inst],
        "playfield_key": playfield_key,
        "blob_len": blen,
    }
    if op != 0:
        return {**res, "kind": "despawn"}, next_p
    parsed = parse_reflection_object(blob)
    if "error" in parsed:
        return {**res, "kind": "undecoded", "error": parsed["error"]}, next_p
    name, xml, name_off = (None, None, None)
    if obj_type == 50000:
        core = decode_character_core(blob, parsed["after_fields"])
        if core:
            res["core"] = core
            name = core.get("name")
    elif obj_type != 51341:
        name, xml, name_off = find_name_and_xml(blob, parsed["after_fields"], parsed["end"])
    res.update(parsed)
    res["name"] = name
    res["xml"] = xml
    if xml:
        tag, ident, category = xml_id(xml)
        res["xml_tag"], res["xml_id"], res["xml_category"] = tag, ident, category
    res["tail_bytes"] = len(blob) - parsed["end"]
    res["unknown_fields_at"] = parsed["after_fields"]
    return res, next_p


def decode_playfield(b, p, end):
    """PlayfieldFIIR_t (body at p+13): 3 f32 arrival position, u32 length + PlayfieldProxyData.
    The class data after the protobuf is not decoded, so the record is taken to end the blob."""
    base = p + 13
    pos = [round(struct.unpack_from(">f", b, base + 4 * i)[0], 4) for i in range(3)]
    plen = be32(b, base + 12)
    proto_bytes = b[base + 16:base + 16 + plen]
    text = protoc_decode("PlayfieldProxyData", "PlayfieldProxyData.proto", proto_bytes) or ""
    m = re.search(r"model_id \{\s*type: (\d+)\s*instance: (\d+)", text)
    rm = re.search(r"running_instance: (\d+)", text)
    fields = {
        "arrival_pos": pos,
        "model_id": [int(m.group(1)), int(m.group(2))] if m else None,
        "running_instance": int(rm.group(1)) if rm else None,
        "proto_len": plen,
    }
    return fields, end


def decode_full_character(b, p, end):
    """FullPlayerCharacterIIR (body at p+13): u32 BE length + ProtoFullCharacterIIR, then the
    class data (not decoded)."""
    base = p + 13
    plen = be32(b, base)
    proto_bytes = b[base + 4:base + 4 + plen]
    text = protoc_decode("ProtoFullCharacterIIR", "FullCharacterIIR.proto", proto_bytes) or ""
    counts = {
        "character_tags": len(re.findall(r"^character_tag:", text, re.M)),
        "inventory_items": len(re.findall(r"^\s+item_id:", text, re.M)),
        "quests_added": len(re.findall(r"added_main_quest \{", text)),
    }
    return {"proto_len": plen, "counts": counts}, end


def decode_passblob(b, rpc_index):
    """b: PassBlob BinaryStream arg (u32 BE length + records). Returns (records, unparsed).
    Each PassBlob holds the records of one dynel/playfield/character update; a record's size is
    known only for UpdateMessage (its blob length), so the rest of the blob is one record."""
    recs, unparsed = [], []
    end = 4 + be32(b, 0)
    p = 4
    while p + 13 <= end:
        t = be32(b, p)
        ident = [be32(b, p + 4), be32(b, p + 8)]
        name = IIR_NAMES.get(t)
        if t == T_UPDATE:
            rec, nxt = decode_update_message(b, p)
            rec["raw_offset"] = p
            recs.append(rec)
            p = nxt
            continue
        if t == T_PLAYFIELD:
            fields, nxt = decode_playfield(b, p, end)
            recs.append({"kind": "playfield", "raw_offset": p, "identity": ident, **fields})
            p = nxt
            continue
        if t == T_FULL_CHAR:
            fields, nxt = decode_full_character(b, p, end)
            unparsed.append({"record": name, "raw_offset": p, "identity": ident,
                             "note": "player's own character: header protobuf counted, the "
                                     "class data is not decoded", **fields})
            p = nxt
            continue
        unparsed.append({"record": name or f"type {t:#010x}", "raw_offset": p,
                         "identity": ident, "len": end - p,
                         "note": "size not known; rest of blob"})
        break
    # complete: every byte of the blob belongs to a decoded record (lengths add up)
    complete = p == end
    return recs, unparsed, complete


def decode_login(row, b):
    """LoginConnectReady etc. (PlayerInterface, S->C): address, cookie, character, PF proxy."""
    m = row["method"]
    if m.startswith("LoginConnectReady"):
        n = struct.unpack_from(">H", b, 0)[0]
        addr = b[2:2 + n].decode(errors="replace")
        cookie = be32(b, 2 + n)
        char = [be32(b, 6 + n), be32(b, 10 + n)]
        plen = be32(b, 14 + n)
        proto_bytes = b[18 + n:18 + n + plen]
        text = protoc_decode("PlayfieldProxyData", "PlayfieldProxyData.proto", proto_bytes) or ""
        mm = re.search(r"model_id \{\s*type: (\d+)\s*instance: (\d+)", text)
        return {"record": "LoginConnectReady", "address": addr, "cookie": cookie,
                "character": char, "playfield": [int(mm.group(1)), int(mm.group(2))]
                if mm else None}
    if m.startswith("ChatServerConnectReady"):
        n = struct.unpack_from(">H", b, 0)[0]
        return {"record": "ChatServerConnectReady", "address": b[2:2 + n].decode(errors="replace")}
    if m.startswith("SendCharacterSettings("):
        return {"record": "SendCharacterSettings", "character": [be32(b, 0), be32(b, 4)],
                "data_len": be32(b, 8)}
    if m.startswith("OverrideGameTweaks"):
        return {"record": "OverrideGameTweaks", "note": "empty DynamicGameTweaksMap"}
    return None


# ---- cross-check -------------------------------------------------------------------------

def load_json(name):
    path = DATA / name
    if not path.exists():
        return None
    return json.loads(path.read_text())


def playfield_name(playfield):
    names = (load_json("playfields.json") or {}).get("playfields", {})
    return names.get(str(playfield), "?")


# Which existing sources can describe each kind of object (position-only matches must stay
# within the same kind: a crate is not an enemy).
KIND_SOURCES = {
    "destructible": {"world_content.smashables", "buildies"},
    "buildie": {"world_content.smashables", "buildies"},
    "door": {"world_content.doors"},
    "simple_dynel": {"world_content.simple_dynels"},
    "character": {"world_content.npcs", "world_content.enemies", "return_points",
                  "destinations.arrival", "world_content.resurrection_points",
                  "world_content.static_points"},
}


def dist(a, b):
    if not a or not b:
        return None
    return math.dist(a[:3], b[:3])


def candidates_for(playfield):
    """Existing entries for this playfield: (source, id, pos, label)."""
    pf = str(playfield)
    out = []
    wc = load_json("world_content.json") or {}
    entry = wc.get(pf) or {}
    for key, idkey in (("smashables", "template"), ("enemies", None), ("npcs", None),
                       ("doors", "guid"), ("simple_dynels", "guid"), ("portals", None),
                       ("resurrection_points", "guid"), ("static_points", None)):
        for e in entry.get(key, []) if isinstance(entry.get(key), list) else []:
            out.append(("world_content." + key, e.get(idkey) if idkey else None,
                        e.get("pos"), e.get("name"), e.get("source")))
    for e in load_json("buildies.json") or []:
        if e.get("playfield") == playfield:
            out.append(("buildies", e.get("template"), e.get("pos"), e.get("name"), None))
    spawns = load_json("spawns.json") or {}
    for e in spawns.get(pf, []) if isinstance(spawns.get(pf), list) else []:
        out.append(("spawns", e.get("template") or e.get("monster_data"), e.get("pos"),
                    e.get("name"), None))
    rp = (load_json("return_points.json") or {}).get(pf)
    if isinstance(rp, dict):
        out.append(("return_points", None, rp.get("pos"), "return point", None))
    dest = load_json("destinations.json") or {}
    d = (dest.get("destinations") or {}).get(pf)
    if isinstance(d, dict):
        out.append(("destinations.arrival", d.get("guid"), d.get("pos"), d.get("name"), None))
    for e in (dest.get("portals") or {}).get(pf, []):
        out.append(("destinations.portal", None, e.get("pos"), f"to {e.get('to')}", None))
    return out


def match_object(obj, cands, kind):
    """Best existing entry for a decoded object: ID match (template / GUID) and distance."""
    best = None
    oid = obj.get("xml_id")
    allowed = KIND_SOURCES.get(kind, set())
    for src, cid, pos, label, _ in cands:
        if src not in allowed:
            continue
        d = dist(obj.get("pos"), pos)
        same_id = cid is not None and oid is not None and cid == oid
        if same_id:
            cat = "id+pos" if d is not None and d <= 3.0 else "id only"
        elif d is not None and d <= 3.0:
            cat = "pos only"
        else:
            continue
        score = (0 if cat == "id+pos" else 1 if cat == "id only" else 2, d or 1e9)
        if best is None or score < best[0]:
            best = (score, {"source": src, "match": cat, "existing_name": label,
                            "distance_m": round(d, 2) if d is not None else None})
    return best[1] if best else {"match": "new"}


# ---- main --------------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("capture")
    ap.add_argument("--out", default=str(ROOT / "data" / "live-capture"))
    ap.add_argument("--jsonl", help="existing livecap --jsonl dump (skips decoding the capture)")
    args = ap.parse_args()

    tmpdir = None
    jsonl = args.jsonl
    if not jsonl:
        tmpdir = tempfile.mkdtemp(prefix="livepf-")
        jsonl = os.path.join(tmpdir, "rpc.jsonl")
        run_livecap(args.capture, jsonl)
    rows = read_rows(jsonl)

    objects, unparsed, records = [], [], []
    blobs_complete = 0
    playfield_recs = []
    login = []
    pb_index = 0
    for row in rows:
        if row.get("dir") != "S->C" or not row.get("args_hex"):
            continue
        raw = bytes.fromhex(row["args_hex"])
        if row["method"].startswith("PassBlob"):
            pb_index += 1
            ordinal = pb_index
            recs, unp, complete = decode_passblob(raw, ordinal)
            if complete and not unp:
                blobs_complete += 1
            base = {"rpc": ordinal, "flow": row["flow"], "frame": row.get("frame"),
                    "ts_raw": row.get("ts_raw"), "args_len": len(raw)}
            for r in recs:
                if r.get("kind") == "playfield":
                    playfield_recs.append({**base, **r})
                elif "obj" in r:
                    records.append({**base, **r})
            for u in unp:
                unparsed.append({**base, **u})
        else:
            info = decode_login(row, raw)
            if info:
                login.append({"flow": row["flow"], "frame": row.get("frame"), **info})

    # playfields: PlayfieldFIIR running_instance (the UpdateMessage key) -> model id
    pf_by_key, visits = {}, []
    for p in playfield_recs:
        if p.get("model_id") and p.get("running_instance") is not None:
            pf_by_key[p["running_instance"]] = p["model_id"][1]
            visits.append({"playfield": p["model_id"][1],
                           "name": playfield_name(p["model_id"][1]),
                           "key": p["running_instance"], "flow": p["flow"], "rpc": p["rpc"],
                           "arrival_pos": p["arrival_pos"]})

    # cross-check each object against the data of its own playfield
    cands_cache = {}
    out_objects = []
    unkeyed = 0
    for r in records:
        if r.get("op") != 0 or "pos" not in r:
            continue
        playfield = pf_by_key.get(r["playfield_key"])
        if playfield is None:
            unkeyed += 1
        if playfield not in cands_cache:
            cands_cache[playfield] = candidates_for(playfield) if playfield else []
        cands = cands_cache[playfield]
        typ, inst = r["obj"]
        kind = DYNEL_KIND.get(typ, f"dynel_{typ}")
        cat = r.get("xml_tag")
        if typ == 51321 and r.get("xml_id") is not None:
            buildie = any(c[1] == r["xml_id"] for c in cands if c[0] == "buildies")
            kind = "buildie" if buildie else "destructible"
        o = {
            "playfield": playfield,
            "playfield_key": r["playfield_key"],
            "kind": kind,
            "type": typ,
            "instance": inst,
            "pos": r["pos"],
            "rot": r.get("rot"),
            "scale": r.get("scale"),
            "name": r.get("name"),
            "xml": {"tag": cat, "id": r.get("xml_id"), "category": r.get("xml_category")}
            if cat else None,
            "template_or_guid": r.get("xml_id"),
            "core": r.get("core"),
            "flow": r["flow"],
            "frame": r.get("frame"),
            "t": round(r["ts_raw"] / 1e6, 3) if r.get("ts_raw") is not None else None,
            "rpc": r["rpc"],
            "raw_offset": r["raw_offset"],
            "identity_record": r["identity"],
            "cross_check": match_object(r, cands, kind) if cands else {"match": "no data"},
        }
        out_objects.append(o)
    despawns = sum(1 for r in records if r.get("kind") == "despawn")
    for r in records:
        if r.get("kind") == "undecoded":
            unparsed.append({"record": "UpdateMessage", "rpc": r["rpc"], "flow": r["flow"],
                             "obj": r.get("obj"), "note": r.get("error")})
    by_kind, by_pf = {}, {}
    for o in out_objects:
        by_kind[o["kind"]] = by_kind.get(o["kind"], 0) + 1
        k = str(o["playfield"])
        by_pf.setdefault(k, {})
        by_pf[k][o["kind"]] = by_pf[k].get(o["kind"], 0) + 1
    unparsed_by_record = {}
    for u in unparsed:
        unparsed_by_record[u["record"]] = unparsed_by_record.get(u["record"], 0) + 1
    out = {
        "source": os.path.basename(args.capture),
        "playfields": visits,
        "playfields_note": "one entry per PlayfieldFIIR_t (zone entry); key = running_instance, "
                           "which every UpdateMessage of that zone carries as its playfield key",
        "frames_note": "frame / t: capture frame and seconds from capture start of the TCP "
                       "segment that inflated the PassBlob (livecap segmap); rpc = ordinal of "
                       "the PassBlob among S->C PassBlobs in capture order",
        "objects": out_objects,
        "unparsed": unparsed,
        "login": login,
        "stats": {
            "passblobs": pb_index,
            "update_messages": len(records),
            "despawns": despawns,
            "objects_by_kind": by_kind,
            "objects_by_playfield": by_pf,
            "objects_without_playfield": unkeyed,
            "playfield_records": len(playfield_recs),
            "passblobs_fully_decoded": blobs_complete,
            "unparsed_by_record": unparsed_by_record,
        },
        "field_notes": {
            "pos": "Vector3 from the GameObject Position struct (f32 x,y,z in world units)",
            "rot": "quaternion [x,y,z,w] from the Rotation struct (client order)",
            "scale": "from the Scale struct (not a uniform scalar)",
            "instance": "ID32 instance of the GameObject (the dynel's identity)",
            "xml.id": "remoteformat id = template (destructibles, buildies); localized id = GUID "
                      "(doors, matches world_content doors' guid)",
            "name": "dev name of the dynel (protobuf field 2 in the blob)",
            "after_scale": "a field 0x076023f3 (u32, value 1 in all records; static guess: a flag) "
                           "and further unknown class data are not decoded",
            "raw_offset": "offset of the blob's record in the PassBlob argument (BinaryStream, "
                          "includes its u32 length)",
        },
    }

    outdir = Path(args.out)
    outdir.mkdir(parents=True, exist_ok=True)
    (outdir / "playfield-objects.json").write_text(
        json.dumps(out, separators=(",", ":"), ensure_ascii=False) + "\n")
    print(json.dumps({"playfields": visits, "stats": out["stats"]}, indent=1))
    if tmpdir:
        for f in os.listdir(tmpdir):
            os.remove(os.path.join(tmpdir, f))
        os.rmdir(tmpdir)


if __name__ == "__main__":
    main()
