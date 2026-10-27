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
  any       "Complete Any N": N of the parent's other achievements (N from the description,
            text category 55700: "Complete Any 10" says "Complete any 11")
  all       a parent: all its visible children
  upgrade   "<Minifigure>_Upgrade N": "Get the Nth upgrade": N upgrades bought for the minifigure
  lost_creations  "<Minifigure>_Lost Creations_Get to Level N": level N of the Lost Creations
            completed with that minifigure
  series    "Global_Minifigures_Series N" / "_All": every minifigure of the series (all) owned
  defeat    "..._Defeat <name>": a boss of that name defeated
  target    "<Minifigure>_<name>": with that minifigure active, defeat the boss <name> or the
            boss of the dungeon <name>
  defeat    also "..._Combat_Champion_<name>": a champion (or boss) of that name defeated
  clear     "..._Combat_PA_[Clear the ]<area>": every enemy of the dungeon <area> defeated in
            one visit
  find      "..._Collectibles_<x>" / "[<world>] Boombox": the world object `find` (in
            `playfield`, when given) found: collectibles are clicked, boomboxes walked up to;
            exploration ("..._Exploration_<x>", EXPLORE): the bounded area `find` walked into
  manual    no rule (some exploration, events, global Lost Creations, ...)

Every achievement also gets the client's own `title` (text category 55701, "Fiery Explorer")
and `text` (55700, "Find the Volcano Summit in Dinosaur Rise."); `find` achievements get the
`place` a player would call what they find ("Volcano Summit"), from the text when it says
"Find/Discover the X in Y", else from PLACES.

