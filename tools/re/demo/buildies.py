"""Buildies of the recorded session -> data/buildies.json: every DestructibleFullUpdateIIR_c in a
CARS group other than 3 (enemy smashables), with what the server sent for it: position and
rotation, name, template, VisualData (monsterdata, particles, bounds), the CoreDestructible and
Destructible fields, and the visual it switched to once built (SimpleDynelVisualChangeIIR_c
state 4). Needs recs.pkl (see README).  Usage: python3 buildies.py <recs dir>"""
import json, os, pickle, struct, sys

from pblib import parse

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../..")
recs = pickle.load(open(os.path.join(sys.argv[1], "recs.pkl"), "rb"))


def fields(b):
    out = {}
    for f, w, v in parse(b):
        out.setdefault(f, v)
    return out


def floats(b):
    """ProtoVec3 / ProtoQuat: fixed32 floats by field number."""
    return {f: struct.unpack("<f", v)[0] for f, w, v in parse(b) if w == 5}


def decode(body):
    o = 0
    n = struct.unpack_from(">I", body, o)[0]
    dynel = fields(body[o + 4:o + 4 + n])
    o += 4 + n
    n = struct.unpack_from(">I", body, o)[0]
    simple = fields(body[o + 4:o + 4 + n])
    o += 4 + n
    u32 = lambda: struct.unpack_from(">I", body, o)[0]
    res_id, res_type, count = struct.unpack_from(">3I", body, o)
    o += 12
    particles = [list(struct.unpack_from(">5I", body, o + 20 * i)) for i in range(count)]
    o += 20 * count
    bounds = list(struct.unpack_from(">6f", body, o))
    o += 24
    scale, vu32 = struct.unpack_from(">fI", body, o)
    o += 9  # + u8
    health, max_health, df, side, lk, li = struct.unpack_from(">IIfIII", body, o)
    o += 24
    tail = list(struct.unpack_from(">IffIf", body, o))
    o += 20
    assert o == len(body), (o, len(body))
    pos, rot = floats(dynel[1]), floats(dynel[2])
    return dict(
        name=simple[2].decode(), display_name=simple.get(3, b"").decode(), template=simple.get(8, 0), cars_group=simple.get(5, 0),
        # exact float32 values (the server re-sends them unchanged)
        playfield=dynel.get(3), pos=[pos.get(i, 0.0) for i in (1, 2, 3)],
        rot=[rot.get(i, 0.0) for i in (1, 2, 3, 4)],
        monster_data=res_id, visual_type=res_type, particles=particles, bounds=bounds, scale=scale,
        health=health, max=max_health, destructible=[df, side], tail=tail,
    )


# the dynel's playfield field is the instance; the playfield comes from world_content.json, where
# extract_world_content.py placed the same recorded buildies
world = json.load(open(os.path.join(ROOT, "data/world_content.json")))
where = {(b["name"], tuple(b["pos"])): int(pf) for pf, w in world.items() if isinstance(w, dict) for b in w.get("buildies", [])}

out, seen = [], set()
for x in recs:
    if x["name"] != "DestructibleFullUpdateIIR_c" or x["id"] in seen:
        continue
    d = decode(x["body"])
    if d["cars_group"] == 3:
        continue
    seen.add(x["id"])
    built = next((struct.unpack(">II", y["body"])[1] for y in recs
                  if y["id"] == x["id"] and y["name"] == "SimpleDynelVisualChangeIIR_c" and y["body"][:4] == b"\0\0\0\4"), None)
    d["built_visual"] = built
    d["playfield"] = where[(d["name"], tuple(round(v, 2) for v in d["pos"]))]
    out.append(d)
path = os.path.join(ROOT, "data/buildies.json")
open(path, "w").write("[\n" + ",\n".join(" " + json.dumps(d) for d in out) + "\n]\n")
print(len(out), "buildies ->", path)
for d in out:
    print(d["playfield"], d["name"], d["pos"], d["max"], d["built_visual"])
