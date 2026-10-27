"""Gates opened by light (crates/lmoxide-server/src/gates.rs). Writes data/light_gates.json.

Medusa's Lair (2300): a "door light" marker stands in front of three LEGO gates
(`module_wall_4x4_gate_lego_myth_interactable`), and "mirror light" / "mirror N light" markers
face it from about 10 m away. Live (the user remembers) had the player build a mirror there
(item "XX_boss medusa mirror buildie" 489061) that beamed light into the gate and opened it.
A gate with two mirror spots needs both. The markers are client data (RDB 1000057); the build,
the beam and the opening were server script and are inferred (docs/TUNABLES.md "Gates").

Usage: tools/re/py extract_light_gates.py"""
import json, math, os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
GATE = "module_wall_4x4_gate_lego_myth_interactable"
# a door light belongs to the gate within this distance (m)
GATE_DISTANCE = 2.5
# a mirror spot must face its door light this closely (cos of the angle) and lie this near (m)
FACING = 0.98
MIRROR_DISTANCE = 20.0
# the mirror buildie stands on the floor this far below its light marker (the gates' floors)
MARKER_HEIGHT = 1.8


def yaw(q):
    x, y, z, w = q
    return math.atan2(2 * (w * y + x * z), 1 - 2 * (y * y + x * x))


def flat(a, b):
    return math.dist((a[0], a[2]), (b[0], b[2]))


def main():
    worlds = json.load(open(os.path.join(ROOT, "data/archive/world_content.json")))
    out = {"_comment": __doc__.split("\n\n")[0]}
    for pf, w in worlds.items():
        if not isinstance(w, dict):
            continue
        items = w.get("simple_dynels", [])
        gates = [d for d in items if d["name"] == GATE]
        lights = [d for d in items if d["name"] == "door light"]
        spots = [d for d in items if d["name"].startswith("mirror") and d["name"].endswith("light")]
        entries = []
        for g in gates:
            door = next((l for l in lights if flat(l["pos"], g["pos"]) < GATE_DISTANCE), None)
            if not door:
                continue
            mirrors = []
            for s in spots:
                a = yaw(s["rot"])
                dx, dz = door["pos"][0] - s["pos"][0], door["pos"][2] - s["pos"][2]
                length = math.hypot(dx, dz)
                if length < MIRROR_DISTANCE and (dx * math.sin(a) + dz * math.cos(a)) / length > FACING:
                    floor = [s["pos"][0], round(s["pos"][1] - MARKER_HEIGHT, 2), s["pos"][2]]
                    mirrors.append({"guid": s["guid"], "pos": floor, "light": s["pos"], "rot": s["rot"], "length": round(length, 2)})
            if mirrors:
                entries.append({"gate": g["guid"], "pos": g["pos"], "mirrors": mirrors})
        if entries:
            out[pf] = entries
            print(pf, [(e["gate"], len(e["mirrors"])) for e in entries])
    path = os.path.join(ROOT, "data/archive/light_gates.json")
    json.dump(out, open(path, "w"), indent=1)
    print("wrote", path)


if __name__ == "__main__":
    main()
