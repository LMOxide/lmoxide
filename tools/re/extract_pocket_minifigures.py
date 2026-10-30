"""Each pocket adventure's and story dungeon's minifigures into content/worlds/*/world.json:
`minifigure` (given for completing it) and `parts` (the minifigures whose parts drop there).

The Linux client's RDB 1000644/1 (TeleportWindowData) entries of playfield type 4 (the pocket
adventures: the "Complete all ... Pocket Adventures" achievements count exactly these) and 8 (the
story dungeons: the Ruined Fort, the Sorcerer's Keep, ...; the user, 2026-10-30: the Ruined Fort
dropped other parts than its window's) list
rewards (field 14), as the window shows them: the first list's one id is the minifigure's item
template (the big figure under "Rewards"), the second list's ids are portrait icons ("Parts
for:"), each a minifigure's MinifigureIconMiddle stat (item templates, RDB 1000620). Winter
Pass's (a Santa portrait only) is skipped. Run `cargo run -p lmoxide-content -- fmt` afterwards.
Usage: tools/re/py extract_pocket_minifigures.py"""
import glob, json, os

import item_template
import rdb
from extract_elements import CLIENT, ROOT, parse

# pocket adventures, story dungeons (not open worlds 1 or boss dungeons 2)
TYPES = (4, 8)


def windows():
    """playfield -> its pocket adventure window's reward lists"""
    d = rdb.body(1000644, 1, rdb.index(CLIENT))
    for start in range(40):
        try:
            top = parse(d[start:])
            if all(f == 1 for f, _ in top):
                break
        except Exception:
            pass
    out = {}
    for _, w in top:
        for f, e in parse(w):
            if f != 2:
                continue
            entry = parse(e)
            fields = dict(entry)
            if fields.get(2) not in TYPES:
                continue
            rewards = [[v for k, v in parse(r) if k == 2] for f2, r in entry if f2 == 14]
            if len(rewards) == 2:
                out.setdefault(fields[1], rewards)
    return out


def main():
    figures, portraits = {}, {}
    idx = rdb.index()
    for path in glob.glob(os.path.join(ROOT, "content/minifigures/*.json")):
        with open(path) as f:
            m = json.load(f)
        figures[m["template"]] = m["id"]
        _, _, stats = item_template.parse(rdb.body(1000620, m["template"], idx))
        portraits[stats.get("MinifigureIconMiddle")] = m["id"]
    found = windows()
    for path in sorted(glob.glob(os.path.join(ROOT, "content/worlds/*/world.json"))):
        with open(path) as f:
            world = json.load(f)
        reward, parts = found.get(world["playfield"], ([], []))
        figure = figures.get(reward[0]) if len(reward) == 1 else None
        parts = [portraits[p] for p in parts] if figure else []
        if world.get("minifigure") == figure and world.get("parts", []) == parts:
            continue
        keys = [k for k in world if k not in ("minifigure", "parts")]
        world["minifigure"], world["parts"] = figure, parts
        # after `region`, as the record's fields go
        at = keys.index("region") + 1 if "region" in keys else len(keys)
        order = keys[:at] + ["minifigure", "parts"] + keys[at:]
        world = {k: world[k] for k in order if k in keys or world[k]}
        with open(path, "w") as f:
            json.dump(world, f, indent=2)
            f.write("\n")
        print(world["playfield"], world["name"], figure, parts)


if __name__ == "__main__":
    main()
