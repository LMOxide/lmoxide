"""Minifigure -> (left-click, right-click) spell ids and upgrade abilities from the client's
MinifigureUpgradeData resource (RDB type 1000643, protobuf proto/MinifigureUpgradeData.proto).
Writes data/minifigure_spells.json and data/minifigure_abilities.json ([position, ability
template, slot position]; slot 104/105 overrides the left/right-click spell, 0 is passive).
Usage: tools/re/py extract_minifig_spells.py"""
import json, os, subprocess, sys, tempfile
from rdb import index, body

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "../..")
out = tempfile.mkdtemp()
subprocess.check_call(["protoc", "-I" + os.path.join(ROOT, "proto"), "--python_out=" + out, "MinifigureUpgradeData.proto"])
sys.path.insert(0, out)
import MinifigureUpgradeData_pb2 as m  # noqa: E402

idx = index()
table, abilities = {}, {}
for (t, rid) in sorted(idx):
    if t != 1000643:
        continue
    msg = m.MinifigureUpgradeData()
    msg.ParseFromString(body(t, rid, idx))
    for e in msg.MinifigureUpgradeList:
        table[str(e.minifigure_id)] = [e.left_click_id, e.right_click_id]
        abilities[str(e.minifigure_id)] = [[a.position, a.template_id, a.slot_position] for a in e.ability_list]
json.dump(table, open(os.path.join(ROOT, "data/archive/minifigure_spells.json"), "w"), indent=1, sort_keys=True)
json.dump(abilities, open(os.path.join(ROOT, "data/archive/minifigure_abilities.json"), "w"), sort_keys=True)
print(len(table), "minifigures")