Usage: tools/re/py extract_achievements.py"""
import json, os, re, sys

import rdb
import tdc

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "demo"))
from pblib import parse, varint  # noqa: E402

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
STATS = json.load(open(os.path.join(ROOT, "data/stats.json")))
STATS.update(json.load(open(os.path.join(ROOT, "data/stats_linux.json"))))
STAT_ID = {v: int(k) for k, v in STATS.items()}
ACHIEVEMENTS_ROOT = 2
# TDB categories of the achievements' descriptions and titles (keyed by tag id)
ACHIEVEMENT_TEXTS = 55700
ACHIEVEMENT_TITLES = 55701
PLAYFIELD_NAMES = 52000
STARS_ICON = 530321
GENERIC1, GENERIC2 = STAT_ID["InventoryItemGeneric1"], STAT_ID["InventoryItemGeneric2"]


# "..._Collectibles_<x>" -> its world object (RDB 1000057 simple dynel, data/world_content.json)
COLLECTIBLES = {
    "Dreadleg's Treasure": "collectible_dreadlegs_treasure",
    "T-Rex Footprint": "collectible_trex_foot",
    "Queen Flora's Crystal Pouch": "collectible_queens_crystal_pouch",
    "Shadrack's Diary": "collectible_shadrack_secret_diary",
    "All-seeing Eye": "collectible_all_seeing_eye",
}
# "[<world>] Boombox" -> the playfield with that world's "boombox 1" marker
BOOMBOX_WORLDS = {
    "Isle of Yarr": 4200,
    "Volcano Island": 4210,
    "Kingdom": 5200,
    "Enchanted Forest": 5210,
    "Space Colony": 3200,
    "Planet's Dark Side": 3210,
    "Mythology Mountains": 2210,
    "Dino World": 6200,
}


# exploration achievements -> the bounded area (RDB 1000053) the player walks into, by playfield.
# Matched by name; "Lost Pirates"/"Beauty Salon" (Dino) are guesses between the swamp and tar pit.
EXPLORE = {
    "Pirate_Exploration_The Sirens": (4200, "Achievement_Mermaid"),
    "Pirate_Story_Escape Jacket Fort": (4200, "Achievement_EscapeFort"),
    "Pirate_Exploration_Find the Hidden Pirate Town": (4200, "Achievement_PirateTown"),
    "Pirate_Exploration_Cross the Lava River": (4210, "Achievement_LavaRiver"),
    "Pirate_Exploration_Find Blackbird's Ship": (4200, "Achievement_Ship"),
    "Pirate_Exploration_Find the Sunken Ship": (4200, "Achievement_SunkenShip"),
    "Pirate_Exploration_Find Gunslinger Joe": (4200, "Achievement_Gunslinger"),
    "Pirate_Exploration_Discover the Cursed Treasure": (4210, "Achievement_CursedTreasure"),
    "Pirate_Exploration_Climb the Volcano": (4210, "Achievement_Volcano"),
    "Medieval_Exploration_Center of Attention": (5200, "Achievement_CenterofAttention"),
    "Medieval_Exploration_Watcher on the Wall": (5200, "Achievement_Wall"),
    "Medieval_Exploration_Overlook the Medieval World": (5200, "Achievement_Overlook"),
    "Medieval_Exploration_Take the Web Shortcut": (5210, "Achievement_Shortcut"),
    "Medieval_Exploration_Find the Ominous Cauldron": (5210, "Achievement_Cauldron"),
    "Space_Exploration_Find the Tree of Life": (3200, "Achievement_Tree of Life"),
    "Space_Exploration_Space Vanguard": (3200, "Achievement_Colony Vanguard"),
    "Space_Exploration_Escort Ship": (3200, "Achievement_Escort Ship"),
    "Space_Exploration_Research Station": (3210, "Achievement_Research"),
    "Space_Exploration_Altar of Genius": (3210, "Achievement_Genius"),
    "Mythology_Exploration_Zeus' Party Invite": (2210, "Achievement_PartyInvite"),
    "Mythology_Exploration_Ice Cream": (2210, "Achievement_IceCream"),
    "Mythology_Exploration_Hidden Roof": (2320, "BA_Explore_Roof"),
    "Mythology_Story_Find Lightningblast's Arboratory": (2500, "BA_Explore_Forge"),
    "Dino_Exploration_Lava Pier": (6200, "Achievement_Volcano"),
    "Dino_Exploration_Lost Pirates": (6200, "Achievement_Swamp"),
    "Dino_Exploration_Beauty Salon": (6200, "Achievement_Tarpit"),
}


# the place of `find` achievements whose text doesn't name one ("Find the X in Y")
PLACES = {
    "Pirate_Exploration_The Sirens": "The Sirens",
    "Pirate_Story_Escape Jacket Fort": "Way out of Jacket Fort",
    "Pirate_Exploration_Cross the Lava River": "Lava River",
    "Pirate_Exploration_Discover the Cursed Treasure": "Cursed Treasure",
    "Pirate_Exploration_Climb the Volcano": "Top of the volcano",
    "Medieval_Exploration_Center of Attention": "Middle of the training grounds",
    "Medieval_Exploration_Overlook the Medieval World": "Overlook Pointe",
    "Medieval_Exploration_Take the Web Shortcut": "Web shortcut",
    "Space_Exploration_Space Vanguard": "Space Vanguard",
    "Space_Exploration_Escort Ship": "Path to the dark side",
}
FIND_TEXT = re.compile(r"^(?:Find|Discover) (?:the )?(.+?) in (?:the )?[^.]+\.?$")


def place(name, text):
    """What a `find` achievement's player finds, as they would call it."""
    if name in PLACES:
        return PLACES[name]
    if name.endswith("] Boombox"):
        return "Boombox"
    m = FIND_TEXT.match(text.strip())
    return m.group(1)[0].upper() + m.group(1)[1:] if m else None


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
    texts = tdc.texts(ACHIEVEMENT_TEXTS)
    titles = tdc.texts(ACHIEVEMENT_TITLES)
    out = []
    for i in sorted(keep - {ACHIEVEMENTS_ROOT}):
        n = nodes[i]
        s = n["stats"]
        name = n.get("name", "")
        parent = nodes.get(n.get("parent", 0), {})
        a = {"id": i, "parent": n.get("parent", 0), "name": name}
        if titles.get(i, "").strip():
            a["title"] = titles[i].strip()
        if texts.get(i, "").strip():
            a["text"] = texts[i].strip()
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
            d = re.search(r"Complete any (\d+)", texts.get(i, ""), re.I)
            a["kind"], a["n"] = "any", int((d or m).group(1))
        elif (m := re.search(r"_Lost Creations_Get to [Ll]evel (\d+)$", name)) and "minifigure" in a:
            a["kind"], a["n"] = "lost_creations", int(m.group(1))
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
        elif name in EXPLORE:
            a["kind"], a["playfield"], a["find"] = "find", *EXPLORE[name]
        elif m := re.search(r"_Combat_Champion_(.+)$", name):
            a["kind"], a["target"] = "defeat", m.group(1).strip()
        elif m := re.search(r"_Combat_PA_(?:Clear the )?(.+)$", name):
            a["kind"], a["target"] = "clear", m.group(1).strip()
        elif (m := re.search(r"_Collectibles_(.+)$", name)) and m.group(1) in COLLECTIBLES:
            a["kind"], a["find"] = "find", COLLECTIBLES[m.group(1)]
        elif (m := re.match(r"\[(.+)\] Boombox$", name)) and m.group(1) in BOOMBOX_WORLDS:
            a["kind"], a["find"], a["playfield"] = "find", "boombox 1", BOOMBOX_WORLDS[m.group(1)]
        elif m := re.search(r"Defeat (?!.*World Champions)(.+)$", name):
            a["kind"], a["target"] = "defeat", m.group(1).strip()
        elif "MinifigureTemplateID" in parent.get("stats", {}) and "_" in name and "Lost Creations" not in name:
            a["kind"], a["target"] = "target", name.split("_", 1)[1].strip()
        else:
            a["kind"] = "manual"
        if a["kind"] == "find" and (p := place(name, a.get("text", ""))):
            a["place"] = p
        out.append(a)
    # playfield names (TDB 52000) for the dungeon targets: world_content.json lacks some
    # ("Siege Command Room", "Spider's Nest")
    names = {str(k): v for k, v in sorted(tdc.texts(PLAYFIELD_NAMES).items())}
    doc = {"_comment": __doc__.split("\n\n")[0].replace("\n", " "), "achievements": out, "playfields": names}
    path = os.path.join(ROOT, "data/archive/achievements.json")
    json.dump(doc, open(path, "w"), indent=0)
    kinds = {}
    for a in out:
        kinds[a["kind"]] = kinds.get(a["kind"], 0) + 1
    print(f"{len(out)} achievements -> {path}: {kinds}")


main()
