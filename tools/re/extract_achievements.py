"""Achievements from the client's lore tree (RDB 1000108, protobuf LoreTreeBuf) -> data/achievements.json.

The lore tree holds the achievements (under node 2 "Achievements"), lore and tutorial tags. Each
achievement is a tag the server grants with GameCharInterface::AddTag; its stats give the
points, the reward (AchievementRewardValue stars when RewardIcon is the stars icon 530321), the
minifigure it belongs to (MinifigureTemplateID) and, for counters, the stat counted
(TagCounterStat) and the target (TagCounterValue). What earns the others was server script
(reward/criteria Lua); `kind` is inferred from the names:

  counter   TagCounterStat reaches TagCounterValue; `counts` says what raises it for the
            minifigure item counters (InventoryItemGeneric1/2: kills, smashes or builds while
            the minifigure is active, optionally only of things whose name contains `match`)
  any       "Complete Any N": N of the parent's other achievements
  all       a parent: all its visible children
  upgrade   "<Minifigure>_Upgrade N": the minifigure reaches level N
  series    "Global_Minifigures_Series N" / "_All": every minifigure of the series (all) owned
  defeat    "..._Defeat <name>": a boss of that name defeated
  target    "<Minifigure>_<name>": with that minifigure active, defeat the boss <name> or the
            boss of the dungeon <name>
  manual    no rule (exploration, collectibles, events, Lost Creations, ...)

Usage: tools/re/py extract_achievements.py"""
import json, os, re, sys

import rdb

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "demo"))
from pblib import parse, varint  # noqa: E402

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
STATS = json.load(open(os.path.join(ROOT, "data/stats.json")))
STATS.update(json.load(open(os.path.join(ROOT, "data/stats_linux.json"))))
STAT_ID = {v: int(k) for k, v in STATS.items()}
ACHIEVEMENTS_ROOT = 2
STARS_ICON = 530321
GENERIC1, GENERIC2 = STAT_ID["InventoryItemGeneric1"], STAT_ID["InventoryItemGeneric2"]


def signed(v):
    return v - (1 << 64) if v >= 1 << 63 else v


def node(b):
    n = {"children": [], "stats": {}}
    for f, _w, v in parse(b):
        if f == 1:
            n["id"] = v
        elif f == 2:
            n["parent"] = v
        elif f == 3:
            n["name"] = v.decode("utf-8", "replace")
        elif f == 4 and isinstance(v, (bytes, bytearray)):  # packed
            o = 0
            while o < len(v):
                c, o = varint(v, o)
                n["children"].append(c)
        elif f == 4:
            n["children"].append(v)
        elif f == 8:
            n["invisible"] = bool(v)
        elif f == 9:
            n["type"] = v
        elif f == 10:
            s = {k: x for k, _, x in parse(v)}
            n["stats"][STATS.get(str(s.get(1)), str(s.get(1)))] = signed(s.get(3, 0))
    return n


def singular(word):
    for suffix, repl in (("ches", "ch"), ("ies", "y"), ("s", "")):
        if word.endswith(suffix):
            return word[: -len(suffix)] + repl
    return word


def counts(name, stat):
    """What raises a minifigure item counter, from the achievement's name."""
    m = re.search(r"(Smash|Build) \d+ (.+)$", name)
    if not m:
        return None
    verb, what = m.group(1), m.group(2)
    if stat == GENERIC1:
        kind = {"Monsters": "kills", "Smashies": "smashes"}.get(what, "builds" if verb == "Build" else None)
        return {"what": kind} if kind else None
    # Generic2: a specific kind of enemy, smashable or buildie
    words = what.split()
    words[-1] = singular(words[-1])
    return {"what": "builds" if verb == "Build" else "kills_or_smashes", "match": " ".join(words)}


def main():
    tree = parse(rdb.body(1000108, 0))
    nodes = {}
    for f, _w, v in tree:
        if f == 2:
            n = node(v)
            nodes[n["id"]] = n
    keep = set()
    todo = [ACHIEVEMENTS_ROOT]
    while todo:
        i = todo.pop()
        keep.add(i)
        todo.extend(c for c in nodes[i]["children"] if c in nodes)
    out = []
    for i in sorted(keep - {ACHIEVEMENTS_ROOT}):
        n = nodes[i]
        s = n["stats"]
        name = n.get("name", "")
        parent = nodes.get(n.get("parent", 0), {})
        a = {"id": i, "parent": n.get("parent", 0), "name": name}
        if n.get("invisible"):
            a["invisible"] = True
        if s.get("AchievementPoints", 0) > 0:
            a["points"] = s["AchievementPoints"]
        if s.get("RewardIcon") == STARS_ICON and s.get("AchievementRewardValue", 0) > 0:
            a["stars"] = s["AchievementRewardValue"]
        if s.get("MinifigureTemplateID", 0) > 0:
            a["minifigure"] = s["MinifigureTemplateID"]
        children = [c for c in n["children"] if c in nodes and not nodes[c].get("invisible")]
        if "TagCounterStat" in s:
            a["kind"] = "counter"
            a["counter"] = [s["TagCounterStat"], s.get("TagCounterValue", 1)]
            if s["TagCounterStat"] in (GENERIC1, GENERIC2):
                c = counts(name, s["TagCounterStat"])
                if c:
                    a["counts"] = c
                else:
                    a["kind"] = "manual"
        elif m := re.search(r"Complete Any (\d+)$", name):
            a["kind"], a["n"] = "any", int(m.group(1))
        elif m := re.search(r"_Upgrade (\d+)$", name):
            a["kind"], a["n"] = "upgrade", int(m.group(1))
        elif m := re.match(r"Global_Minifigures_(Series (\d+)|All)$", name):
            a["kind"] = "series"
            if m.group(2):
                a["series"] = int(m.group(2))
        elif n.get("type") == 11 or n.get("type") == 1337:
            a["kind"] = "category"
        elif children:
            a["kind"] = "all"
            a["children"] = children
        elif m := re.search(r"Defeat (?!.*World Champions)(.+)$", name):
            a["kind"], a["target"] = "defeat", m.group(1).strip()
        elif "MinifigureTemplateID" in parent.get("stats", {}) and "_" in name and "Lost Creations" not in name:
            a["kind"], a["target"] = "target", name.split("_", 1)[1].strip()
        else:
            a["kind"] = "manual"
        out.append(a)
    doc = {"_comment": __doc__.split("\n\n")[0].replace("\n", " "), "achievements": out}
    path = os.path.join(ROOT, "data/achievements.json")
    json.dump(doc, open(path, "w"), indent=0)
    kinds = {}
    for a in out:
        kinds[a["kind"]] = kinds.get(a["kind"], 0) + 1
    print(f"{len(out)} achievements -> {path}: {kinds}")


main()
