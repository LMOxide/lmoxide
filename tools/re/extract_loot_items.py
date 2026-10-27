"""Loot-related item templates (RDB 1000620) -> data/loot_items.json.

  tools/re/py extract_loot_items.py [out.json]

* minifigure_bags: "Minifigure Bag" templates (dynel type 34001, they go into the bag inventory
  110002). Each names its minifigure in stat MinifigureTemplateID (3000088) and series
  (3000062); its use actions are two server-side SpawnItemOnCharacter with 4-char string hashes
  ("1E2L" on every bag + one per bag) that the client cannot resolve.
* part_items: minifigure part templates (dynel type 34002): ChangeTokenPoints(part token, 1).
* token_items: every template whose game actions include ChangeTokenPoints (token, amount).
* chests: templates with LootType (3000102) 1 = free chest, 2 = member chest (placed inside a
  loot bag; the client shows them as chests), with their server-side LootTable id.
* loot_tables: every template with a LootTable stat (2000139; destructibles, chests) -> id.
The loot tables themselves are server data and are not in the client.
"""
import json, os, struct, sys

import gameactions as ga
import rdb
from item_template import parse

HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "../../data/archive/loot_items.json")
    idx = rdb.index()
    items = {}
    for t, r in sorted(idx):
        if t != 1000620:
            continue
        b = rdb.body(t, r, idx)
        name, desc, stats = parse(b)
        items[r] = {"name": name, "dynel_type": struct.unpack_from("<I", b, 4)[0] if len(b) > 8 else None,
                    "stats": stats, "actions": ga.actions(r, 1000620, idx)}
    parts = json.load(open(os.path.join(HERE, "../../data/archive/minifig_parts.json")))
    part_of = {p["part_template_id"]: (f["minifigure_template_id"], f["name"], p["location"])
               for f in parts["minifigures"] for p in f["parts"]}

    bags, part_items, tokens, chests, tables = [], [], [], [], []
    for r, it in items.items():
        st, acts = it["stats"], it["actions"]
        if it["dynel_type"] == 34001:
            m = st.get("MinifigureTemplateID")
            bags.append({"bag_template": r, "name": it["name"], "minifigure_template": m,
                         "minifigure_name": items.get(m, {}).get("name"), "series": st.get("MinifigureSeries"),
                         "controlled_loot_type": st.get("ControlledLootType"),
                         "actions": [{"type": a["type"], **a["args"]} for a in acts]})
        for a in acts:
            if a["type"] == "ChangeTokenPoints":
                tok, amt = a["args"].get("GameAction_Arg1"), a["args"].get("GameAction_Arg2")
                tokens.append({"template": r, "name": it["name"], "token": tok, "amount": amt, "dynel_type": it["dynel_type"]})
                if it["dynel_type"] == 34002:
                    mf = part_of.get(r)
                    part_items.append({"template": r, "name": it["name"], "token": tok,
                                       "minifigure_template": mf[0] if mf else None,
                                       "minifigure_name": mf[1] if mf else None, "location": mf[2] if mf else None})
        if "LootType" in st:
            chests.append({"template": r, "name": it["name"], "loot_type": st["LootType"],
                           "loot_table": st.get("LootTable"), "monster_data": st.get("MonsterData"),
                           "effect_package": st.get("EffectPackage"), "spawn_effect_package": st.get("SpawnEffectPackage"),
                           "lua_script": st.get("LuaScriptID"),
                           "offset": [st.get("Loot_OffsetXPos"), st.get("Loot_OffsetYPos"), st.get("Loot_OffsetZPos")]})
        if "LootTable" in st:
            tables.append({"template": r, "name": it["name"], "dynel_type": it["dynel_type"], "loot_table": st["LootTable"],
                           "monster_data": st.get("MonsterData")})
    out = {"minifigure_bags": bags, "part_items": part_items, "token_items": tokens, "chests": chests, "loot_tables": tables}
    json.dump(out, open(out_path, "w"), indent=1)
    print(", ".join(f"{k}: {len(v)}" for k, v in out.items()), "->", out_path)


if __name__ == "__main__":
    main()
