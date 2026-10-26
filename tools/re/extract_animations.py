"""The client's animations -> data/animations.json: motion sets, anim graphs and the animation
attributes effect packages set.

* RDB 1050004 motion sets ("MOS ", byte-packed): u32 motion count at 0x1b, u32 name length at
  0x23 and the name (NUL included), u32 0, then per motion: u32 length + .xsm file path, u32
  length + motion id (`idle`, `ability_1`, `walk`, ...). Monster data (data/monster_data.json)
  names the motion set and anim graph of each look.
* RDB 1050003 anim graphs ("XBSF", EMotion FX blend setups): after the exporter's version and
  date, chunks of u32 id, u32 size, ...: the parameter chunk (403: u32 version, u32 count, then per
  parameter u32 length + name and type-dependent defaults; read here as the length-prefixed names
  in the chunk) comes first if the graph has parameters. The parameters are
  the animation attributes effect packages set (`limbostart`, `chainattack`, ...: the
  emotionfxattribute timelines); the graph's motion nodes name motions by id (`motionID`, u8 1, u32
  length + id).
* attributes: every attribute an effect package sets (data/effect_catalog.json), the graphs that
  have it, the packages that set it, and `package`: the package `!anim <attribute>` plays (one that
  sets only this attribute if there is one, else the smallest that sets it).

  tools/re/py extract_animations.py   (after extract_effect_catalog.py)"""
import json, os, re, struct
from collections import defaultdict

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
MOTION_SETS, ANIM_GRAPHS = 1050004, 1050003


def motion_set(b):
    count = struct.unpack_from("<I", b, 0x1B)[0]
    n = struct.unpack_from("<I", b, 0x23)[0]
    name = b[0x27 : 0x27 + n].rstrip(b"\0").decode("latin1")
    o = 0x27 + n + 4
    motions = []
    for _ in range(count):
        l = struct.unpack_from("<I", b, o)[0]
        path = b[o + 4 : o + 4 + l].rstrip(b"\0").decode("latin1")
        o += 4 + l
        l = struct.unpack_from("<I", b, o)[0]
        motion = b[o + 4 : o + 4 + l].rstrip(b"\0").decode("latin1")
        o += 4 + l
        motions.append({"id": motion, "file": path})
    if o != len(b):
        raise ValueError(f"motion set: {len(b) - o} bytes left over")
    return name, motions


def anim_graph(b):
    if b[:4] != b"XBSF":
        raise ValueError("not XBSF")
    name = re.match(rb"(.)\x00\x00\x00([^\x00]*)\x00", b[24:])
    date = re.search(rb"[A-Z][a-z]{2} [ \d]\d \d{4}\x00", b)
    # the parameter chunk (id 403: u32 size, u32 version, u32 count, then the parameters); graphs
    # without parameters start with another chunk
    chunk, size, _version, count = struct.unpack_from("<4I", b, date.end())
    params = []
    if chunk == 403:
        for m in re.finditer(rb"([\x02-\x40])\x00\x00\x00([A-Za-z_][\w]*)\x00", b[date.end() : date.end() + 8 + size]):
            if m.group(1)[0] == len(m.group(2)) + 1 and len(params) < count:
                params.append(m.group(2).decode())
        if len(params) != count:
            raise ValueError(f"{len(params)} of {count} parameters read")
    motions = sorted(set(m.group(1).decode("latin1") for m in re.finditer(rb"motionID\x00\x01[\x01-\x40]\x00\x00\x00([^\x00]+)\x00", b)))
    return (name.group(2).decode("latin1") if name else ""), params, motions


def main():
    idx = rdb.index()
    sets, graphs, problems = [], [], []
    for t, r in sorted(idx):
        if t == MOTION_SETS:
            try:
                name, motions = motion_set(rdb.body(t, r, idx))
            except (ValueError, struct.error, FileNotFoundError) as e:
                problems.append(f"motion set {r}: {e}")
                continue
            sets.append({"id": r, "name": name, "motions": motions})
        elif t == ANIM_GRAPHS:
            try:
                name, params, motions = anim_graph(rdb.body(t, r, idx))
            except (ValueError, struct.error, FileNotFoundError, AttributeError) as e:
                problems.append(f"anim graph {r}: {e}")
                continue
            graphs.append({"id": r, "name": name, "attributes": params, "motions": motions})
    catalog = json.load(open(os.path.join(ROOT, "data/effect_catalog.json")))["effects"]
    packages = defaultdict(list)
    for e in catalog:
        for a in e.get("animation_attributes", []):
            packages[a].append(e)
    in_graphs = defaultdict(list)
    for g in graphs:
        for a in g["attributes"]:
            in_graphs[a].append(g["id"])
    attributes = []
    for a in sorted(set(packages) | set(in_graphs)):
        ps = packages.get(a, [])
        only = [e for e in ps if e["animation_attributes"] == [a] and all(t["type"] == "emotionfxattribute" for t in e["timelines"])]
        pick = min(only or ps, key=lambda e: (len(e["timelines"]), e["id"]), default=None)
        attributes.append({"name": a, "graphs": in_graphs.get(a, []), "packages": [e["id"] for e in ps], "package": pick and pick["id"]})
    path = os.path.join(ROOT, "data/animations.json")
    with open(path, "w") as f:
        f.write('{\n "_comment": "Motion sets, anim graphs and animation attributes. See tools/re/extract_animations.py.",\n')
        for key, rows in (("attributes", attributes), ("motion_sets", sets), ("anim_graphs", graphs)):
            f.write(f' "{key}": [\n' + ",\n".join("  " + json.dumps(x) for x in rows) + "\n ]" + (",\n" if key != "anim_graphs" else "\n"))
        f.write("}\n")
    print(path, len(sets), "motion sets,", sum(len(s["motions"]) for s in sets), "motions,", len(graphs), "anim graphs,", len(attributes), "attributes")
    for p in problems:
        print(" ", p)


if __name__ == "__main__":
    main()
