#!/usr/bin/env python3
"""Add the client's named points to the content tree: the `invisible_monsterdata` dynels of RDB
1000057 (the placed objects; visual 1010030:95472) become area markers (role `marker`, shape
`point`, source `client`) in the worlds' areas.json. The designers named some (Jungle Arena's
pads: NW_1 ... SE_4); unnamed ones get `invisible_monsterdata-<guid>`. Spawns stand on them
with `at` (docs/CONTENT.md).

Only adds or moves markers (by id); edits made to them since (`rot`, `spread`, a `comment`)
stay, except the position. Run `cargo run -p lmoxide-content -- fmt` afterwards.

  tools/re/py ../content/import_points.py [playfield ...]     (default: the Lost Creations worlds)
"""
import json
import math
import os
import re
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "re"))
import rdb  # noqa: E402

ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
LINUX = os.path.join(ROOT, "..", "clients/linux/LEGO Minifigures Online")
CONTENT = os.path.join(ROOT, "content")
MARKER_LOOK = (1010030, 95472)


def placed(idx, pf):
    """RDB 1000057 `pf`: (name, pos, yaw, guid) of each `invisible_monsterdata` dynel."""
    if (1000057, pf) not in idx:
        return []
    b = rdb.body(1000057, pf, idx, LINUX)
    (n,) = struct.unpack_from("<I", b)
    o = 4
    out = []
    for _ in range(n):
        _, ln = struct.unpack_from("<IH", b, o)
        o += 6
        name = b[o:o + ln].decode().strip()
        o += ln
        px, py, pz, qx, qy, qz, qw, _scale = struct.unpack_from("<8f", b, o)
        o += 32
        _sg, rt, rid = struct.unpack_from("<iII", b, o)
        o += 12 + 24
        guid, _, tl = struct.unpack_from("<IBH", b, o)
        o += 7 + tl
        if (rt, rid) == MARKER_LOOK:
            out.append((name, [round(px, 2), round(py, 2), round(pz, 2)], 2 * math.atan2(qy, qw), guid))
    return out


def slug(s):
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


def lc_worlds():
    """Playfields of the worlds the Lost Creations levels use."""
    out = []
    for f in os.listdir(os.path.join(CONTENT, "lost-creations")):
        world = json.load(open(os.path.join(CONTENT, "lost-creations", f)))["world"]
        for d in os.listdir(os.path.join(CONTENT, "worlds")):
            w = json.load(open(os.path.join(CONTENT, "worlds", d, "world.json")))
            if w["id"] == world:
                out.append(w["playfield"])
    return sorted(out)


def world_dir(pf):
    for d in os.listdir(os.path.join(CONTENT, "worlds")):
        if d.split("-")[0] == str(pf):
            return os.path.join(CONTENT, "worlds", d)
    return None


def main():
    pfs = [int(a) for a in sys.argv[1:]] or lc_worlds()
    idx = rdb.index(LINUX)
    for pf in pfs:
        d = world_dir(pf)
        points = placed(idx, pf)
        if not d or not points:
            print(f"{pf}: {'no world' if not d else 'no points'}")
            continue
        path = os.path.join(d, "areas.json")
        data = json.load(open(path)) if os.path.exists(path) else {"areas": []}
        areas = data["areas"]
        by_id = {a["id"]: a for a in areas}
        added = moved = 0
        seen = set()
        for name, pos, yaw, guid in points:
            unnamed = name in ("", "invisible_monsterdata")
            label = f"invisible_monsterdata-{guid}" if unnamed else name
            aid = f"area/{pf}/{slug(label)}"
            if aid in seen:  # a second point of that name: its guid tells them apart
                label = f"{label}-{guid}"
                aid = f"area/{pf}/{slug(label)}"
            seen.add(aid)
            a = by_id.get(aid)
            if a is None:
                a = {"id": aid, "name": label, "role": "marker", "shape": {"point": {"pos": pos}}, "source": "client",
                     "comment": f"RDB 1000057:{pf} guid {guid} (invisible_monsterdata)"}
                if abs(yaw) > 1e-4:
                    a["shape"]["point"]["rot"] = round(yaw, 3)
                areas.append(a)
                by_id[aid] = a
                added += 1
            elif a["shape"]["point"]["pos"] != pos:
                a["shape"]["point"]["pos"] = pos
                moved += 1
        with open(path, "w") as f:
            json.dump(data, f, indent=2)
            f.write("\n")
        print(f"{pf}: {len(points)} points, {added} added, {moved} moved")


if __name__ == "__main__":
    main()
