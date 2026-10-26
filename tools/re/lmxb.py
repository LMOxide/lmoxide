"""LMXB: the client's binary XML (RDB 1010514 monster data, and other resources).

"LMXB", u32 version (1), then everything else is addressed by offsets from byte 8: u32 offset of
the root node, ... Node: u32 children offset (an array of nodes), u32 attributes offset, u32
name offset, u16 child count, u16 attribute count (16 bytes). Attribute: u32 name offset, u32
value offset. Names and values are NUL-terminated strings in a pool at the end.

  tools/re/py lmxb.py TYPE ID      print one resource as indented XML"""
import struct, sys

import rdb


def parse(b):
    """-> {"tag", "attrs": {name: value}, "children": [...]} of the root node."""
    if b[:4] != b"LMXB":
        raise ValueError("not LMXB")
    d = b[8:]
    text = lambda o: d[o : d.index(b"\0", o)].decode("utf-8", "replace")

    def node(o):
        children, attrs, name, nc, na = struct.unpack_from("<3I2H", d, o)
        a = {}
        for i in range(na):
            k, v = struct.unpack_from("<2I", d, attrs + 8 * i)
            a.setdefault(text(k), text(v))
        return {"tag": text(name), "attrs": a, "children": [node(children + 16 * i) for i in range(nc)]}

    return node(struct.unpack_from("<I", d, 0)[0])


def show(n, depth=0):
    attrs = "".join(f' {k}="{v}"' for k, v in n["attrs"].items())
    print("  " * depth + f"<{n['tag']}{attrs}" + (">" if n["children"] else "/>"))
    for c in n["children"]:
        show(c, depth + 1)
    if n["children"]:
        print("  " * depth + f"</{n['tag']}>")


if __name__ == "__main__":
    show(parse(rdb.body(int(sys.argv[1]), int(sys.argv[2]))))
