"""Minifigure parts, collection and upgrade data from the client RDB -> data/minifig_parts.json.

  tools/re/py extract_minifig_parts.py [out.json]

Sources (raw protobuf bodies, loaded by MinifigureHolder::LoadMinifigureData / LoadStaticMinifigureData):
* RDB 1000645 id 1  MinifigurePartsData  { MinifigureList { minifigure_template_id, parts_list
  { part_template_id, part_token_id, part_location 1 Head 2 Torso 3 Legs 4 Spark } },
  minifig_craft_templates }
* RDB 1000639 id 1  MinifigureCollection { MinifigureList { templateID, series,
  dropLocationTdbEntry, dropLocationPicture } }
* RDB 1000643 id 1  MinifigureUpgradeData (left/right click ids per minifigure)
Names: item templates (RDB 1000620) via extract_world_content.item_name.
"""
import json, os, sys

from rdb import body, index
from demo.pblib import parse

HERE = os.path.dirname(os.path.abspath(__file__))
LOC = {1: "Head", 2: "Torso", 3: "Legs", 4: "Spark"}


def fields(b):
    d = {}
    for f, w, v in parse(b):
        d.setdefault(f, []).append(v)
    return d


def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "../../data/minifig_parts.json")
    idx = index()
    from extract_world_content import item_name

    coll = {}
    for e in fields(body(1000639, 1, idx)).get(1, []):
        d = fields(e)
        coll[d[1][0]] = {k: d.get(n, [None])[0] for n, k in ((2, "series"), (3, "drop_location_tdb"), (4, "drop_location_picture"))}

    upg = {}
    for e in fields(body(1000643, 1, idx)).get(1, []):
        d = fields(e)
        upg[d[1][0]] = {"left_click": d.get(2, [None])[0], "right_click": d.get(3, [None])[0]}

    parts = fields(body(1000645, 1, idx))
    figs = []
    for e in parts.get(1, []):
        d = fields(e)
        mt = d[1][0]
        pl = []
        for p in d.get(2, []):
            pd = fields(p)
            t = pd.get(1, [None])[0]
            pl.append({
                "location": LOC.get(pd.get(3, [0])[0], pd.get(3, [None])[0]),
                "part_template_id": t,
                "part_token_id": pd.get(2, [None])[0],
                "part_name": item_name(t) if t else None,
            })
        figs.append({"minifigure_template_id": mt, "name": item_name(mt), **coll.get(mt, {}), **upg.get(mt, {}), "parts": pl})
    craft = []
    for v in parts.get(2, []):
        if isinstance(v, int):
            craft.append(v)
        else:  # packed
            o = 0
            from demo.pblib import varint
            while o < len(v):
                x, o = varint(v, o)
                craft.append(x)
    in_parts = {f["minifigure_template_id"] for f in figs}
    out = {
        "minifigures": figs,
        "collection_only": [{"minifigure_template_id": t, "name": item_name(t), **c} for t, c in sorted(coll.items()) if t not in in_parts],
        "minifig_craft_templates": [{"template_id": t, "name": item_name(t)} for t in craft],
    }
    json.dump(out, open(out_path, "w"), indent=1)
    print(f"{len(figs)} minifigures with parts, {len(out['collection_only'])} collection-only, {len(craft)} craft templates -> {out_path}")


if __name__ == "__main__":
    main()
