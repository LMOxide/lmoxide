"""Encounters (the original server's Scry scripts on bounded areas) -> data/encounters.json.

The scripts themselves aren't in the client; their bounded areas are (RDB 1000053, only the Linux
client ships it: XML with <name>, <scryid>, <heights>, <vertex> x z). The area names say what ran
there (research notes: research/lmoxide-notes/world-events.md):

  arenas     "BossEncounter_Start[_X]", "MiniBossEncounter_Start_X", "EventEncounter_Start_X",
             "<X>_Start" with "<X>_Outside": entering engages the boss; the arena doors (RDB
             1000057 simple dynels "Boss|Event" + "Entrance|Exit" + "VisualOpen|VisualClosed"
             [_X], in data/world_content.json) close behind the player
  ambushes   "*Ambush*" (not counters): entering spawns a wave
  champion   "BA_SpawnChampion" in the side dungeons: where the champion appears

Per playfield: {"arenas": [{name, polygon, heights, doors: {open, closed, exit}}], "ambushes":
[{name, polygon, heights}], "champion": [x, z] or absent}. Polygons are [x, z] lists, heights
[low, high]. Doors without the arena's suffix go to the nearest arena.

Usage: tools/re/py extract_encounters.py"""
import json, math, os, re

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
LINUX = os.path.join(ROOT, "../clients/linux/LEGO Minifigures Online")
ARENA = re.compile(r"^(?:Boss|MiniBoss|Miniboss|Event)Encounter_Start(?:_(\w+))?$|^(\w+?)_Start$")
AMBUSH = re.compile(r"ambush", re.I)


def areas(idx, pf):
    out = []
    if (1000053, pf) not in idx:
        return out
    b = rdb.body(1000053, pf, idx, LINUX).decode("latin1")
    for sec in re.finditer(r"<(boundedareas(?:_\w+)?)>(.*?)</\1>", b, re.S):
        for m in re.finditer(r"<boundedarea (.*?)>(.*?)</boundedarea>", sec.group(2), re.S):
            s = m.group(2)
            name = (re.search(r"<name>\s*(.*?)\s*</name>", s, re.S) or [None, ""])[1]
            heights = (re.search(r"<heights>\s*(.*?)\s*</heights>", s, re.S) or [None, "-1000 1000"])[1]
            poly = [[round(float(v), 2) for v in p.split()[:2]] for p in re.findall(r"<vertex>\s*(.*?)\s*</vertex>", s, re.S)]
            if len(poly) >= 3:
                out.append({"name": name, "polygon": poly, "heights": [float(h) for h in heights.split()[:2]]})
    return out


def centre(poly):
    return [sum(p[0] for p in poly) / len(poly), sum(p[1] for p in poly) / len(poly)]


def main():
    idx = rdb.index(LINUX)
    world = json.load(open(os.path.join(ROOT, "data/world_content.json")))
    out = {}
    for pf_s, content in world.items():
        if not pf_s.isdigit():
            continue
        pf = int(pf_s)
        seen, arenas, ambushes, champion = set(), [], [], None
        # the playfield's area layers (RDB ids pf, pf + 1, pf + 2)
        found = [a for layer in (pf, pf + 1, pf + 2) if str(layer) == pf_s or str(layer) not in world for a in areas(idx, layer)]
        names = {a["name"] for a in found}
        for a in found:
            key = (a["name"], tuple(round(v) for v in centre(a["polygon"])))
            if key in seen:
                continue
            seen.add(key)
            m = ARENA.match(a["name"])
            if m and (m.group(1) is not None or a["name"].lower().startswith(("boss", "miniboss", "event")) or f"{m.group(2)}_Outside" in names):
                suffix = m.group(1) or (m.group(2) if not a["name"].lower().startswith(("boss", "miniboss", "event")) else "")
                arenas.append({"name": a["name"], "suffix": suffix or "", "polygon": a["polygon"], "heights": a["heights"]})
            elif AMBUSH.search(a["name"]) and "counter" not in a["name"].lower():
                ambushes.append({"name": a["name"], "polygon": a["polygon"], "heights": a["heights"]})
            elif a["name"] == "BA_SpawnChampion":
                champion = [round(v, 2) for v in centre(a["polygon"])]
        for arena in arenas:
            arena["doors"] = {"open": [], "closed": [], "exit": []}
        for d in content.get("simple_dynels", []) if arenas else []:
            m = re.match(r"^(?:Boss|Event)(Entrance|Exit)Visual(Open|Closed)(?:_(\w+))?$", d["name"])
            if not m:
                continue
            kind = "exit" if m.group(1) == "Exit" else m.group(2).lower()
            owner = [a for a in arenas if m.group(3) and a["suffix"].lower() == m.group(3).lower()]
            if not owner:
                c = lambda a: centre(a["polygon"])
                owner = [min(arenas, key=lambda a: math.dist(c(a), (d["pos"][0], d["pos"][2])))]
            owner[0]["doors"][kind].append(d["guid"])
        entry = {}
        if arenas:
            entry["arenas"] = arenas
        if ambushes:
            entry["ambushes"] = ambushes
        if champion:
            entry["champion"] = champion
        if entry:
            out[pf_s] = entry
    doc = {"_comment": __doc__.split("\n\n")[0].replace("\n", " "), **out}
    path = os.path.join(ROOT, "data/encounters.json")
    json.dump(doc, open(path, "w"), indent=0)
    print(f"{path}: {sum(len(v.get('arenas', [])) for v in out.values())} arenas, "
          f"{sum(len(v.get('ambushes', [])) for v in out.values())} ambushes, "
          f"{sum('champion' in v for v in out.values())} champion spots in {len(out)} playfields")


main()
