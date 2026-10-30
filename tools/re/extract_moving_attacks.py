"""Effect packages whose animation is a moving attack (EMotionFX attribute `movingattack` true:
the recorded Captain Dreadleg spun after players) -> data/effect_moving_attacks.json [package].
An enemy keeps moving through a self-cast channel only if its package is one (npc_spells.rs
`busy`); otherwise it stands (the Minotaur's Rampage slid frozen).
  tools/re/py extract_moving_attacks.py   (after extract_effect_catalog.py)"""
import json, os, re

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
idx = rdb.index()
cat = json.load(open(os.path.join(ROOT, "data/effect_catalog.json")))["effects"]
out = []
for e in (cat.values() if isinstance(cat, dict) else cat):
    if "movingattack" not in (e.get("animation_attributes") or []):
        continue
    b = rdb.body(1000622, e["id"], idx).decode("latin1", "replace")
    vals = re.findall(r"<attributename>movingattack</attributename>\s*<attributetype>[^<]*</attributetype>\s*<\w+value>([^<]*)<", b)
    if any(v.lower() == "true" for v in vals):
        out.append(e["id"])
with open(os.path.join(ROOT, "data/effect_moving_attacks.json"), "w") as f:
    json.dump(sorted(out), f)
    f.write("\n")
print(len(out), "moving attack packages")
