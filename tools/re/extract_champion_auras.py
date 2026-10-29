"""Champion element auras (the server Lua's script 3722 "champion aura random selector") as
content skills: each aura spell (spell line 2020) and every nano it reaches through CastNano,
NanoTemplate or TemplateID, with their stats and game action lists, written to
content/skills/<slug>.json (skills already there are left alone).

  tools/re/py extract_champion_auras.py
"""
import json, os, re

import gameactions as ga
import item_template as it
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
SKILLS = os.path.join(ROOT, "content/skills")
# element -> aura spell (script 3722); 582786 removes spell line 2020 when the champion leaves combat
AURAS = {"light": 504532, "shadow": 579603, "tech": 635575, "magic": 506108, "nature": 506217}
KEEP = ("TimeExist", "ChannelingInterval", "ChannelingTicks", "RechargeDelay", "TargetingMode", "MaxBuffCounters", "ItemDelay",
        "EffectPackage", "AttackRange", "SpellLine", "AISpellCategory")
idx = rdb.index()

have = set()
for f in os.listdir(SKILLS):
    s = json.load(open(os.path.join(SKILLS, f)))
    if s.get("client", {}).get("spell") and "no-client-definition" not in s.get("tags", []):
        have.add(s["client"]["spell"])


def slug(name, spell):
    if spell in AURAS.values():
        return "champion-" + re.sub(r"[^a-z0-9]+", "-", name.split(":")[-1].lower()).strip("-")
    return f"spell-{spell}"


def record(spell):
    name, _d, stats = it.parse(rdb.body(1000621, spell, idx))
    lists = ga.lists(spell, idx=idx) or {}
    actions = {}
    refs = []
    for event, acts in sorted(lists.items()):
        out = []
        for a in acts:
            args = {k.replace("GameAction_", ""): v for k, v in a["args"].items()}
            g = {"type": a["type"]}
            if a["client"]:
                g["client"] = True
            g["args"] = args
            if a["criteria"]:
                g["criteria"] = [list(c) for c in a["criteria"]]
            out.append(g)
            for k in ("Value", "NanoTemplate", "TemplateID"):
                if a["type"] in ("CastNano", "CoreFilteredAreaFormula", "CoreRandomPointAreaFormula", "CorePersistentGroundEffect", "ColumnFormula") and isinstance(args.get(k), int):
                    refs.append(args[k])
        actions[str(event)] = out
    rec = {"id": f"skill/{slug(name, spell)}", "name": name, "client": {"spell": spell, "stats": {k: v for k, v in stats.items() if k in KEEP}, "actions": actions},
           "source": "client", "comment": "A champion's element aura (script 3722) or a nano it casts; tools/re/extract_champion_auras.py."}
    return rec, refs


todo, done, written = list(AURAS.values()) + [582786], set(), 0
while todo:
    s = todo.pop()
    if s in done:
        continue
    done.add(s)
    try:
        rec, refs = record(s)
    except KeyError:
        continue
    todo.extend(refs)
    if s in have:
        continue
    path = os.path.join(SKILLS, rec["id"].split("/")[1] + ".json")
    with open(path, "w") as f:
        json.dump(rec, f, indent=2)
        f.write("\n")
    written += 1
print(f"{written} skills written, {len(done)} spells visited")
