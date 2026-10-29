#!/usr/bin/env python3
"""Set `power_tier` on the loot entries of content/loot/*.json from the client's item stat
`PowerTier` of what each entry drops (RDB 1000620): health orbs, minifigure parts (the
minifigures' part items), minifigure bags, chests and named items. An entry that drops several
kinds takes the highest tier; one whose drops have no PowerTier (stars, potions, star bundles,
nested tables) gets none. Run with the repo's venv, then `cargo run -p lmoxide-content -- fmt`:

  tools/re/py ../content/loot_power_tiers.py [content dir]
"""
import glob
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "re"))
import rdb  # noqa: E402
import item_template  # noqa: E402

content = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "..", "content")
idx = rdb.index()
items = {}
for path in glob.glob(os.path.join(content, "items", "*.json")):
    d = json.load(open(path))
    items[d["id"]] = d


def tier_of_template(t):
    if t is None:
        return None
    return item_template.parse(rdb.body(1000620, t, idx))[2].get("PowerTier")


def tier_of_item(ref):
    return tier_of_template(items.get(ref, {}).get("template"))


def kind_tier(kind):
    """The most common tier of the items of a kind (minifigure bags: 132 have 3, 2 have 3000)."""
    import collections
    tiers = collections.Counter(tier_of_item(i) for i, d in items.items() if d.get("kind") == kind)
    tiers.pop(None, None)
    print(kind, dict(tiers))
    return tiers.most_common(1)[0][0] if tiers else None


PART, BAG = kind_tier("part"), kind_tier("minifig_bag")
ORB = tier_of_item("item/health-orb")
changed = 0
for path in sorted(glob.glob(os.path.join(content, "loot", "*.json"))):
    table = json.load(open(path))
    for e in table["entries"]:
        d = e["drop"]
        tiers = [t for t in [ORB if d.get("hearts") else None, PART if d.get("minifig_parts") else None,
                             BAG if d.get("minifig_bags") else None]
                 + [tier_of_item(c) for c in d.get("chest", [])] + [tier_of_item(i) for i in d.get("items", [])] if t]
        tier = max(tiers) if tiers else None
        if e.get("power_tier") != tier:
            if tier is None:
                e.pop("power_tier", None)
            else:
                e["power_tier"] = tier
            changed += 1
    json.dump(table, open(path, "w"), indent=2)
print(f"orb {ORB}, part {PART}, minifigure bag {BAG}; {changed} entries changed")
