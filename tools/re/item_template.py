"""Dump item templates (RDB 1000620): name, description, dynel type, game actions, stat block.

  tools/re/py item_template.py ID...          one or more templates
  tools/re/py item_template.py -n REGEX       every template whose name matches

Layout (as far as decoded): u32 version (5), u32 dynel type (34000-34002 inventory items,
51320 simple dynel, 51321 destructible, 51341, 30xxx ...), ..., game action lists (see
gameactions.py; the action header's criteria hold e.g. the minifigure template id), then
u32 n + n x (u32 stat, u8 kind 0, u32 value), then u16 len + name, u16 len + description.
"""
import json, os, re, struct, sys

import gameactions as ga
import rdb

STATS = ga.STATS


def parse(b):
    """-> (name, description, stats dict) ; stats found by locating the block that ends at the name."""
    names = None
    for m in re.finditer(rb"([\x03-\x60])\x00([A-Za-z][ -~]{2,95})", b):
        n = m.group(1)[0]
        if len(m.group(2)) >= n:
            names = m.start(1)
            break
    name = desc = None
    stats = {}
    if names is not None:
        n = struct.unpack_from("<H", b, names)[0]
        name = b[names + 2 : names + 2 + n].decode("latin1")
        p = names + 2 + n
        if p + 2 <= len(b):
            dn = struct.unpack_from("<H", b, p)[0]
            desc = b[p + 2 : p + 2 + dn].decode("latin1")
        for cnt in range(0, 80):
            s = names - 4 - 9 * cnt
            if s < 0:
                break
            if struct.unpack_from("<I", b, s)[0] == cnt and all(b[s + 4 + 9 * i + 4] in (0, 1) for i in range(cnt)):
                for i in range(cnt):
                    k, kind = struct.unpack_from("<IB", b, s + 4 + 9 * i)
                    v = struct.unpack_from("<f" if kind else "<I", b, s + 9 + 9 * i)[0]
                    stats[STATS.get(str(k), str(k))] = v
                break
    return name, desc, stats


def dump(rid, idx):
    b = rdb.body(1000620, rid, idx)
    name, desc, stats = parse(b)
    return {"id": rid, "name": name, "desc": desc, "dynel_type": struct.unpack_from("<I", b, 4)[0],
            "actions": ga.actions(rid, 1000620, idx), "stats": stats}


def main():
    idx = rdb.index()
    if sys.argv[1] == "-n":
        rx = re.compile(sys.argv[2], re.I)
        ids = []
        for t, r in sorted(idx):
            if t == 1000620:
                n = parse(rdb.body(t, r, idx))[0]
                if n and rx.search(n):
                    ids.append(r)
    else:
        ids = [int(x) for x in sys.argv[1:]]
    for r in ids:
        print(json.dumps(dump(r, idx)))


if __name__ == "__main__":
    main()
