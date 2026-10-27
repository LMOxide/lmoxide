"""Where each minifigure comes from -> data/minifigure_drops.json.

  tools/re/py extract_minifig_drops.py [out.json]

The Collection's "You can find me in ..." text of each minifigure (MinifigureCollection
dropLocationTdbEntry, TDB category 100; data/minifig_parts.json drop_location_tdb) says where its
bag and parts dropped. The drop tables themselves were server data; this file only tells the
server which world's enemies may drop which minifigure:

* source "world": dropped in `worlds` (names as the texts say, "Lost Creations, Dino World" is
  Dino World, "Monster Trials, LEGO Center" the Monster Trials).
* source "achievement": given for an achievement (`achievement`: its lore tag).
* source "membership": a long-term membership benefit (every player is a member here).
* source "store", "event", "club": store packs, the Winter Holiday event, LEGO Club codes; not
  dropped.

`worlds` maps each world to its playfields: the world's own playfields (thousands digit 2-6)
plus the side dungeons whose portal leads into it (data/world_content.json); the tutorial (1300)
counts as Pirate World. How rare each minifigure was is in data/minifigure_rarity.json (edited
by hand).
"""
import json, os, sys

import tdc

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "../../data/archive")

WORLD_DIGIT = {2: "Mythology World", 3: "Space World", 4: "Pirate World", 5: "Medieval World", 6: "Dino World"}
# "You can find me in the X." -> world
WORLD_TEXTS = {
    "Pirate World": ["Pirate World"],
    "Medieval World": ["Medieval World"],
    "Space World": ["Space World"],
    "Mythology World": ["Mythology World"],
    "Dino World": ["Dino World"],
    "Lost Creations, Dino World": ["Dino World"],
    "Monster Trials, LEGO Center": ["Monster Trials"],
}
# Chicken Suit Guy: "completing the 'Legend of the Chicken Suit Guy' achievement" = Global_Chicken
# Suit Guy (lore tag 942, data/achievements.json)
CHICKEN_ACHIEVEMENT = 942


def source(text):
    if text is None:
        return {"source": "unknown"}
    if text.startswith("You can find me in the "):
        place = text[len("You can find me in the "):].rstrip(".")
        return {"source": "world", "worlds": WORLD_TEXTS[place]}
    if "achievement" in text:
        return {"source": "achievement", "achievement": CHICKEN_ACHIEVEMENT}
    if "membership" in text:
        return {"source": "membership"}
    if "Most Awesome Pack" in text:
        return {"source": "store"}
    if "Winter Holiday" in text:
        return {"source": "event"}
    if "LEGO Club" in text:
        return {"source": "club"}
    raise SystemExit(f"unknown drop text: {text!r}")


def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(DATA, "minifigure_drops.json")
    texts = tdc.texts(100)
    figs = json.load(open(os.path.join(DATA, "minifig_parts.json")))["minifigures"]
    content = json.load(open(os.path.join(DATA, "world_content.json")))

    worlds = {w: [] for w in list(WORLD_DIGIT.values()) + ["Monster Trials"]}
    for pf, v in content.items():
        if not pf.isdigit() or not isinstance(v, dict):
            continue
        p = int(pf)
        world = WORLD_DIGIT.get(p // 1000) if p >= 2000 else None
        if 1600 <= p < 1700:
            world = next((WORLD_DIGIT.get(x["to"] // 1000) for x in v.get("portals", []) if x.get("to")), None)
        if p == 1300:
            world = "Pirate World"
        if world and (v.get("enemies") or v.get("smashables")):
            worlds[world].append(p)

    out = []
    for f in sorted(figs, key=lambda f: f["minifigure_template_id"]):
        text = texts.get(f.get("drop_location_tdb"))
        out.append({"template": f["minifigure_template_id"], "name": f["name"], "series": f.get("series"),
                    "text": text, **source(text)})
    json.dump({"_comment": __doc__.strip().splitlines()[0] + " See tools/re/extract_minifig_drops.py.",
               "worlds": worlds, "minifigures": out}, open(out_path, "w"), indent=1)
    print(out_path, len(out))


if __name__ == "__main__":
    main()
