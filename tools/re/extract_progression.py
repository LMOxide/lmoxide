"""Progression tables from the client -> data/progression.json:

  minifigure_xp   MinifigureXPTables (RDB 1000525): minifigure level -> MinifigureXP it takes
                  (u32 table count?, u32 entries, then (u32 level, u32 xp) pairs; the client's
                  GetNextLevelXP/GetLastLevelXP read it)
  rank_player_level  lore tags 466..474 "Tier N Upgrade Requirement" (stat Level): the player
                  level a minifigure of rank i needs to rank up (CarouselView.CanUpgrade reads
                  upgradeTags[rank]); rank 8 is the last (CanUpgrade refuses above 7)

Usage: tools/re/py extract_progression.py"""
import json, os, struct, sys

import rdb

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "demo"))
from pblib import parse  # noqa: E402

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
LEVEL_STAT = 54
TIER_TAGS = list(range(466, 475))

b = rdb.body(1000525, 0)
_count, _table = struct.unpack_from("<II", b, 0)
pairs = [struct.unpack_from("<II", b, 8 + 8 * i) for i in range((len(b) - 8) // 8)]
xp = {lvl: v for lvl, v in pairs}

levels = {}
for f, _w, v in parse(rdb.body(1000108, 0)):
    if f != 2:
        continue
    node = {k: x for k, _, x in parse(v) if k in (1,)}
    if node.get(1) in TIER_TAGS:
        for k, _w2, s in parse(v):
            if k == 10:
                st = {a: c for a, _, c in parse(s)}
                if st.get(1) == LEVEL_STAT:
                    levels[node[1]] = st.get(3)
doc = {
    "_comment": __doc__.split("\n\n")[0].replace("\n", " "),
    "minifigure_xp": {str(k): xp[k] for k in sorted(xp)},
    "rank_player_level": [levels[t] for t in TIER_TAGS],
}
path = os.path.join(ROOT, "data/archive/progression.json")
json.dump(doc, open(path, "w"), indent=1)
print(doc)
