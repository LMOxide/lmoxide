"""Game action lists of the minifigures' abilities and every nano they cast, for the
server's interpreter (crates/lmoxide-server/src/actions.rs). Writes data/spell_actions.json:

  spell -> {name, stats: {TimeExist, ChannelingInterval, ChannelingTicks, RechargeDelay,
            TargetingMode, MaxBuffCounters, LuaScriptID, MoreFlags, ItemDelay, EffectPackage}
            (centiseconds where timed),
            lists: {event: [{type, client, args, criteria}]}}

Events (GameData::GameActionList_e): 0 OnUse, 1 OnRechargeDelay, 33 OnChanneling (each channel
tick), 35 channel start, 36 OnChannelingEnd, 39 PreUse. `client` = the client can run the action
itself (it runs those of its own spells); the rest are server-only. Criteria: (0, ability, 91)
= the caster has that upgrade ability, (buff, n, 166) = the buff's counter is n.

Spells covered: minifigure normal/special attacks and their combo steps (the hits' `via` in
data/minifigure_stats.json) and the upgrade abilities that replace an attack on its shortcut
(data/minifigure_abilities.json slot 104/105) that aren't charge projectiles (data/projectiles.json), plus every
spell they reach through CastNano,
NanoTemplate, RemoveBuff/AddBuffCounter or the TemplateID of Blink and Chain.

  items: projectile item -> {name, actions: [{type, client, args}]}: what the items the spells'
         Projectile actions shoot do where they hit or land (RDB 1000620; usually CastNano of a
         hit nano, a ground effect for bombs). Their nanos are covered too. The spells an OverrideShortcut puts on a shortcut
(TemplateID2: combo steps, mode switches) count as the minifigure's abilities too.  Usage: tools/re/py extract_spell_actions.py"""
import json, os

import gameactions as ga
import item_template as it
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
idx = rdb.index()
minifigs = json.load(open(os.path.join(ROOT, "data/archive/minifigure_stats.json")))["minifigures"]
projectiles = json.load(open(os.path.join(ROOT, "data/archive/projectiles.json")))
KEEP = ("TimeExist", "ChannelingInterval", "ChannelingTicks", "RechargeDelay", "TargetingMode", "MaxBuffCounters", "LuaScriptID", "MoreFlags",
        "ItemDelay", "EffectPackage")
SHORT = {"GameAction_": "", "AbilityMod": "AbilityMod"}


def short(args):
    return {k.replace("GameAction_", ""): v for k, v in args.items()}


def record(spell):
    try:
        b = rdb.body(1000621, spell, idx)
    except KeyError:
        return None
    name, _desc, stats = it.parse(b)
    lists = ga.lists(spell, idx=idx) or {}
    return {
        "name": name,
        "stats": {k: v for k, v in stats.items() if k in KEEP},
        "lists": {
            str(e): [
                {"type": a["type"], "client": a["client"], "args": short(a["args"]), **({"criteria": a["criteria"]} if a["criteria"] else {})}
                for a in acts
            ]
            for e, acts in sorted(lists.items())
        },
    }


def charge(spell):
    p = projectiles.get(str(spell))
    return bool(p and p.get("charge_items"))


roots = {}
for m in minifigs.values():
    for kind in ("normal", "special"):
        a = m.get(kind)
        if not a or charge(a["spell"]):
            continue
        # the ability and its combo steps (the client casts the next step while the previous
        # step's buff is on)
        steps = [a["spell"]] + [v for h in a.get("hits", []) for v in h.get("via", [])]
        for s in dict.fromkeys(steps):
            if record(s) and not charge(s):
                roots[s] = f"{m['name']}: {a['name']}"

def item_record(item):
    try:
        d = it.dump(item, idx)
    except (KeyError, FileNotFoundError):
        return None
    return {"name": d["name"], "actions": [{"type": a["type"], "client": a["client"], "args": short(a["args"])} for a in d["actions"]]}


def follow(a):
    """The spells an action reaches."""
    out = []
    for k in ("Value", "NanoTemplate", "TemplateID"):
        v = a["args"].get(k)
        if a["type"] == "SetSpellRecastTime" or not isinstance(v, int):
            continue
        if (v > 1000 and a["type"] in ("CastNano", "RemoveBuff", "AddBuffCounter", "Blink", "Chain")) or k == "NanoTemplate":
            out.append(v)
    return out


# upgrade abilities that take an attack's shortcut (slot 104 normal, 105 special: Lizard Man's
# MOAR RAWR replaces RAWR)
abilities = json.load(open(os.path.join(ROOT, "data/archive/minifigure_abilities.json")))
for template, rows in abilities.items():
    m = minifigs.get(template, {})
    for _bit, spell, slot in rows:
        if slot and spell not in roots and not charge(spell) and (r := record(spell)):
            roots[spell] = f"{m.get('name')}: {r['name']}"

out, items = {}, {}
todo = list(roots)
owner = dict(roots)
while todo:
    s = todo.pop()
    if str(s) in out:
        continue
    r = record(s)
    if not r:
        continue
    if s in roots:
        r["minifigure"] = roots[s]
    out[str(s)] = r
    for acts in r["lists"].values():
        for a in acts:
            v = a["args"].get("TemplateID2")
            if a["type"] == "OverrideShortcut" and isinstance(v, int) and v not in roots and record(v) and not charge(v):
                roots[v] = owner.get(s, "")
                owner[v] = roots[v]
                todo.append(v)
            for v in follow(a):
                owner.setdefault(v, owner.get(s, ""))
                todo.append(v)
            item = a["args"].get("ItemInstance")
            if a["type"] == "Projectile" and isinstance(item, int) and str(item) not in items:
                irec = item_record(item)
                if irec:
                    items[str(item)] = irec
                    for ia in irec["actions"]:
                        for v in follow(ia):
                            owner.setdefault(v, owner.get(s, ""))
                            todo.append(v)

out = {
    "_comment": __doc__.split("\n\n")[0].replace("\n", " "),
    **{k: out[k] for k in sorted(out, key=int)},
    "items": dict(sorted(items.items(), key=lambda kv: int(kv[0]))),
}
path = os.path.join(ROOT, "data/archive/spell_actions.json")
json.dump(out, open(path, "w"), indent=1)
print(f"{len(roots)} abilities and combo steps, {len(out) - 2} spells, {len(items)} projectile items -> {path}")
for s, who in sorted(roots.items(), key=lambda x: x[1]):
    print(f"  {s} {who}")
