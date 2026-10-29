"""The Lost Creations levels into the content tree (docs/LOST-CREATIONS.md, "Levels").

For each finished level (the user left out 7102, 7103, 7105, 7106, 7401, 7501 on purpose) and the
Treasure Chamber 7001, from the Linux client (tools/re/extract_world_content.py's readers):
- worlds/<world>/spawns.json: the start and exit gates' shields and visuals, the StartButton
  (RDB 1000057 simple dynels) and the exit door (RDB 1000102);
- worlds/<world>/areas.json: the `Start` and `End` bounded areas (RDB 1000053);
- lost-creations/<slug>.json: the level (`lc` record), its mode and quest, the guids, where the
  team arrives;
- world.json `kind`: instance (made per team on entry).
Existing files are replaced. Start positions: the user's for Candy Mayhem and Gatekeeper's Realm,
else a guess beside the button (tagged). Usage: tools/re/py extract_lost_creations.py, then
`cargo run -p lmoxide-content -- fmt`."""
import glob, json, math, os

import extract_world_content as w

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
CONTENT = os.path.join(ROOT, "content")
# playfield -> (mode, main quest)
LEVELS = {
    7100: ("mayhem", 136), 7101: ("mayhem", 136), 7104: ("mayhem", 136),
    7200: ("brawl", 138), 7201: ("brawl", 138), 7202: ("brawl", 138),
    7300: ("champions", 139), 7301: ("champions", 139),
    7400: ("survival", 140), 7500: ("smash", 141), 7600: ("gatekeeper", 142),
}
TREASURE_CHAMBER = 7001
# the user's (2026-10-28): (pos, facing)
STARTS = {7104: ([136.4, 40.0, 137.2], 1.571), 7600: ([204.5, 42.9, 279.3], 1.571)}
GATEKEEPER = {7600: [239.9, 42.2, 277.1]}


def world_dir(pf):
    return next(d for d in glob.glob(os.path.join(CONTENT, "worlds", f"{pf}-*")) if os.path.isdir(d))


def quest_of(main):
    best = None
    for p in sorted(glob.glob(os.path.join(CONTENT, "quests", "*.json"))):
        q = json.load(open(p))
        if q.get("main") == main and "outdated" not in q["id"]:
            best = q["id"]
    return best


def slug_of(d):
    return os.path.basename(d).split("-", 1)[1]


def slug(name):
    out = ""
    for c in name.strip():
        if c.isascii() and c.isalnum():
            out += c.lower()
        elif out and not out.endswith("-"):
            out += "-"
    return out.strip("-") or "unnamed"


def spawn_of(pf, e, kind):
    s = {
        "id": f"spawn/{pf}/{slug(e['name'])}",
        "name": e["name"], "kind": kind, "template": f"object/{slug(e['name'])}",
        "pos": e["pos"], "rot": e["rot"],
    }
    if e.get("scale", 1) != 1:
        s["scale"] = e["scale"]
    client = {"guid": e["guid"], "tag": e.get("tag", "----"), "script_group": e["script_group"], "visual": e["visual"]}
    if "bounds" in e:
        client["bounds"] = e["bounds"]
    s["client"] = client
    s["source"] = "client"
    return s


def area_of(pf, a):
    return {
        "id": f"area/{pf}/{slug(a['name'])}", "name": a["name"], "role": "place",
        "shape": {"polygon": {"points": [[round(x, 2), round(z, 2)] for x, z in a["poly"]], "heights": a["heights"]}},
        "source": "client",
    }


def write(path, data):
    with open(path, "w") as f:
        json.dump(data, f, indent=2)
        f.write("\n")


def objects():
    return {os.path.basename(p)[:-5] for p in glob.glob(os.path.join(CONTENT, "objects", "*.json"))}


def main():
    have = objects()
    for pf in [*LEVELS, TREASURE_CHAMBER]:
        d = world_dir(pf)
        dynels = {e["name"]: e for e in w.simple_dynels(pf) or []}
        doors = w.doors(pf) or []
        spawns, ids = [], set()

        def add(e, kind):
            s = spawn_of(pf, e, kind)
            while s["id"] in ids:
                s["id"] += "-2"
            ids.add(s["id"])
            # the object record only if the tree has one by that name
            if s["template"][len("object/"):] not in have:
                del s["template"]
            spawns.append(s)
            return s

        for door in doors:
            add(door, "door")
        if pf in LEVELS:
            names = ["GateStartShield", "GateStartVisual", "GateExitShield", "GateExitVisual", "GateExitVisualClosed", "StartButton"]
            for n in names:
                if n in dynels:
                    add(dynels[n], "interactable" if n == "StartButton" else "scenery")
        write(os.path.join(d, "spawns.json"), {"spawns": spawns})
        areas = [area_of(pf, a) for a in w.bounded_areas(pf) if a["name"] in ("Start", "End", "BA_TriggerDropLoot", "BA_Teleport")]
        write(os.path.join(d, "areas.json"), {"areas": areas})

        world = json.load(open(os.path.join(d, "world.json")))
        world["kind"] = "instance" if pf in LEVELS else world["kind"]
        write(os.path.join(d, "world.json"), world)
        if pf not in LEVELS:
            continue

        mode, main = LEVELS[pf]
        exit_visual = dynels.get("GateExitVisual") or dynels["GateExitVisualClosed"]
        exit_shield = dynels["GateExitShield"]
        # the Mayhems' exit shield is parked off the map (x 1-2, z 508): nothing blocks
        parked = exit_shield["pos"][2] > 500
        button = dynels["StartButton"]
        if pf in STARTS:
            pos, rot = STARTS[pf]
            start, tags, source = {"pos": pos, "rot": rot}, [], "server"
        else:
            # beside the button, away from the start gate
            g = dynels["GateStartShield"]["pos"]
            dx, dz = button["pos"][0] - g[0], button["pos"][2] - g[2]
            n = math.hypot(dx, dz) or 1
            pos = [round(button["pos"][0] + dx / n * 2, 2), button["pos"][1], round(button["pos"][2] + dz / n * 2, 2)]
            start, tags, source = {"pos": pos, "rot": round(math.atan2(-dx, -dz), 3)}, ["approximate", "todo"], "guess"
        lc = {
            "id": f"lc/{slug_of(d)}", "name": world["name"], "world": world["id"], "mode": mode, "start": start,
            "start_gate": {"shield": dynels["GateStartShield"]["guid"], "visual": dynels["GateStartVisual"]["guid"]},
            "exit_gate": {**({} if parked else {"shield": exit_shield["guid"]}), "visual": exit_visual["guid"],
                          **({"open": True} if mode == "mayhem" else {})},
            "button": button["guid"],
            "exit_door": doors[0]["guid"],
            "quest": quest_of(main),
            "end_area": f"area/{pf}/end",
        }
        if pf in GATEKEEPER:
            lc["gatekeeper"] = {"pos": GATEKEEPER[pf], "rot": -1.571}
        lc["source"] = source
        if tags:
            lc["tags"] = tags
            lc["comment"] = "Start position guessed beside the button (the client has none): check it in game."
        os.makedirs(os.path.join(CONTENT, "lost-creations"), exist_ok=True)
        write(os.path.join(CONTENT, "lost-creations", f"{slug_of(d)}.json"), lc)
        print(pf, mode, lc["id"], len(spawns), "spawns", len(areas), "areas")


if __name__ == "__main__":
    main()
