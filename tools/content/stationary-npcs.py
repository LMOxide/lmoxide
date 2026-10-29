#!/usr/bin/env python3
"""Marks enemy templates (content/npcs) whose look can't walk as `ai.stationary`.

A look's motion set (data/monster_data.json -> data/animations.json) without a walk or run
motion can't move on the client: cannons, the ballista troll, spider egg sacks and mounds,
Boom and Spore Shrooms, Alien Pods, the Kraken. Looks without a motion set are left alone (the
fire spirit has none and chased on live), and so is the invisible trails runner (scripted paths). Spider eggs and mounds are also `ai.unarmed`: on
live they only hatched spiderlings.

Usage: tools/content/stationary-npcs.py [--check], then `cargo run -p lmoxide-content -- fmt`
"""
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
UNARMED = ("spider-egg", "small-spider-mound")
# invisible look, but it ran scripted paths on live
MOVES_ANYWAY = ("xx-invis-trails-runner-green",)


def main():
    check = "--check" in sys.argv
    monsters = {m["id"]: m for m in json.loads((ROOT / "data/monster_data.json").read_text())["monster_data"]}
    sets = {s["id"]: s for s in json.loads((ROOT / "data/animations.json").read_text())["motion_sets"]}

    def can_walk(look):
        m = monsters.get(look)
        s = sets.get(m["motion_set"]) if m and m["motion_set"] else None
        if s is None:
            return True
        return any(x["id"].lower().startswith(("walk", "run")) for x in s["motions"])

    changed = []
    for path in sorted((ROOT / "content/npcs").glob("*.json")):
        t = json.loads(path.read_text())
        if t.get("kind") != "enemy":
            continue
        look = (t.get("render") or {}).get("monster_data")
        ai = dict(t.get("ai") or {})
        want = dict(ai)
        if not can_walk(look) and path.stem not in MOVES_ANYWAY:
            want["stationary"] = True
        if path.stem.startswith(UNARMED):
            want["unarmed"] = True
        if want != ai:
            changed.append(path.stem)
            if not check:
                t["ai"] = want
                path.write_text(json.dumps(t, indent=2, ensure_ascii=False) + "\n")
    print(("would change: " if check else "changed: ") + ", ".join(changed) if changed else "nothing to change")


if __name__ == "__main__":
    main()
