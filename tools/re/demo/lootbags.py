"""Decode every loot bag in the recorded session (recs.pkl from recs.py).

  python3 lootbags.py /path/to/recs.pkl [/path/to/tl.pkl]

LootBagFullUpdateIIR_c body (big-endian stream): u32 len + ProtoGameCodeDynelIIR, u32 len +
ProtoSimpleDynelIIR, VisualData (u32 resId, u32 resType, u32 n + n x 5 u32, 6 x f32, f32 scale,
u32, u8), u32 len + ProtoLootBagIIR {1 cash, 2 current_looter_id, 3 owner_id, 4 dropper_id,
5 allowed_looter_id, 6 special_loot_owner, 7 has_master_looter, 8 inventory}.
Prints per bag: owner/dropper/allowed, items (template, stack/stats) and the following
NewAllowedLooterIIR_c / LootRemoveItemsIIR_c records.
"""
import os, pickle, struct, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pblib import parse


def f(b):
    d = {}
    for k, w, v in parse(b):
        d.setdefault(k, []).append(v)
    return d


def id32(b):
    d = f(b)
    return (d.get(1, [0])[0], d.get(2, [0])[0])


def acg(b):
    d = f(b)
    return {k: struct.unpack("<I", d[k][0])[0] for k in (4, 5, 6, 8) if k in d} | ({"level": d[7][0]} if 7 in d else {})


def decode(body):
    o = 0
    n = struct.unpack_from(">I", body, o)[0]; dyn = f(body[o + 4:o + 4 + n]); o += 4 + n
    n = struct.unpack_from(">I", body, o)[0]; o += 4 + n
    o += 8
    n = struct.unpack_from(">I", body, o)[0]; o += 4 + 20 * n
    o += 24 + 4 + 4 + 1
    n = struct.unpack_from(">I", body, o)[0]
    bag = f(body[o + 4:o + 4 + n])
    out = {"cash": bag.get(1, [None])[0]}
    for k, name in ((2, "current_looter"), (3, "owner"), (4, "dropper"), (5, "allowed")):
        if k in bag:
            out[name] = id32(bag[k][0])
    out["special"] = [(f(e)[1][0], id32(f(e)[2][0]) if 2 in f(e) else None) for e in bag.get(6, [])]
    out["master"] = bag.get(7, [None])[0]
    items = []
    if 8 in bag:
        inv = f(bag[8][0])
        out["inv_id"] = id32(inv[1][0])
        out["max_size"] = inv.get(2, [None])[0]
        for e in inv.get(4, []):
            ed = f(e)
            ent = f(ed[2][0])
            stats = {f(s)[1][0]: f(s)[2][0] for s in ent.get(2, [])}
            items.append({"pos": ed[1][0], "item_id": ent.get(1, [None])[0], "acg": acg(ent[3][0]) if 3 in ent else None, "stats": stats})
    out["items"] = items
    out["pos"] = [struct.unpack("<f", x)[0] for x in f(dyn[1][0]).values() for x in x] if 1 in dyn else None
    return out


def main():
    recs = pickle.load(open(sys.argv[1], "rb"))
    follow = {}
    for r in recs:
        if r["name"] in ("NewAllowedLooterIIR_c", "LootRemoveItemsIIR_c", "NewCurrentLooterIIR_c", "LootUpdateAllowedLootersIIR", "LootSpecialLootUpdateIIR_c"):
            follow.setdefault(r["id"], []).append((r["i"], r["name"], r["body"].hex()))
    for r in recs:
        if r["name"] == "LootBagFullUpdateIIR_c":
            d = decode(r["body"])
            print(f"#{r['i']} bag {r['id']}: " + ", ".join(f"{k}={v}" for k, v in d.items() if k != "items"))
            for it in d["items"]:
                print("    ", it)
            for x in follow.get(r["id"], []):
                print("    ->", x)


if __name__ == "__main__":
    main()
