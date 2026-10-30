"""Client spells as content skills: each spell named and every nano it reaches through CastNano,
NanoTemplate or TemplateID (and the projectile items its Projectile actions shoot, with the
nanos those cast where they hit), with their stats and game action lists, written to
content/skills/<slug>.json and content/items/<slug>.json. Records already there are left alone.

  tools/re/py import_spells.py [--comment TEXT] SPELL...

Slugs come from the client names ("XX_Call to Arms" -> call-to-arms), else spell-<id>; a slug
taken by another record gets the id appended. Projectile items' radius isn't in the client:
0.5 m as the other projectile items.
"""
import argparse, json, os, re

import gameactions as ga
import item_template as it
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
SKILLS = os.path.join(ROOT, "content/skills")
ITEMS = os.path.join(ROOT, "content/items")
KEEP = ("TimeExist", "ChannelingInterval", "ChannelingTicks", "RechargeDelay", "TargetingMode", "MaxBuffCounters", "ItemDelay",
        "EffectPackage", "AttackRange", "SpellLine", "AISpellCategory", "AIPowerrating", "MoreFlags")
REFS = ("Value", "NanoTemplate", "TemplateID", "TemplateID2")
CASTING = ("CastNano", "CoreFilteredAreaFormula", "CoreRandomPointAreaFormula", "CorePersistentGroundEffect", "ColumnFormula", "SpawnMonster",
           "PlaceTrap", "Blink", "Chain", "ColumnDash", "AddBuffCounter", "RemoveBuff")
idx = rdb.index()


def loaded(folder, key):
    out = {}
    for f in os.listdir(folder):
        r = json.load(open(os.path.join(folder, f)))
        out[r["id"]] = key(r)
    return out


skills = loaded(SKILLS, lambda r: (r.get("client") or {}).get("spell"))
items = loaded(ITEMS, lambda r: r.get("template"))
have_spells = {s for s in skills.values() if s}
have_items = {i for i in items.values() if i}


def slug(kind, name, rid, taken):
    s = re.sub(r"[^a-z0-9]+", "-", re.sub(r"^XX_", "", name.strip()).lower()).strip("-")
    s = s if s and not s.isdigit() else f"spell-{rid}"
    return f"{kind}/{s}" if f"{kind}/{s}" not in taken else f"{kind}/{s}-{rid}"


def short(args):
    return {k.replace("GameAction_", ""): v for k, v in args.items()}


def entries(acts, refs, shots):
    out = []
    for a in acts:
        args = short(a["args"])
        g = {"type": a["type"]}
        if a["client"]:
            g["client"] = True
        g["args"] = args
        if a.get("criteria"):
            g["criteria"] = [list(c) for c in a["criteria"]]
        out.append(g)
        if a["type"] in CASTING:
            refs.extend(args[k] for k in REFS if isinstance(args.get(k), int) and not (k == "Value" and a["type"] in ("AddBuffCounter", "SpawnMonster")))
        if a["type"] == "Projectile" and isinstance(args.get("ItemInstance"), int):
            shots.append(args["ItemInstance"])
    return out


def write(folder, rec):
    with open(os.path.join(folder, rec["id"].split("/")[1] + ".json"), "w") as f:
        json.dump(rec, f, indent=2)
        f.write("\n")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--comment", default="Imported from the client by tools/re/import_spells.py.")
    p.add_argument("spells", nargs="+", type=int)
    o = p.parse_args()
    todo, done, shots, wrote = list(o.spells), set(), [], []
    while todo or shots:
        if shots:
            item = shots.pop()
            if item in done:
                continue
            done.add(item)
            try:
                d = it.dump(item, idx)
            except (KeyError, FileNotFoundError):
                continue
            refs = []
            acts = entries(d["actions"], refs, [])
            todo.extend(refs)
            if item in have_items:
                continue
            rec = {"id": slug("item", d["name"], item, items), "name": d["name"].strip(), "kind": "projectile", "template": item}
            if d["stats"].get("EffectPackage"):
                rec["effect_package"] = d["stats"]["EffectPackage"]
            rec |= {"radius": 0.5, "actions": acts, "source": "client"}
            items[rec["id"]] = item
            write(ITEMS, rec)
            wrote.append(rec["id"])
            continue
        s = todo.pop()
        if s in done:
            continue
        done.add(s)
        try:
            name, _d, stats = it.parse(rdb.body(1000621, s, idx))
        except (KeyError, FileNotFoundError):
            print(f"{s}: not in the client")
            continue
        refs = []
        actions = {str(e): entries(acts, refs, shots) for e, acts in sorted((ga.lists(s, idx=idx) or {}).items())}
        todo.extend(refs)
        if s in have_spells:
            continue
        rec = {"id": slug("skill", name, s, skills), "name": name.strip(), "client": {"spell": s, "stats": {k: v for k, v in stats.items() if k in KEEP}, "actions": actions},
               "source": "client", "comment": o.comment}
        skills[rec["id"]] = s
        write(SKILLS, rec)
        wrote.append(rec["id"])
    print("\n".join(wrote) or "nothing new")


if __name__ == "__main__":
    main()
