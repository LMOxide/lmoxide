"""Enemy skills whose cast animation needs a motion the enemy's look doesn't have: an effect
package setting `ability` N needs an `ability_N` motion in the look's motion set (else the client
holds the model stiff, as the Pirate Grenadier's XX_Slash, ability 5). Lists npc, look, skill,
spell, the ability numbers wanted and the ones the look has.  python3 tools/content/anim_audit.py"""
import json, os, re, sys
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
sys.path.insert(0, os.path.join(ROOT, "tools/re"))
import rdb

idx = rdb.index()
md = {m["id"]: m for m in json.load(open(os.path.join(ROOT, "data/monster_data.json")))["monster_data"]}
anim = json.load(open(os.path.join(ROOT, "data/animations.json")))
sets = {m["id"]: {x["id"] for x in m["motions"]} for m in anim["motion_sets"]}
skills = {}
for f in os.listdir(os.path.join(ROOT, "content/skills")):
    s = json.load(open(os.path.join(ROOT, "content/skills", f)))
    if s.get("client"):
        skills[s["id"]] = s["client"]
cache = {}


def abilities(pkg):
    if pkg not in cache:
        try:
            b = rdb.body(1000622, pkg, idx).decode("latin1", "replace")
            cache[pkg] = {int(v) for v in re.findall(r"<attributename>ability</attributename>\s*<attributetype>[^<]*</attributetype>\s*<integervalue>(\d+)<", b)}
        except (KeyError, FileNotFoundError):
            cache[pkg] = set()
    return cache[pkg]


for f in sorted(os.listdir(os.path.join(ROOT, "content/npcs"))):
    n = json.load(open(os.path.join(ROOT, "content/npcs", f)))
    look = md.get((n.get("render") or {}).get("monster_data"))
    if not look or n.get("kind") not in ("enemy", "boss", "champion"):
        continue
    have = {int(m.split("_")[1]) for m in sets.get(look["motion_set"], set()) if re.fullmatch(r"ability_\d+", m)}
    for u in n.get("skills", []):
        c = skills.get(u["skill"])
        pkg = c and c.get("stats", {}).get("EffectPackage")
        want = abilities(pkg) if pkg else set()
        if want - have:
            print(f"{n['id']}\t{look['name']}\t{u['skill']}\t{c['spell']}\twants {sorted(want)} has {sorted(have)}")
