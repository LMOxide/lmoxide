"""Checks every jump pad's landing point (data/world_content.json) against the playfield's
navmesh (RDB 1000051, navmesh.py) and prints the doubtful ones:

  short     lands within 4 m of the pad (a hop on the spot, or the way back's landing)
  off-mesh  no walkable cell under the landing point
  stranded  lands on a small island (< 4 cells) with no pad on it to leave by
  backward  lands next to (within 4 m of) a pad on the same island that throws straight back

Usage: tools/re/py check_jumppads.py [playfield ...]"""
import json, math, os, sys

from navmesh import cells, contains
from rdb import index

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
SMALL_ISLAND = 4


def flat(a, b):
    return math.dist((a[0], a[2]), (b[0], b[2]))


def throws_on(pad):
    return pad["visual"][1] == 95472 or pad.get("target_by") == "chain" or "chain" in pad["name"] or "lily" in pad["name"]


def islands(cs):
    """cell id -> island id, island id -> cell count."""
    by = {c["id"]: c for c in cs}
    label = {}
    for c in cs:
        if c["id"] in label:
            continue
        label[c["id"]], todo = c["id"], [c["id"]]
        while todo:
            for n in by[todo.pop()]["neighbours"]:
                if n in by and n not in label:
                    label[n] = c["id"]
                    todo.append(n)
    size = {}
    for v in label.values():
        size[v] = size.get(v, 0) + 1
    return label, size


def island_at(cs, label, p):
    """The island under `p` (the cell whose floor is nearest p's height), or None."""
    hits = [c for c in cs if contains(c, p, 0.5) and c["min"][1] - 2.5 <= p[1] <= c["max"][1] + 2.5]
    if not hits:
        return None
    return label[min(hits, key=lambda c: abs((c["min"][1] + c["max"][1]) / 2 - p[1]))["id"]]


def main():
    worlds = json.load(open(os.path.join(ROOT, "data/world_content.json")))
    only = set(sys.argv[1:])
    idx = index()
    total = flagged = 0
    for pf, w in sorted(worlds.items(), key=lambda kv: kv[0]):
        if not isinstance(w, dict) or (only and pf not in only):
            continue
        # (pads without the JUMP tag are scenery: jumppad.rs Pad::launches)
        pads = [d for d in w.get("simple_dynels", []) if d["kind"] == "jump_pad" and d.get("tag") == "JUMP"]
        if not pads:
            continue
        cs = cells(int(pf), idx)
        label, size = islands(cs) if cs else ({}, {})
        pad_islands = {island_at(cs, label, p["pos"]) for p in pads} if cs else set()
        for p in pads:
            total += 1
            t = p.get("target")
            if not t:
                print(f"{pf} {p['guid']} {p['name']}: no target")
                flagged += 1
                continue
            why = []
            # landing on a pad that throws the player on at once (jumppad.rs Pad::throws_on)
            if any(throws_on(q) and flat(q["pos"], t) <= 1 and abs(q["pos"][1] - t[1]) <= 2 for q in pads if q is not p):
                continue
            if flat(p["pos"], t) < 4:
                why.append(f"short ({flat(p['pos'], t):.1f} m)")
            if cs:
                isl = island_at(cs, label, t)
                if isl is None:
                    why.append("off-mesh")
                elif size[isl] < SMALL_ISLAND and isl not in pad_islands:
                    why.append(f"stranded (island of {size[isl]} cells)")
            back = [q for q in pads if q is not p and flat(q["pos"], t) < 4 and q.get("target") and flat(q["target"], p["pos"]) < 4]
            if back and flat(p["pos"], t) < 8 and cs and island_at(cs, label, p["pos"]) == island_at(cs, label, t):
                why.append(f"backward ({back[0]['guid']} throws back)")
            if why:
                flagged += 1
                pos = ", ".join(f"{v:.1f}" for v in p["pos"])
                print(f"{pf} {p['guid']} {p['name'][:32]} at ({pos}) by {p.get('target_by')}: {'; '.join(why)}")
    print(f"{flagged} of {total} pads flagged")


if __name__ == "__main__":
    main()
