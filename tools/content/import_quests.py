#!/usr/bin/env python3
"""Make the content tree's quests (content/quests/*.json) and quest areas (role `quest` in the
worlds' areas.json) from the client's quest data and the live capture (one-off import, like
`lmoxide-content import`; run `cargo run -p lmoxide-content -- fmt` afterwards).

Inputs:
  - questlist.json: the client's RDB 1000104 QuestGiverList with its TDB texts
    (research/lmoxide-notes/questlist.py),
  - the client's `Quest_*` bounded areas (RDB 1000053),
  - what LMO_Captures.cap showed (LIVE below; research/lmoxide-notes/quests.md).

Which area runs which area quest is not in the client: AREAS below matches them by name
(source `guess`, except those the capture confirmed). What a goal counts is guessed from its
text (`goal_rule`); the count is the number in the text, else 3 (user's rule, 2026-10-28). A
goal nothing counts whose quest area `Quest_<X>` has a `Quest_<X>Complete` area gets that as its
`area`: entering it completes the goal (quests.rs; the Tar Pit's nest, the Gauntlet's room exits;
guess by name).

  python3 tools/content/import_quests.py ../research/lmoxide-notes/questlist.json [content]
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "re"))
import rdb  # noqa: E402

LINUX = os.path.join(HERE, "..", "..", "..", "clients/linux/LEGO Minifigures Online")


def bounded_areas(idx, pf):
    """The bounded areas of playfield `pf` (RDB 1000053), as tools/re/extract_encounters.py
    reads them (that module writes its output when imported)."""
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

# "[<prefix>] name" -> playfield
PREFIX_WORLD = {
    "Pirate 1": 4200,
    "Pirate 1 - Isle of Yarr": 4200,
    "Pirate 1 - Trials": 4300,
    "Pirate 2": 4210,
    "Pirate 2 - Grotto": 4310,
    "Pirate 2 - Treasure Island": 4500,
    "Medieval 1 - Kingdom": 5200,
    "Medieval 1 - Ruined Fort": 5300,
    "Medieval 1 - Dragon Halls": 5500,
    "Medieval 2 - Enchanted Forest": 5210,
    "Medieval 2 - Evil Castle": 5310,
    "Space 1 - Space Colony": 3200,
    "Space 1 - Police HQ": 3300,
    "Space 2 - Alien World": 3210,
    "Space 2 - Alien HQ": 3500,
    "Space 2 - Mercenary Base": 3310,
    "Myth 2 - Mountains": 2210,
    "Myth 2 - Forge": 2210,
    "Myth 2 - Armory": 2310,
    "Myth 2 - Underworld": 2320,
    "Myth 2 - Medusas Lair": 2300,
    "Dino 1 - Outdoor": 6200,
    "Dino 1 - Gauntlet": 6300,
    "Holiday Winter 2014": 1410,
    "Tutorial": 1300,
}
STORY_WORLD = {45: 4200, 84: 5200, 108: 3200, 17: 2210, 122: 6200}

# main quest -> its Quest_* area (the playfield's; side dungeon champions: Quest_Micro)
AREAS = {
    # Isle of Yarr
    39: "Quest_Farm", 40: "Quest_Fort", 41: "Quest_Native", 42: "Quest_Cave",
    43: "Quest_TreasureHunt", 114: "Quest_Scientist", 179: "Quest_Beach",
    # Volcano Island
    62: "Quest_Beach", 49: "Quest_Native", 50: "Quest_PirateFort", 52: "Quest_ScorchedCity",
    # Pirate Grotto
    53: "Quest_LeftHand", 54: "Quest_LoadingBay", 55: "Quest_Prison", 56: "Quest_Boss",
    # Treasure Island
    58: "Quest_NativeBeach", 59: "Quest_KrakenWorship", 60: "Quest_Piratebay", 61: "Quest_Boss",
    # Pirate Trials
    35: "Quest_CannonShooting1", 37: "Quest_CannonTraining2", 112: "Quest_CannonDodge1",
    113: "Quest_CannonDodge2",
    # Kingdom
    73: "Quest_Siege", 74: "Quest_DragonHill", 75: "Quest_TrollCamp", 76: "Quest_CandyWitch",
    77: "Quest_MountainClimb",
    # Ruined Fort
    78: "Quest_Storage", 79: "Quest_Armoury", 80: "Quest_Graveyard", 81: "Quest_Boss",
    # Sorcerer's Keep
    82: "Quest_Boss", 83: "Quest_Courtyard", 88: "Quest_SecretLab",
    # Dragon Halls
    65: "Quest_Arena", 66: "Quest_Ritual", 67: "Quest_Boss", 85: "Quest_Guardroom",
    # Enchanted Forest
    64: "Quest_DarkSpiderForest", 70: "Quest_Quarry", 71: "Quest_SpiderForest",
    72: "Quest_TrollVillage", 86: "Quest_RedRidingHood",
    # Space Colony
    90: "Quest_Mercs", 91: "Quest_Biodome", 92: "Quest_CrystalMine", 93: "Quest_SpaceTown",
    109: "Quest_Colonists",
    # Planet's Dark Side
    94: "Quest_Swamp", 95: "Quest_Ruins", 96: "Quest_AlienBase", 97: "Quest_Refinery",
    # Space Command HQ
    98: "Quest_Quarters", 99: "Quest_Platform", 100: "Quest_Garage", 111: "Quest_Boss",
    # Shadow Centurion Base
    105: "Quest_Port", 106: "Quest_Research", 107: "Quest_Boss",
    # Alien Command Center
    89: "Quest_Overlord", 101: "Quest_CrystalLab", 102: "Quest_UFO", 103: "Quest_FirstEncounter",
    110: "Quest_Boss",
    # Mythology Mountains, Armory, Underworld, Medusa's Lair
    21: "Quest_Ogre_Mountains", 25: "Quest_Harpy", 118: "Quest_Satyr",
    119: "Quest_SawEvent", 120: "Quest_Boss", 121: "Quest_Witches", 116: "Quest_Medusa",
    # Dinosaur Rise, Gauntlet
    123: "Quest_Swamp", 124: "Quest_Volcano", 125: "Quest_Pond", 126: "Quest_Tarpit",
    127: "Quest_BuzzRoom", 128: "Quest_TikiRoom", 129: "Quest_MirrorRoom",
    130: "Quest_ShootingRoom", 131: "Quest_DodgeRoom", 132: "Quest_MazeRoom",
    133: "Quest_BossRoom", 134: "Quest_PressurePlateRoom",
    # Winter Pass
    176: "Quest_Crunch",
}

# What LMO_Captures.cap showed (research/lmoxide-notes/quests.md)
LIVE_AREAS = {41, 179, 123, 144, 147}
# goal -> count, bar maximum (QuestGoalIIR steps), marker (x, z, radius, y)
LIVE_GOALS = {
    93: dict(count=8, max=100, marker=(176.8, 398.3, 20.0, 74.0)),
    341: dict(count=5, max=100, marker=(98.0, 85.0, 15.0, 70.0)),
    259: dict(max=100, marker=(139.0, 111.0, 60.0, 61.0)),
    298: dict(count=1, max=1),
    295: dict(count=1, max=1),
    343: dict(count=25, max=25),
    351: dict(count=9, max=90),
    352: dict(count=3, max=100),
    353: dict(marker=(140.1, 321.4, 1.0, 56.0)),
    # story goals
    116: dict(marker=(63.5, 15.5, 5.0, 70.9)),
    117: dict(marker=(140.4, 107.5, 10.0, 64.0)),
    220: dict(marker=(230.6, 258.0, 5.0, 16.0)),
    221: dict(marker=(329.0, 273.9, 10.0, 18.6)),
    118: dict(marker=(176.8, 398.3, 5.0, 74.6)),
    332: dict(marker=(140.4, 107.5, 10.0, 64.0)),
}
# goal -> marker (x, z, radius, y) not from the capture (guesses)
GOAL_MARKERS = {
    261: (271.8, 192.7, 5.0, 86.6),  # Return the egg to the nest: at the nest's eggs (dino_egg_1/2)
}
LIVE_GOAL_KIND = {93: "trigger", 341: "trigger", 259: "trigger", 298: "defeat", 295: "defeat",
                  343: "defeat", 351: "trigger", 352: "trigger", 353: "trigger", 356: "defeat",
                  346: "trigger", 330: "defeat", 116: "locate", 117: "locate", 229: "locate",
                  228: "trigger", 220: "destroy", 118: "trigger", 332: "trigger", 221: "trigger"}
# live cooldowns: area quest 41 3600 s, side dungeon champion 147 82800 s
COOLDOWN_AREA = "1h"
COOLDOWN_CHAMPION = "23h"
# the Pirate story line's order (MainQuestUpdateIIR_c: a countdown, 21 first)
PIRATE_ORDER = {279: 21, 78: 20, 186: 19, 79: 18, 185: 17, 177: 16, 80: 15, 178: 14, 281: 13,
                282: 12, 286: 11, 287: 10, 280: 9, 81: 8, 179: 7, 82: 6, 85: 5, 183: 4, 181: 3,
                83: 2, 86: 1, 87: 0}

NUMBERS = {"two": 2, "three": 3, "four": 4, "five": 5, "six": 6, "seven": 7, "eight": 8,
           "nine": 9, "ten": 10}
PLACEHOLDER = re.compile(r"^<write the task .* here>$|^$")


def text(s):
    s = (s or "").strip()
    return None if PLACEHOLDER.match(s) else s


def number_in(s):
    for w in re.findall(r"[A-Za-z]+|\d+", s or ""):
        if w.isdigit():
            return int(w)
        if w.lower() in NUMBERS:
            return NUMBERS[w.lower()]
    return None


def goal_rule(goal_text, champion):
    """(kind, counts, enemies, count, max) guessed from an area quest goal's text."""
    t = goal_text or ""
    low = t.lower()
    n = number_in(t)
    if champion or "champion" in low:
        return "defeat", "champions", [], n or 1, None if not n else 100
    m = re.match(r"(?:defeat|best|destroy) (?:the )?(.+?)(?:,.*| in a .*)?$", t, re.I)
    named = m and m.group(1)[:1].isupper() and not re.search(r"\b(and|guardians|forces|crew|invaders|enemies)\b", m.group(1))
    if named and n is None:
        return "defeat", "kills", [m.group(1).strip()], 1, None
    if re.match(r"(smash|destroy|wreak|mess up|sabotage|free|rescue)\b", low):
        return "trigger", "smashes", [], n or 3, 100
    if re.match(r"(weaken|cripple)\b", low):
        return "trigger", "kills_or_smashes", [], n or 3, 100
    if re.match(r"(collect|gather|clean up|help)\b", low):
        return "trigger", "kills_or_smashes", [], n or 3, 100
    if re.match(r"(rebuild|build)\b", low):
        return "trigger", "builds", [], n or 3, 100
    if re.match(r"(defeat|fight|push back|disrupt|fend off|beat|survive|clear|cleanse|diminish|assault|ruffle|stop|take control|prove|make|break)\b", low):
        return "trigger", "kills", [], n or 3, 100
    # puzzles, courses, chests: nothing the server counts yet
    return "trigger", None, [], n or 1, None


