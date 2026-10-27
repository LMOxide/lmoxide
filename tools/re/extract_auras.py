"""Minifigure achievement auras, for crates/lmoxide-server/src/auras.rs. Writes
data/minifigure_auras.json: {minifigure template: [tag1, tag2, tag3, tag4]}.

The server-side Lua scripts ship with the client: RDB 1010223 id 0, a protobuf (field 1 version,
repeated field 2 = {1 id, 2 type, 3 source}). Script 2152 (LuaScriptID of nano 540656
"XX_Minifigure Aura Applier", cast when a minifigure becomes active and by achievement reward
scripts) holds `MinifigureAuraTagTable` ([template] = {tag1, tag2, tag3, tag4}) and casts on the
player: tag1..4 -> 540569 Superachiever, tag1..3 -> 540568 Grandmaster, tag1+2 -> 540567
Master, tag1 -> 540565 Disciple, else 540633 Aura Remover. tag1 = "<Minifig>_Complete Any 10",
tag2 = the minifigure's "all" achievement, tag3 = its series', tag4 = all minifigures.

Usage: tools/re/py extract_auras.py"""
import json, os, re

import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
SCRIPT = 2152


def varint(b, o):
    v = s = 0
    while True:
        c = b[o]
        o += 1
        v |= (c & 0x7F) << s
        s += 7
        if not c & 0x80:
            return v, o


def fields(b):
    """(field, value) pairs of one protobuf message (varints and length-delimited only)."""
    o = 0
    while o < len(b):
        key, o = varint(b, o)
        f, wire = key >> 3, key & 7
        if wire == 0:
            v, o = varint(b, o)
        elif wire == 2:
            n, o = varint(b, o)
            v, o = b[o : o + n], o + n
        else:
            raise ValueError(f"wire type {wire}")
        yield f, v


def script(sid):
    for f, entry in fields(rdb.body(1010223, 0, rdb.index())):
        if f == 2:
            e = dict(fields(entry))
            if e.get(1) == sid:
                return e[3].decode("latin-1")
    raise KeyError(sid)


source = script(SCRIPT)
table = source[source.index("MinifigureAuraTagTable") :]
table = table[: table.index("}\r\n\r\n") if "}\r\n\r\n" in table else len(table)]
auras = {int(t): [int(x) for x in tags.split(",")] for t, tags in re.findall(r"\[(\d+)\]\s*=\s*\{([\d,\s]+)\}", table)}
out = {"_comment": __doc__.split("\n\n")[0].replace("\n", " "), **{str(k): auras[k] for k in sorted(auras)}}
path = os.path.join(ROOT, "data/archive/minifigure_auras.json")
json.dump(out, open(path, "w"), indent=1)
print(f"{len(auras)} minifigures -> {path}")
