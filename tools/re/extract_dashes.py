"""Dash abilities: minifigure spells (normal, special, upgrade abilities) with a ColumnDash game
action, a server-only action (the client never moves by itself). Writes data/dashes.json:
spell -> distance (m), radius (hit column width), backwards, effect (the caster's effect
package, with "charge"/"execute" hooks), path_mod / arrival_mod (AbilityMod of the WeaponHit cast
on enemies along the path / around the end point, or null) and arrival_radius.

Channelled spells whose ColumnDash only comes with an upgrade (Samurai Warrior's stance) are left
out.  Usage: tools/re/py extract_dashes.py"""
import json, os

import gameactions as ga
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
idx = rdb.index()
minifigs = json.load(open(os.path.join(ROOT, "data/minifigure_stats.json")))["minifigures"]


def acts(spell):
    try:
        return ga.actions(spell, idx=idx)
    except Exception:
        return []


def weapon_hit(spell, depth=0):
    """(AbilityMod, area radius) of the first WeaponHit reached from `spell`."""
    for a in acts(spell):
        args = a["args"]
        if a["type"] == "WeaponHit":
            return args.get("AbilityMod", 1.0), None
        if depth < 2:
            nxt = args.get("GameAction_NanoTemplate") if a["type"].endswith("AreaFormula") else args.get("GameAction_Value")
            if a["type"] in ("CastNano",) or a["type"].endswith("AreaFormula"):
                hit = weapon_hit(nxt, depth + 1) if isinstance(nxt, int) else None
                if hit:
                    return hit[0], hit[1] or args.get("GameAction_Radius")
    return None


spells = {}
for m in minifigs.values():
    for kind in ("normal", "special"):
        if m.get(kind):
            spells[m[kind]["spell"]] = f"{m['name']}: {m[kind]['name']}"
    for u in m.get("upgrades", []):
        if u.get("ability"):
            spells[u["ability"]] = f"{m['name']}: {u.get('name', '')}"

out = {}
for spell, who in sorted(spells.items()):
    dashes = [a["args"] for a in acts(spell) if a["type"] == "ColumnDash"]
    if not dashes or len(dashes) > 1:
        continue
    d = dashes[0]
    if d.get("GameAction_ApplyOn") == 1 and "GameAction_NPCSelectMode" not in d:
        continue
    path = weapon_hit(d["GameAction_TemplateID"]) if "GameAction_TemplateID" in d else None
    arrival = weapon_hit(d["GameAction_TemplateID2"]) if "GameAction_TemplateID2" in d else None
    out[str(spell)] = {
        "name": who,
        "distance": d.get("GameAction_Distance", 0),
        "radius": d.get("GameAction_Radius", 1),
        "backwards": d.get("GameAction_Value2") == 1,
        "effect": d.get("GameAction_Value1", 0),
        "path_mod": path[0] if path else None,
        "arrival_mod": arrival[0] if arrival else None,
        "arrival_radius": arrival[1] if arrival else None,
    }
path = os.path.join(ROOT, "data/dashes.json")
lines = ["{"] + [f'  "{k}": {json.dumps(v)}' + ("," if i < len(out) - 1 else "") for i, (k, v) in enumerate(out.items())] + ["}"]
open(path, "w").write("\n".join(lines) + "\n")
print(len(out), "dashes")
