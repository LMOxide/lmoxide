"""Teleport window of every portal in data/destinations.json (adds "window" to each portal entry).

Windows are RDB 1000644/1 (TeleportWindowData protobuf: window id, entries with destination
playfield, type, level, quest, cooldown buff, teleport_point, ...). Every window has one entry.
A destination can have several windows; which portal used which was server script data, so
this picks the one whose teleport_point names the source ("TP_From<source>"), else the
destination's plain window (see docs/TUNABLES.md).  Usage: tools/re/py extract_portal_windows.py"""
import json, os

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
path = os.path.join(ROOT, "data/archive/destinations.json")
HUB = 1200


def varint(b, o):
    r = s = 0
    while True:
        c = b[o]
        o += 1
        r |= (c & 0x7F) << s
        s += 7
        if c < 0x80:
            return r, o


def parse(b):
    o, out = 0, []
    while o < len(b):
        k, o = varint(b, o)
        f, t = k >> 3, k & 7
        if t == 0:
            v, o = varint(b, o)
        elif t == 2:
            n, o = varint(b, o)
            v, o = b[o:o + n], o + n
        elif t == 5:
            v, o = b[o:o + 4], o + 4
        elif t == 1:
            v, o = b[o:o + 8], o + 8
        else:
            raise ValueError((t, o))
        out.append((f, v))
    return out


def windows():
    """window id -> (destination playfield, teleport_point or None)"""
    d = rdb.body(1000644, 1, rdb.index())
    for start in range(40):
        try:
            top = parse(d[start:])
            if all(f == 1 for f, _ in top):
                break
        except Exception:
            pass
    out = {}
    for _, w in top:
        win = parse(w)
        wid = next(v for f, v in win if f == 1)
        for f, e in win:
            if f == 2:
                entry = dict(parse(e))
                out[wid] = (entry[1], entry[7].decode() if 7 in entry else None)
    return out


# teleport_point -> source playfields it is the arrival from
TP_SOURCES = {
    "TP_FromSpidersNest": {5610}, "TP_FromMysteriousCrystalCave": {5620}, "TP_FromRuinedFort": {5300},
    "TP_FromPirateStorage": {4630}, "TP_FromVolcanoIsland": {4210}, "TP_EntranceFarmlands": {4610},
    "TP_FromMercCaveMicro": {1600}, "TP_FromCrystalCaveMicro": {1610}, "TP_FromCrystalMineMicro": {1612},
    "TP_FromMarsWasteland": {3210}, "TP_FromMercHangarMicro": {1620, 1622}, "TP_FromLostCreation": {6300, 7001},
}
# the plain window where a destination has several without a teleport_point
PREFERRED = {3200: 8, 3210: 22, 5210: 20, 5500: 17}
# window 26 ("from Space Colony") has no teleport_point
SPECIAL = {(3200, 3210): 26}

data = json.load(open(path))
wins = windows()
for src, portals in data["portals"].items():
    src = int(src)
    for p in portals:
        to = HUB if p["to"] == src else p["to"]
        cands = sorted(w for w, (pf, _) in wins.items() if pf == to)
        pick = next((w for w in cands if src in TP_SOURCES.get(wins[w][1], ())), None) or SPECIAL.get((src, to))
        if pick is None:
            plain = [w for w in cands if wins[w][1] is None]
            pick = PREFERRED.get(to, plain[0] if plain else (cands[0] if cands else 0))
        p["window"] = pick
lines = ["{", ' "_comment": ' + json.dumps(data["_comment"]) + ",", ' "destinations": {']
items = list(data["destinations"].items())
lines += [f'  "{k}": {json.dumps(v)}' + ("," if i < len(items) - 1 else "") for i, (k, v) in enumerate(items)]
lines += [" },", ' "portals": {']
items = list(data["portals"].items())
lines += [f'  "{k}": {json.dumps(v)}' + ("," if i < len(items) - 1 else "") for i, (k, v) in enumerate(items)]
lines += [" }", "}"]
open(path, "w").write("\n".join(lines) + "\n")
print("portals without a window:", [(s, p["to"]) for s, l in data["portals"].items() for p in l if not p["window"]])
