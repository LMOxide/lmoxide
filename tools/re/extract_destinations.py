"""Arrival points for every teleport destination in data/destinations.json: for each playfield a
portal leads to, and every other named playfield (data/playfields.json, the 7xxx minigames, test
maps, and the sound and art layers too: `!tp` reaches every playfield, broken ones included), the
first non-discoverable resurrection point (RDB 1000090 XML; Guid, position, Orientation) and its
name (RDB 1070003 playfield description, else data/playfields.json). A playfield without
resurrection points gets a guessed one, said in `arrival`: where the live capture's player
arrived ("capture", data/live-capture/lmo-captures/playfield-objects.json), else the middle of
its walkable area ("navmesh": the centre of the best-connected cell of the largest connected
part, from the cell's top), else a scenery piece ("scenery", world_content.json simple dynels),
else the origin ("origin"). Existing entries (and their teleport window ids) are kept.
Usage: tools/re/py extract_destinations.py"""
import json, os, re

import navmesh
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
path = os.path.join(ROOT, "data/destinations.json")
data = json.load(open(path))
idx = rdb.index()
targets = {p["to"] for lst in data["portals"].values() for p in lst}
# skipping the sound and art layers of other playfields ("Medieval - Kingdom - SOUND")
names = {int(k): name for k, name in json.load(open(os.path.join(ROOT, "data/playfields.json")))["playfields"].items()}
targets |= set(names)
capture = {}
for e in json.load(open(os.path.join(ROOT, "data/live-capture/lmo-captures/playfield-objects.json")))["playfields"]:
    capture.setdefault(e["playfield"], e["arrival_pos"])
world = json.load(open(os.path.join(ROOT, "data/world_content.json")))


def guessed(pf):
    """(position, how) for a playfield without resurrection points."""
    if pf in capture:
        return [round(v, 2) for v in capture[pf]], "capture"
    cs = navmesh.cells(pf, idx)
    if cs:
        by_id = {c["id"]: c for c in cs}
        seen, parts = set(), []
        for c in cs:
            if c["id"] in seen:
                continue
            part, todo = [], [c["id"]]
            seen.add(c["id"])
            while todo:
                cur = by_id[todo.pop()]
                part.append(cur)
                for n in cur["neighbours"]:
                    if n in by_id and n not in seen:
                        seen.add(n)
                        todo.append(n)
            parts.append(part)
        best = max(max(parts, key=len), key=lambda c: len(c["neighbours"]))
        mid = [(best["min"][i] + best["max"][i]) / 2 for i in (0, 2)]
        return [round(mid[0], 2), round(best["max"][1] + 0.5, 2), round(mid[1], 2)], "navmesh"
    pieces = (world.get(str(pf)) or {}).get("simple_dynels") if isinstance(world.get(str(pf)), dict) else None
    if pieces:
        return [round(v, 2) for v in pieces[0]["pos"]], "scenery"
    return [0.0, 0.0, 0.0], "origin"


added, missing = [], []
for pf in sorted(targets):
    if str(pf) in data["destinations"]:
        continue
    try:
        xml = rdb.body(1000090, pf, idx).decode("utf-8", "replace")
    except Exception:
        xml = ""
    points = re.findall(r'<resurrectionpoint ([^>]*)>\s*<position x="([-\d.]+)" y="([-\d.]+)" z="([-\d.]+)"', xml)
    if not points:
        pos, how = guessed(pf)
        data["destinations"][str(pf)] = {"name": names.get(pf, str(pf)), "guid": 0, "pos": pos, "rot": 0.0, "window": 0, "arrival": how}
        missing.append((pf, how))
        continue
    attrs = lambda a: dict(re.findall(r'(\w+)="([^"]*)"', a))
    best = next((p for p in points if attrs(p[0]).get("discoverable") == "false"), points[0])
    a = attrs(best[0])
    name = a.get("name", "")
    try:
        desc = rdb.body(1070003, pf, idx).decode("utf-8", "replace")
        name = re.search(r'<playfield Name="([^"]*)"', desc).group(1)
    except Exception:
        pass
    data["destinations"][str(pf)] = {
        "name": name,
        "guid": int(a.get("Guid", 0)),
        "pos": [round(float(best[i]), 2) for i in (1, 2, 3)],
        "rot": round(float(a.get("Orientation", 0)), 3),
        "window": 0,
    }
    added.append(pf)
lines = ["{", ' "_comment": ' + json.dumps(data["_comment"]) + ",", ' "destinations": {']
items = list(data["destinations"].items())
lines += [f'  "{k}": {json.dumps(v)}' + ("," if i < len(items) - 1 else "") for i, (k, v) in enumerate(items)]
lines += [" },", ' "portals": {']
items = list(data["portals"].items())
lines += [f'  "{k}": {json.dumps(v)}' + ("," if i < len(items) - 1 else "") for i, (k, v) in enumerate(items)]
lines += [" }", "}"]
open(path, "w").write("\n".join(lines) + "\n")
print("added", len(added), added, "\nguessed arrivals:", missing)