def story_goal_kind(t):
    low = (t or "").lower()
    if re.match(r"(talk|ask|report|confront|return to|find someone|recruit)\b", low):
        return "talk"
    if re.match(r"(travel|go to|reach|find|sail|take the|board|enter|jump|follow|use the|pass|search|sneak|explore|investigate)\b", low):
        return "locate"
    if re.match(r"(defeat|find and face)\b", low):
        return "defeat"
    return "trigger"


def slug(name):
    out = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
    return out or "unnamed"


def main():
    questlist = json.load(open(sys.argv[1]))
    content = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "..", "..", "content")
    worlds = {}
    for d in os.listdir(os.path.join(content, "worlds")):
        w = json.load(open(os.path.join(content, "worlds", d, "world.json")))
        worlds[int(d.split("-")[0])] = (d, w["id"], w.get("display_name") or w.get("name"))
    by_name = {slug(v[2] or ""): pf for pf, v in worlds.items()}

    # the Quest_* areas of every world in the tree
    idx = rdb.index(LINUX)
    quest_areas = {}
    for pf, (d, wid, _) in sorted(worlds.items()):
        found = [a for a in bounded_areas(idx, pf) if a["name"].startswith("Quest_")]
        if not found:
            continue
        path = os.path.join(content, "worlds", d, "areas.json")
        data = json.load(open(path)) if os.path.exists(path) else {"areas": []}
        ids = {a["id"] for a in data["areas"]}
        for a in found:
            aid = f"area/{pf}/{slug(a['name'])}"
            quest_areas[(pf, a["name"])] = (aid, a)
            if aid in ids:
                continue
            ids.add(aid)
            data["areas"].append({"id": aid, "name": a["name"], "role": "quest",
                                  "shape": {"polygon": {"points": a["polygon"], "heights": a["heights"]}},
                                  "source": "client"})
        with open(path, "w") as fh:
            json.dump(data, fh, indent=2)
            fh.write("\n")

    out_dir = os.path.join(content, "quests")
    os.makedirs(out_dir, exist_ok=True)
    used = set()
    for m in questlist:
        name = m["name"].strip()
        prefix_m = re.match(r"\[\s*([^\]]*?)\s*\]\s*(.*)", name)
        prefix, title = (prefix_m.group(1), prefix_m.group(2)) if prefix_m else ("", name)
        test = m["type"] in (0, 3, 4) or re.match(r"(dev|test|temp|\s*test)", prefix, re.I) or prefix.startswith("temp")
        story = m["type"] == 5
        kind = "story" if story else "test" if test else "area"
        rec = {"id": None, "name": title or name, "main": m["main"], "hash": m["hash"], "client_name": name,
               "kind": kind}
        tags = []
        # its world
        pf = STORY_WORLD.get(m["main"]) if story else None
        champion = " - PA - " in f" {prefix} "
        if champion:
            place = prefix.split(" - PA - ", 1)[1]
            pf = by_name.get(slug(place)) or by_name.get(slug(place.replace("Spiders", "Spider's")))
        elif pf is None:
            pf = PREFIX_WORLD.get(prefix)
        if pf in worlds:
            rec["world"] = worlds[pf][1]
        # its area
        area_name = "Quest_Micro" if champion else AREAS.get(m["main"])
        if kind == "area" and area_name and (pf, area_name) in quest_areas:
            rec["area"] = quest_areas[(pf, area_name)][0]
        # live repeated area quests after a cooldown (QuestCooldownIIR at completion); the
        # client's repeat_count doesn't say (0 for the Dino quest the capture shows)
        if kind == "area":
            rec["repeatable"] = True
            rec["cooldown"] = COOLDOWN_CHAMPION if champion else COOLDOWN_AREA
        if m.get("cooldown"):
            rec["cooldown_id"] = m["cooldown"]
        if m.get("rewards"):
            rec["rewards"] = [list(r) for r in m["rewards"]]
        # steps
        quests = m["quests"]
        if m["main"] == 45:
            quests = sorted(quests, key=lambda q: -PIRATE_ORDER.get(q["id"], -1))
        area = quest_areas.get((pf, area_name)) if rec.get("area") else None
        steps = []
        for q in quests:
            goals = []
            for g in q["goals"]:
                gt = text(g["text"]) or text(g["name"])
                goal = {"goal": g["id"], "hash": g["hash"]}
                if gt and gt != "<auto-generate>":
                    goal["text"] = gt
                if kind == "story":
                    goal["kind"] = story_goal_kind(gt)
                else:
                    gk, counts, enemies, count, bar = goal_rule(gt, champion)
                    goal["kind"] = gk
                    if counts:
                        goal["counts"] = counts
                    if enemies:
                        goal["enemies"] = enemies
                    if count != 1:
                        goal["count"] = count
                    if bar:
                        goal["max"] = bar
                    if area:
                        a = area[1]
                        xs = [p[0] for p in a["polygon"]]
                        zs = [p[1] for p in a["polygon"]]
                        c = [sum(xs) / len(xs), sum(zs) / len(zs)]
                        r = max(((p[0] - c[0]) ** 2 + (p[1] - c[1]) ** 2) ** 0.5 for p in a["polygon"])
                        goal["marker"] = {"pos": [round(c[0], 1), round(a["heights"][0], 1), round(c[1], 1)],
                                          "radius": round(min(r, 60.0), 1)}
                done_in = quest_areas.get((pf, f"{area_name}Complete")) if area else None
                if done_in and not goal.get("counts") and len(quests) == 1 and len(q["goals"]) == 1:
                    goal["area"] = done_in[0]
                if g["id"] in GOAL_MARKERS:
                    x, z, r, y = GOAL_MARKERS[g["id"]]
                    goal["marker"] = {"pos": [x, y, z], "radius": r}
                live = LIVE_GOALS.get(g["id"])
                if g["id"] in LIVE_GOAL_KIND:
                    goal["kind"] = LIVE_GOAL_KIND[g["id"]]
                if live:
                    if "count" in live:
                        goal["count"] = live["count"]
                    if "max" in live:
                        goal["max"] = live["max"]
                    if "marker" in live:
                        x, z, r, y = live["marker"]
                        goal["marker"] = {"pos": [x, y, z], "radius": r}
                if goal.get("max") == goal.get("count", 1):
                    goal.pop("max", None)
                if goal.get("count") == 1:
                    goal.pop("count", None)
                goals.append(goal)
            step = {"quest": q["id"], "hash": q["hash"]}
            if text(q.get("task")) and text(q["task"]) != title:
                step["task"] = text(q["task"])
            if text(q.get("desc")) and text(q["desc"]) != text(q.get("task")):
                step["description"] = text(q["desc"])
            step["goals"] = goals
            steps.append(step)
        rec["steps"] = steps
        # where the values came from
        if kind == "area" and rec.get("area"):
            rec["source"] = "live" if m["main"] in LIVE_AREAS else "guess"
            if m["main"] not in LIVE_AREAS:
                tags.append("approximate")
        else:
            rec["source"] = "client"
        if story and m["main"] != 45:
            tags.append("todo")
            rec["comment"] = "Story order not known (only the Pirate World's was in the capture): the client's order."
        elif kind == "area" and not rec.get("area"):
            tags.append("todo")
            rec["comment"] = "No Quest_* area matched: not started by the server."
        if kind == "area" and any(not g.get("counts") and not g.get("area") for s in steps for g in s["goals"]):
            tags.append("todo")
        if tags:
            rec["tags"] = sorted(set(tags))
        s = slug(f"{prefix} {title}" if prefix else title)
        if kind == "story":
            s = slug(title.replace("World", "") + " story")
        base, n = s, 2
        while s in used:
            s, n = f"{base}-{n}", n + 1
        used.add(s)
        rec["id"] = f"quest/{s}"
        with open(os.path.join(out_dir, f"{s}.json"), "w") as fh:
            json.dump(rec, fh, indent=2)
            fh.write("\n")
    print(f"{len(used)} quests, {len(quest_areas)} quest areas")


if __name__ == "__main__":
    main()
