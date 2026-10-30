"""Client spells (RDB 1000621) as text: name, the stats the server uses, and every game action
list (event, type, C for client-run, args, criteria). The first step of reconstructing an enemy's
kit (docs/ENEMY-SKILLS.md).

  tools/re/py spell_dump.py 379720 440770          spells by id
  tools/re/py spell_dump.py -n 'grenadier|bomb'    spells whose name matches (regex, any case)
  tools/re/py spell_dump.py -i 319952              projectile items: name, effect, on-hit actions
  tools/re/py spell_dump.py -f 154034              spells whose EffectPackage is one of these
"""
import argparse, json, re

import gameactions as ga
import item_template as it
import rdb

STATS = ("TargetingMode", "AttackRange", "RechargeDelay", "ItemDelay", "ChannelingInterval", "ChannelingTicks", "TimeExist",
         "AISpellCategory", "AIPowerrating", "EffectPackage", "MaxBuffCounters", "SpellLine", "SpellLine3", "LuaScriptID")


def short(args):
    return {k.replace("GameAction_", ""): v for k, v in args.items()}


def spell(r, idx):
    try:
        name, _d, st = it.parse(rdb.body(1000621, r, idx))
    except (KeyError, FileNotFoundError):
        print(f"{r}: not in the client")
        return
    print(f"== {r} {name} {json.dumps({k: st[k] for k in STATS if k in st})}")
    for e, acts in sorted((ga.lists(r, idx=idx) or {}).items()):
        for a in acts:
            crit = f" {a['criteria']}" if a["criteria"] else ""
            print(f"   {e} {a['type']}{' C' if a['client'] else ''} {json.dumps(short(a['args']))}{crit}")


def all_spells(idx):
    for t, r in idx:
        if t != 1000621:
            continue
        try:
            name, _d, st = it.parse(rdb.body(t, r, idx))
        except Exception:
            continue
        yield r, name or "", st


def main():
    p = argparse.ArgumentParser()
    p.add_argument("-n", "--name", help="regex on spell names")
    p.add_argument("-i", "--items", action="store_true", help="the ids are projectile items")
    p.add_argument("-f", "--effect", action="store_true", help="the ids are effect packages")
    p.add_argument("ids", nargs="*", type=int)
    o = p.parse_args()
    idx = rdb.index()
    if o.name:
        rx = re.compile(o.name, re.I)
        for r, name, _ in sorted(all_spells(idx)):
            if rx.search(name):
                spell(r, idx)
    elif o.effect:
        for r, _, st in sorted(all_spells(idx)):
            if st.get("EffectPackage") in o.ids:
                spell(r, idx)
    elif o.items:
        for i in o.ids:
            d = it.dump(i, idx)
            print(f"== item {i} {d['name']} effect {d['stats'].get('EffectPackage')}")
            for a in d["actions"]:
                print(f"   {a['type']}{' C' if a['client'] else ''} {json.dumps(short(a['args']))}")
    else:
        for r in o.ids:
            spell(r, idx)


if __name__ == "__main__":
    main()
