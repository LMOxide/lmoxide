"""Every playfield's resurrection points (teleport spots on the World Map) from RDB 1000090 XML:
data/resurrection_points.json, playfield -> [{guid, name, pos, rot, restoration, shady,
discoverable}].  Usage: tools/re/py extract_resurrection_points.py"""
import json, os, re

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
idx = rdb.index()
out = {}
for (t, pf) in sorted(idx):
    if t != 1000090:
        continue
    try:
        xml = rdb.body(t, pf, idx).decode("utf-8", "replace")
    except Exception:
        continue
    points = []
    for a, x, y, z in re.findall(r'<resurrectionpoint ([^>]*)>\s*<position x="([-\d.]+)" y="([-\d.]+)" z="([-\d.]+)"', xml):
        at = dict(re.findall(r'(\w+)="([^"]*)"', a))
        points.append({
            "guid": int(at.get("Guid", 0)),
            "name": at.get("name", ""),
            "pos": [round(float(v), 2) for v in (x, y, z)],
            "rot": round(float(at.get("Orientation", 0)), 3),
            "restoration": int(float(at.get("RestorationLevel", 100))),
            "shady": at.get("shady") == "true",
            "discoverable": at.get("discoverable") == "true",
        })
    if points:
        out[str(pf)] = points
json.dump(out, open(os.path.join(ROOT, "data/archive/resurrection_points.json"), "w"), separators=(",", ":"), sort_keys=True)
print(len(out), "playfields,", sum(map(len, out.values())), "points")
