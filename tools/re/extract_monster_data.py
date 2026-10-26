"""Every monster data (RDB 1010514, LMXB: tools/re/lmxb.py) -> data/monster_data.json: the looks
`!skin <id>` takes. Each has the designers' name (`minotaur_monster_boss`), its motion set (RDB
1050004, animations: data/animations.json), anim graph ("blendsetup", RDB 1050003), death effect
package (RDB 1000622) and mesh, and what uses it here: the characters and smashables placed in
the world (data/world_content.json, data/spawns.json) by name and kind, and the minifigures
(data/minifigure_monster_data.json) by item template.

  tools/re/py extract_monster_data.py"""
import json, os
from collections import defaultdict

import lmxb
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
MONSTER_DATA = 1010514


def world_users():
    """monster data -> {(name, kind)} of what the world data places."""
    users = defaultdict(set)
    content = json.load(open(os.path.join(ROOT, "data/world_content.json")))
    for pf in content.values():
        if not isinstance(pf, dict):  # (_comment)
            continue
        for kind, key in (("enemy", "enemies"), ("smashable", "smashables"), ("npc", "npcs")):
            for e in pf.get(key, []):
                if e.get("monster_data"):
                    users[e["monster_data"]].add((e["name"], kind))
    for spawns in json.load(open(os.path.join(ROOT, "data/spawns.json"))).values():
        for e in spawns if isinstance(spawns, list) else []:
            if e.get("monster_data"):
                users[e["monster_data"]].add((e["name"], e.get("kind", "enemy")))
    return users


def main():
    idx = rdb.index()
    users = world_users()
    figures = defaultdict(list)
    for template, md in json.load(open(os.path.join(ROOT, "data/minifigure_monster_data.json"))).items():
        figures[md].append(int(template))
    out, skipped = [], []
    for t, r in sorted(idx):
        if t != MONSTER_DATA or r == 0:  # (0: a copy of 1057; not a usable id)
            continue
        try:
            root = lmxb.parse(rdb.body(t, r, idx))
        except (ValueError, FileNotFoundError):
            skipped.append(r)
            continue
        if root["tag"] != "monsterdata":
            skipped.append(r)
            continue
        a = root["attrs"]
        child = lambda tag, key: next((int(c["attrs"][key]) for c in root["children"] if c["tag"] == tag and c["attrs"].get(key, "").isdigit()), None)
        num = lambda k: int(a[k]) if a.get(k, "").isdigit() else None
        out.append({
            "id": r,
            "name": a.get("name", ""),
            "motion_set": num("motionset"),
            "anim_graph": num("blendsetup"),
            "actor": num("actor"),
            "mesh": child("mesh", "mesh"),
            "death_effect_package": child("deatheffectpackage", "rdbid"),
            "used_by": [{"name": n, "kind": k} for n, k in sorted(users.get(r, ()))],
            "minifigures": sorted(figures.get(r, [])),
        })
    data = {"_comment": "Every monster data (RDB 1010514): the looks !skin takes, with what uses them. See tools/re/extract_monster_data.py.", "monster_data": out}
    path = os.path.join(ROOT, "data/monster_data.json")
    with open(path, "w") as f:
        f.write('{\n "_comment": ' + json.dumps(data["_comment"]) + ',\n "monster_data": [\n')
        f.write(",\n".join("  " + json.dumps(m) for m in out))
        f.write("\n ]\n}\n")
    print(path, len(out), "monster data;", sum(1 for m in out if m["used_by"] or m["minifigures"]), "used; skipped", skipped)


if __name__ == "__main__":
    main()
