"""Medusa mirror puzzles (crates/lmoxide-server/src/mirrors.rs). Writes data/mirror_puzzles.json.

Every `mirror_medusa_c_interactable` (playfield simple dynels, RDB 1000057) has two invisible
markers beside it, "mirror <name> +" and "mirror <name> -", whose facing is where the light
leaves the mirror in each pose (the one 45 degrees from the mirror's facing for the open pose,
225 for the placed one) (the two point opposite ways along one diagonal, so the
light comes in along the other). Following each marker's facing to the first mirror or target
in line gives the puzzle: in the Armory (2310) one chain of settings carries the light from
mirror "1" to "mirror L.2.1.1 complete" on the pedestal by the Thunderbolt arena's exit; in the
Gauntlet (6300) mirrors L and R each pass it on to two invisible markers by the gate
(Quest_MirrorRoom / Quest_MirrorRoomComplete areas). What the light looked like and what solving
did was server script; the targets and rewards here are inferred (docs/TUNABLES.md).

Usage: tools/re/py extract_mirror_puzzles.py"""
import json, math, os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
MIRROR = "mirror_medusa_c_interactable"
# how far a target may lie off a marker's line (m)
IN_LINE = 1.5
# how far light that meets no mirror or target is shown going (m)
STRAY = 4.0

# per playfield: the mirrors the light first falls on (the Armory's L.2.1.1 can send it back to
# "1", so the roots aren't simply the mirrors nothing lights), the light's targets (a marker's
# name or a position) and what lighting all of them opens: an arena's exit (encounter.rs; the
# Armory's pedestal stands by the Thunderbolt arena's exit barrier) or a gate (scenery.rs)
PUZZLES = {
    "2310": {"roots": ["1"], "targets": {"pedestal": "mirror L.2.1.1 complete"}, "opens": {"arena_exit": "Thunderbolt_Start"}},
    "6300": {"roots": ["L", "R"], "targets": {"gate north": [158.6, 49.8, 170.0], "gate south": [158.6, 49.8, 174.0]}, "opens": {"gate": 517927}},
}


def yaw(q):
    x, y, z, w = q
    return math.atan2(2 * (w * y + x * z), 1 - 2 * (y * y + x * x))


def main():
    worlds = json.load(open(os.path.join(ROOT, "data/archive/world_content.json")))
    out = {"_comment": __doc__.split("\n\n")[0]}
    for pf, spec in PUZZLES.items():
        items = worlds[pf]["simple_dynels"]
        mirrors = [d for d in items if d["name"] == MIRROR]
        markers = {d["name"]: d for d in items if d["name"].startswith("mirror ")}
        named = {}
        for n, d in markers.items():
            if n.endswith((" +", " -")):
                m = min(mirrors, key=lambda m: math.dist(m["pos"], d["pos"]))
                named[n[7:-2]] = m
        targets = {k: (markers[v]["pos"] if isinstance(v, str) else v) for k, v in spec["targets"].items()}
        spots = {k: m["pos"] for k, m in named.items()} | targets
        entries, lit_by = [], set()
        for name, m in sorted(named.items()):
            outs, beams = {}, {}
            # the marker 45 degrees from the mirror's facing is where its open pose (anim_1, sent
            # as "plus") reflects; the one at 225 is the placed pose's (anim_0, "minus"). The
            # editor's "+"/"-" names don't follow that (the user saw half the mirrors reflect the
            # other way), so the names are matched to the poses here.
            plus_sign = "+" if (yaw(markers[f"mirror {name} +"]["rot"]) - yaw(m["rot"])) % (2 * math.pi) < math.pi else "-"
            minus_sign = "-" if plus_sign == "+" else "+"
            for state, sign in (("plus", plus_sign), ("minus", minus_sign)):
                d = markers[f"mirror {name} {sign}"]
                a = yaw(d["rot"])
                v = (math.sin(a), math.cos(a))
                best = None
                for k, p in spots.items():
                    if k == name:
                        continue
                    dx, dz = p[0] - m["pos"][0], p[2] - m["pos"][2]
                    t = dx * v[0] + dz * v[1]
                    off = abs(dx * v[1] - dz * v[0])
                    if t > 0 and off < IN_LINE and (best is None or t < best[1]):
                        best = (k, t)
                outs[state] = best[0] if best else None
                # where the light leaves in this position: the marker's place and facing, and
                # how far it goes (to the next mirror or target, else STRAY m into the room)
                beams[state] = {"pos": d["pos"], "rot": d["rot"], "length": round(best[1], 2) if best else STRAY}
                if best:
                    lit_by.add(best[0])
            entries.append({"guid": m["guid"], "name": name, "pos": m["pos"], "out": outs, "beam": beams})
        out[pf] = {
            "mirrors": entries,
            "roots": spec["roots"],
            "targets": [{"name": k, "pos": p} for k, p in targets.items()],
            "opens": spec["opens"],
        }
        print(pf, len(entries), "mirrors, unlit by others:", sorted(set(named) - lit_by))
    path = os.path.join(ROOT, "data/archive/mirror_puzzles.json")
    json.dump(out, open(path, "w"), indent=1)
    print("wrote", path)


if __name__ == "__main__":
    main()
