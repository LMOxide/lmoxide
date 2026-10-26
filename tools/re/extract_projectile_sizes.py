"""Hit radius of every projectile item and range of every spell that shoots one, so the server's
shots hit what the client's would: data/projectile_sizes.json.

  items:  projectile item -> radius (m) = max(0.5, CharRadius/100 x MonsterScale/100) of the
          item (stats 421, 360; MonsterScale 0: 1), as CoreProjectile::InitFromItem sets it. The
          client hits a character when the projectile's centre comes within both radii
          (ProjectileMoveTask::TryHitDynels; characters' CharRadius is the server's, 50).
  spells: spell -> AttackRange (m), the distance its projectile flies when the full update says
          0 (CoreProjectileFullUpdateIIR::Transfer; the server now sends it explicitly).

Items come from the minifigures' spells (projectiles.json, spell_actions.json), the enemies'
(npc_spells.json) and the scripted rooms' (EXTRA).  Usage: tools/re/py extract_projectile_sizes.py"""
import json, os

import item_template as it
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
MIN_RADIUS = 0.5
# projectiles shot by scripts rather than spells: tiki.rs FIREBALL, ballista.rs ARROW, cannon.rs SHOT_ITEM
EXTRA = (519852, 506676, 234304)


def load(name):
    return json.load(open(os.path.join(ROOT, "data", name)))


def projectile_actions(lists):
    for acts in lists.values():
        for a in acts:
            if a["type"] == "Projectile" and a["args"].get("ItemInstance"):
                yield a["args"]["ItemInstance"]


items, spells = set(EXTRA), set()
for sid, p in load("projectiles.json").items():
    spells.add(int(sid))
    items.update([p["item"], *p.get("charge_items", [])])
for source in (load("spell_actions.json"), load("npc_spells.json")["spells"]):
    for sid, s in source.items():
        if sid.startswith("_"):
            continue
        shot = list(projectile_actions(s.get("lists", {})))
        if shot:
            spells.add(int(sid))
            items.update(shot)

idx = rdb.index()
radii = {}
for item in sorted(items):
    _name, _desc, st = it.parse(rdb.body(1000620, item, idx))
    r = st.get("CharRadius", 0) / 100 or MIN_RADIUS
    if st.get("MonsterScale"):
        r *= st["MonsterScale"] / 100
    radii[str(item)] = round(max(MIN_RADIUS, r), 3)
ranges = {}
for spell in sorted(spells):
    try:
        _name, _desc, st = it.parse(rdb.body(1000621, spell, idx))
    except KeyError:
        continue
    if st.get("AttackRange"):
        ranges[str(spell)] = st["AttackRange"] / 100

out = {
    "_comment": "Projectile hit radii by item and ranges by spell (m), tools/re/extract_projectile_sizes.py",
    "items": radii,
    "spells": ranges,
}
with open(os.path.join(ROOT, "data/projectile_sizes.json"), "w") as f:
    json.dump(out, f, indent=1)
    f.write("\n")
print(len(radii), "items,", len(ranges), "spells with a range")
