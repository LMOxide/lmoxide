"""Client enemy spells whose cast animation a look can play (anim_audit.py's counterpart): spells
named by a regex, with each one's animation attributes (ability N needs motion ability_N,
chainattack N chain_1_<a+N-1>) and whether the look has them.
  python3 tools/content/anim_fit.py <monster data id> '<spell name regex>'"""
import json, os, re, sys
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
sys.path.insert(0, os.path.join(ROOT, "tools/re"))
import item_template as it
import rdb

idx = rdb.index()
md = {m["id"]: m for m in json.load(open(os.path.join(ROOT, "data/monster_data.json")))["monster_data"]}
anim = json.load(open(os.path.join(ROOT, "data/animations.json")))
sets = {m["id"]: {x["id"] for x in m["motions"]} for m in anim["motion_sets"]}


def attrs(pkg):
    try:
        b = rdb.body(1000622, pkg, idx).decode("latin1", "replace")
    except (KeyError, FileNotFoundError):
        return []
    return re.findall(r"<attributename>([^<]*)</attributename>\s*<attributetype>[^<]*</attributetype>\s*<\w+value>([^<]*)<", b)


def fits(have, a):
    for name, v in a:
        if name == "ability" and v.isdigit() and f"ability_{v}" not in have:
            return False
        if name in ("chainattack", "chainattack2") and v.isdigit() and int(v) > 0 and not any(m.startswith("chain_") for m in have):
            return False
    return True


look = md[int(sys.argv[1])]
have = sets.get(look["motion_set"], set())
print(look["name"], sorted(have))
rx = re.compile(sys.argv[2], re.I)
for t, r in idx:
    if t != 1000621:
        continue
    try:
        name, _d, st = it.parse(rdb.body(t, r, idx))
    except Exception:
        continue
    if not name or not rx.search(name) or not st.get("EffectPackage"):
        continue
    a = attrs(st["EffectPackage"])
    keep = {k: st[k] for k in ("AISpellCategory", "AIPowerrating", "AttackRange", "RechargeDelay", "TargetingMode") if k in st}
    print(f"{'OK ' if fits(have, a) else '-- '}{r} {name} {keep} pkg {st['EffectPackage']} {[f'{k}={v}' for k, v in a if k not in ('no_interrupt',)]}")
