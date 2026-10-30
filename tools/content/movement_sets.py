#!/usr/bin/env python3
"""NPC movement speeds: the client's movement sets (RDB 1010205 `<movementset>`) into tunables
`npc.movement_sets`, and each content NPC template's `movement` (which set, and the speeds the
original server changed). Rerunnable; run `cargo run -p lmoxide-content -- fmt` afterwards.

The live server gave every NPC the speeds of one of the 25 sets (1361 of the capture's 1376 NPC
records exactly; the other 15 change only the run speed). The set is chosen, in this order:

  - an npc_template the live capture shows (every record of a template has the same speeds):
    the set with those speeds, else the closest set and the speeds that differ;
  - a name the live capture shows (other templates of that name): the speeds most of its records
    had;
  - the look's set (monster data `movementsetcombat`, RDB 1010514) when it is one made for that
    monster (ogre, treant, harpy, satyr, ...): live agreed with those every time it saw them.
    The generic sets (CharacterDefault, MonsterFast, Monster, MonsterSlow, no_turning) and the
    two live contradicted (PirateCorsair: live Corsairs used MonsterFast; ogre_ranged: the
    Gargantuan ran as the default) are left to
  - none: tunables `npc.movement_default` by kind (enemies MonsterFast, others
    CharacterDefault, as the server did before).

  LMO_CLIENT=<client dir> python3 tools/content/movement_sets.py [content dir]
"""
import collections
import glob
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "re"))
import lmxb  # noqa: E402
import rdb  # noqa: E402

CONTENT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "..", "content")
CAPTURE = os.path.join(HERE, "..", "..", "data", "live-capture", "lmo-captures", "playfield-objects.json")
SETS, MONSTER_DATA = 1010205, 1010514
# (content field, movement set attribute, live core stat)
FIELDS = [
    ("walk", "basewalkspeed", "movement_walk_speed"),
    ("run", "baserunspeed", "movement_run_speed"),
    ("sprint", "basesprintspeed", "movement_sprint_speed"),
    ("walk_backwards", "basereversewalkspeed", "movement_walk_backwards_speed"),
    ("charge", "chargespeed", "movement_charge_speed"),
]
# sets with the same speeds as another: the first named here wins
PREFERRED = ["CharacterDefaultMovementSet", "MonsterSlowMovementSet", "MonsterFastMovementSet"]
# sets that say nothing about the monster (and two the live capture contradicted)
GENERIC = {
    "PlayerMovementSet", "CharacterDefaultMovementSet", "no_turning_movementset", "MonsterFastMovementSet",
    "MonsterMovementSet", "MonsterSlowMovementSet", "PirateCorsairMovementset", "ogre_ranged_movementset",
}
DEFAULT = {"enemy": "monster-fast", "other": "character-default"}


def slug(name):
    """MonsterFastMovementSet -> monster-fast, ogre_ranged_movementset -> ogre-ranged."""
    n = re.sub(r"(?i)_?movement_?set$|Movement$", "", name)
    n = re.sub(r"([a-z0-9])([A-Z])", r"\1-\2", n).replace("_", "-").lower()
    return n.replace("-monster", "") if n.endswith("-monster") else n


def client_sets():
    idx = rdb.index()
    out = {}
    for t, r in idx:
        if t != SETS:
            continue
        xml = rdb.body(t, r, idx).decode("utf-8", "replace")
        a = dict(re.findall(r'(\w+)="([^"]*)"', xml))
        out[r] = {"id": r, "name": a["name"], **{f: float(a.get(k, 0)) for f, k, _ in FIELDS}}
    return out


def look_sets():
    """monster data -> its combat movement set id."""
    idx = rdb.index()
    out = {}
    for t, r in idx:
        if t != MONSTER_DATA:
            continue
        try:
            a = lmxb.parse(rdb.body(t, r, idx))["attrs"]
        except Exception:
            continue
        if a.get("movementsetcombat"):
            out[r] = int(a["movementsetcombat"])
    return out


def live():
    """(npc_template -> speeds, name -> Counter of speeds) of the live NPC records."""
    by_template, by_name = {}, collections.defaultdict(collections.Counter)
    with open(CAPTURE) as f:
        for o in json.load(f)["objects"]:
            core = o.get("core") or {}
            st = core.get("stats") or {}
            if o.get("kind") != "character" or "movement_run_speed" not in st or core.get("firstname"):
                continue
            speeds = tuple(round(float(st[s]), 3) for _, _, s in FIELDS)
            tpl = (core.get("npc_stats") or {}).get("template_id")
            if tpl is not None:
                assert by_template.setdefault(tpl, speeds) == speeds, (tpl, speeds)
            by_name[(o.get("name") or "").strip()][speeds] += 1
    return by_template, by_name


def movement(speeds, sets):
    """The set closest to `speeds` (fewest speeds differ; PREFERRED first) and the others."""
    def key(s):
        diff = sum(1 for (f, _, _), v in zip(FIELDS, speeds) if s[f] != v)
        return (diff, PREFERRED.index(s["name"]) if s["name"] in PREFERRED else len(PREFERRED), s["id"])
    best = min(sets.values(), key=key)
    m = {"set": slug(best["name"])}
    for (f, _, _), v in zip(FIELDS, speeds):
        if best[f] != v:
            m[f] = v
    return m


def main():
    sets, looks = client_sets(), look_sets()
    by_template, by_name = live()
    tunables_path = os.path.join(CONTENT, "tunables.json")
    with open(tunables_path) as f:
        tunables = json.load(f)
    table = {slug(s["name"]): {k: v for k, v in s.items()} for s in sorted(sets.values(), key=lambda s: s["id"])}
    assert len(table) == len(sets), "movement set slugs collide"
    tunables["npc"]["movement_sets"] = table
    tunables["npc"]["movement_default"] = DEFAULT
    with open(tunables_path, "w") as f:
        json.dump(tunables, f, indent=2, ensure_ascii=False)
        f.write("\n")
    why = collections.Counter()
    for path in sorted(glob.glob(os.path.join(CONTENT, "npcs", "*.json"))):
        with open(path) as f:
            d = json.load(f)
        render = d.get("render") or {}
        tpl, name = render.get("npc_template"), (d.get("name") or "").strip()
        look = sets.get(looks.get(render.get("monster_data")))
        if tpl in by_template:
            m, reason = movement(by_template[tpl], sets), "live template"
        elif name in by_name:
            m, reason = movement(by_name[name].most_common(1)[0][0], sets), "live name"
        elif look and look["name"] not in GENERIC:
            m, reason = {"set": slug(look["name"])}, "look"
        else:
            m, reason = None, "default"
        why[reason] += 1
        if d.get("movement") == m:
            continue
        if m is None:
            d.pop("movement", None)
        else:
            d["movement"] = m
        with open(path, "w") as f:
            json.dump(d, f, indent=2, ensure_ascii=False)
            f.write("\n")
        print(f"{os.path.basename(path)}: {d.get('name')} {m} ({reason})")
    print(dict(why))


if __name__ == "__main__":
    main()
