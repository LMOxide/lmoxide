"""Arrival points for every teleport destination in data/destinations.json: for each playfield a
portal leads to, and every other named playfield (data/playfields.json, e.g. the 7xxx minigames
reached through encounters, for `!tp`; not their sound and art layers), the first non-discoverable resurrection point (RDB 1000090 XML; Guid, position,
Orientation) and its name (RDB 1070003 playfield description). Existing entries (and their
teleport window ids) are kept.  Usage: tools/re/py extract_destinations.py"""
import json, os, re

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
path = os.path.join(ROOT, "data/destinations.json")
data = json.load(open(path))
idx = rdb.index()
targets = {p["to"] for lst in data["portals"].values() for p in lst}
# skipping the sound and art layers of other playfields ("Medieval - Kingdom - SOUND")
targets |= {int(k) for k, name in json.load(open(os.path.join(ROOT, "data/playfields.json")))["playfields"].items()
            if not re.search(r"[ _-](sound|art)$", name, re.I)}
added, missing = [], []
for pf in sorted(targets):
    if str(pf) in data["destinations"]:
        continue
    try:
        xml = rdb.body(1000090, pf, idx).decode("utf-8", "replace")
    except Exception:
        missing.append(pf)
        continue
    points = re.findall(r'<resurrectionpoint ([^>]*)>\s*<position x="([-\d.]+)" y="([-\d.]+)" z="([-\d.]+)"', xml)
    if not points:
        missing.append(pf)
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
print("added", len(added), added, "\nno data locally:", missing)
