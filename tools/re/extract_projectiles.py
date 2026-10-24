"""Projectile parameters of every minifigure spell (left/right-click spells, upgrade abilities):
data/projectiles.json, spell -> {item, type, speed, attractor, offset_z, ricochets, range}.
The client's Projectile GameAction does nothing by itself; the server spawns the projectile dynel
(CoreProjectileFullUpdateIIR) from these values.  Usage: tools/re/py extract_projectiles.py"""
import json, os

import rdb
from gameactions import actions

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
spells = json.load(open(os.path.join(ROOT, "data/minifigure_spells.json")))
abilities = json.load(open(os.path.join(ROOT, "data/minifigure_abilities.json")))
ids = {s for v in spells.values() for s in v} | {a[1] for v in abilities.values() for a in v}
idx = rdb.index()
out = {}
for sid in sorted(ids):
    try:
        acts = actions(sid, idx=idx)
    except Exception:
        continue
    for a in acts:
        if a["type"] != "Projectile":
            continue
        g = a["args"]
        out[str(sid)] = {
            "item": g.get("GameAction_ItemInstance", 0),
            "type": g.get("GameAction_ProjectileType", 1),
            "speed": g.get("GameAction_Speed", 1000) / 100,
            "attractor": g.get("AttractorID", g.get("GameAction_AttractorID", 0)),
            "offset_z": g.get("GameAction_DisplaceMentZ", 0) / 100,
            "ricochets": g.get("GameAction_MaxRicochets", 0),
            "range": g.get("AttackRange", 0) / 100,
        }
        break  # the first (uncharged) projectile
json.dump(out, open(os.path.join(ROOT, "data/projectiles.json"), "w"), indent=0, sort_keys=True)
print(len(out), "projectile spells")
