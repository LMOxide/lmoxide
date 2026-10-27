"""Every playfield's name -> data/playfields.json, from its playfield description (RDB 1070003
XML, `<playfield Name=".." ID="..">`), including those without a teleport destination (the
tutorial 1300, art and sound layers, test maps). data/destinations.json keeps the `!tp` targets.

  tools/re/py extract_playfields.py"""
import json, os, re
from html import unescape

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
idx = rdb.index()
names = {}
for t, r in sorted(idx):
    if t != 1070003:
        continue
    m = re.search(r'<playfield Name="([^"]*)"', rdb.body(t, r, idx).decode("utf-8", "replace"))
    if m:
        names[str(r)] = unescape(m.group(1))
path = os.path.join(ROOT, "data/archive/playfields.json")
with open(path, "w") as f:
    f.write('{\n "_comment": "Playfield names (RDB 1070003). See tools/re/extract_playfields.py.",\n "playfields": {\n')
    f.write(",\n".join(f"  {json.dumps(k)}: {json.dumps(v)}" for k, v in names.items()) + "\n }\n}\n")
print(path, len(names), "playfields")
