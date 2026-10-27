"""Minifigure item template -> MonsterData (body/costume, RDB 1010514), from each template's
SetStatGameAction on stat 359 (RDB 1000620 body: `d2 02 00 00 00 | 67 01 00 00 | f2 02 00 00 00
01 00 00 00 f9 02 00 00 00` then u32 LE value). Writes data/minifigure_monster_data.json for the
minifigures in data/minifigure_spells.json.  Usage: tools/re/py extract_minifig_monster_data.py"""
import json, os, struct
from rdb import index, body

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
PATTERN = bytes.fromhex("d2020000006701000" "0f20200000001000000f902000000")
idx = index()
ids = json.load(open(os.path.join(ROOT, "data/archive/minifigure_spells.json")))
out, missing = {}, []
for t in sorted(ids, key=int):
    try:
        b = body(1000620, int(t), idx)
    except Exception:
        missing.append(t)
        continue
    i = b.find(PATTERN)
    if i < 0:
        missing.append(t)
        continue
    out[t] = struct.unpack_from("<I", b, i + len(PATTERN))[0]
json.dump(out, open(os.path.join(ROOT, "data/archive/minifigure_monster_data.json"), "w"), indent=1, sort_keys=True)
print(len(out), "found; missing:", missing)
