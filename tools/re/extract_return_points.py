"""Return points: where a player arrives in playfield A when coming back from playfield B through
a teleport window. Live used source-specific teleport points ("TP_From<source>", named in the
teleport windows, RDB 1000644) whose positions were server-side data. As a stand-in, this picks a
walkable spot (navmesh, RDB 1000051) 5-8 m from A's portal to B, on the side facing A's main
arrival point, facing away from the portal. Reads data/destinations.json (portals, arrivals),
writes data/return_points.json {A: {B: {pos, rot}}}.  Usage: tools/re/py extract_return_points.py"""
import json, math, os

import navmesh
from rdb import index

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
path = os.path.join(ROOT, "data/destinations.json")
out_path = os.path.join(ROOT, "data/return_points.json")
# outside the server's portal trigger radius (teleport.rs PORTAL_RADIUS 3 m)
RINGS = (5.0, 6.5, 8.0)
DIRECTIONS = 24
MAX_CLIMB = 2.0


def ground(cells, x, z, near_y):
    ys = [c["min"][1] + 0.6 * (c["max"][1] - c["min"][1]) for c in cells
          if c["min"][0] <= x <= c["max"][0] and c["min"][2] <= z <= c["max"][2]]
    return min(ys, key=lambda y: abs(y - near_y)) if ys else None


def main():
    data = json.load(open(path))
    idx = index()
    returns, missing = {}, []
    for here, portals in sorted(data["portals"].items()):
        cells = navmesh.cells(int(here), idx)
        dest = data["destinations"].get(here)
        for p in portals:
            src = str(p["to"])
            if src == here or not cells:
                continue
            px, py, pz = p["pos"]
            # prefer stepping out towards the playfield's main arrival point
            toward = math.atan2(dest["pos"][0] - px, dest["pos"][2] - pz) if dest else 0.0
            best = None
            for r in RINGS:
                for k in range(DIRECTIONS):
                    a = 2 * math.pi * k / DIRECTIONS
                    x, z = px + r * math.sin(a), pz + r * math.cos(a)
                    y = ground(cells, x, z, py)
                    if y is None or abs(y - py) > MAX_CLIMB:
                        continue
                    # too close to another portal would open that one's window
                    if any(math.hypot(x - q["pos"][0], z - q["pos"][2]) < 4.0 for q in portals if q is not p):
                        continue
                    score = abs(math.remainder(a - toward, 2 * math.pi)) + 0.1 * r
                    if best is None or score < best[0]:
                        best = (score, [round(x, 2), round(y, 2), round(z, 2)], round(a, 3))
            if best is None:
                missing.append((here, src))
                continue
            returns.setdefault(here, {})[src] = {"pos": best[1], "rot": best[2]}
    lines = [f' "{k}": {json.dumps(v)}' for k, v in returns.items()]
    open(out_path, "w").write("{\n" + ",\n".join(lines) + "\n}\n")
    print(sum(len(v) for v in returns.values()), "return points; none found for", missing)


main()
