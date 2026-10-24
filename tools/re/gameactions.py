"""Parse GameAction lists out of RDB records (spells 1000621, items 1000620).

GameActionData: u32 type, u32, u32 list, u32 nCriteria + 3 × u32 each, u32 n, then n × (u32 stat
key, u8 kind 0=int/1=float/2=string, value). Types: data/gameaction_types.txt (the client's
GameActionFactory registry; instantiator 0x0 = server-only action). Stat names:
data/stats.json + data/stats_linux.json.

A record (spells) is u32 version 5, u32 0xc76b, u32 number of lists, then per list u32 event
(GameData::GameActionList_e: 0 OnUse, 1 OnRechargeDelay, 33 OnChanneling (each channel tick),
35 channel start, 36 OnChannelingEnd, 39 PreUse; names from the DEV TECH test spells), u32,
u32 count and the actions; `lists` parses that, `actions` scans for actions anywhere.

  tools/re/py gameactions.py SPELL...     print a spell's game actions, by list
"""
import json, os, re, struct, sys

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
STATS = json.load(open(os.path.join(ROOT, "data/stats.json")))
STATS.update(json.load(open(os.path.join(ROOT, "data/stats_linux.json"))))
TYPES = {}
for line in open(os.path.join(ROOT, "data/gameaction_types.txt")):
    p = line.split()
    TYPES[int(p[0])] = (p[2].replace("e_Type_GC_GameAction_", ""), p[3] != "0x0")


def _try(b, o):
    t, _a, _c, ncrit = struct.unpack_from("<4I", b, o)
    if t not in TYPES or ncrit > 20:
        return None
    p = o + 16 + 12 * ncrit
    n = struct.unpack_from("<I", b, p)[0]
    p += 4
    if n > 40:
        return None
    args = {}
    for _ in range(n):
        k = struct.unpack_from("<I", b, p)[0]
        kind = b[p + 4]
        p += 5
        if kind == 0:
            v = struct.unpack_from("<i", b, p)[0]
            p += 4
        elif kind == 1:
            v = struct.unpack_from("<f", b, p)[0]
            p += 4
        elif kind == 2:
            ln = struct.unpack_from("<I", b, p)[0]
            v = b[p + 4 : p + 4 + ln].decode("latin1")
            p += 4 + ln
        else:
            return None
        args[STATS.get(str(k), str(k))] = v
    name, client = TYPES[t]
    return {"type": name, "client": client, "args": args}, p


def actions(rid, typ=1000621, idx=None):
    b = rdb.body(typ, rid, idx)
    out, o = [], 0
    while o + 20 <= len(b):
        r = _try(b, o)
        if r:
            out.append(r[0])
            o = r[1]
        else:
            o += 1
    return out


def lists(rid, typ=1000621, idx=None):
    """-> {event: [action]} (each action also has "criteria": [(a, b, kind)]), or None if the
    record doesn't parse as action lists."""
    b = rdb.body(typ, rid, idx)
    if len(b) < 12:
        return None
    version, _t, n = struct.unpack_from("<3I", b, 0)
    if version != 5 or n > 64:
        return None
    o, out = 12, {}
    for _ in range(n):
        if o + 12 > len(b):
            return None
        event, _x, count = struct.unpack_from("<3I", b, o)
        o += 12
        acts = []
        for _ in range(count):
            r = _try(b, o)
            if not r:
                return None
            ncrit = struct.unpack_from("<I", b, o + 12)[0]
            r[0]["criteria"] = [struct.unpack_from("<3I", b, o + 16 + 12 * i) for i in range(ncrit)]
            acts.append(r[0])
            o = r[1]
        out[event] = acts
    return out


if __name__ == "__main__":
    for s in sys.argv[1:]:
        ls = lists(int(s))
        if ls is None:
            for a in actions(int(s)):
                print(a)
            continue
        for event, acts in ls.items():
            print("list", event)
            for a in acts:
                print("  ", a)
