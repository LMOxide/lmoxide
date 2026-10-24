"""The client's localized text database (TDB) in the RDB.

RDB 1030002 holds TDC2 blobs, one per (category, language): "TDC2", u32 category, u32 0, u32
string bytes, u32 n, md5[16], the strings, then n x {u32 id, u32 ?, u32 offset, u32 length}
(little-endian). RDB 1030001 holds per-language TDL1 maps (category -> RDB id); English is id 21.

Categories used by dialogues and quests: 50002 Knubot (NPC dialogue) texts, 50001 dialogue
answers, 50300-50304 quest name / task / description / solved text / goal name, 51000 NPC
names.

  tools/re/py tdc.py 50002 741     print one text"""
import struct, sys

import rdb

IDX = rdb.index()
ENGLISH = 21


def parse(b):
    """-> (category, {id: text})"""
    c, _zero, size, n = struct.unpack_from("<4I", b, 4)
    strings = b[36 : 36 + size]
    out = {}
    for k in range(n):
        i, _flags, o, l = struct.unpack_from("<4I", b, 36 + size + 16 * k)
        out[i] = strings[o : o + l].rstrip(b"\0").decode("utf-8", "replace")
    return c, out


def language_map(lang=ENGLISH):
    """category -> RDB 1030002 id of that category's texts in `lang`."""
    b = rdb.body(1030001, lang, IDX)
    o = 8 + 3
    for _ in range(3):
        n = struct.unpack_from("<I", b, o)[0]
        o += 4 + n + 1
    n = struct.unpack_from("<I", b, o)[0]
    o += 4
    m = {}
    for _ in range(n):
        c, i = struct.unpack_from("<2I", b, o)
        o += 8
        m[c] = i
    return m


_EN = None


def texts(category):
    """{id: text} of a category in English."""
    global _EN
    if _EN is None:
        _EN = language_map()
    return parse(rdb.body(1030002, _EN[category], IDX))[1]


if __name__ == "__main__":
    print(texts(int(sys.argv[1])).get(int(sys.argv[2])))
