"""Each world's element (docs/LOST-CREATIONS.md, "Elements") into content/worlds/*/world.json.

The Linux client's RDB 1000644/1 (TeleportWindowData) gives each window entry an
`elemental_type` (field 18: 1 Light, 2 Shadow, 3 Tech, 4 Magic, 5 Nature); the Windows copy in
clients/windows is older and lacks them. Live sent the same per zone (PlayfieldProxyData
`elemental_type`, LMO_Captures.cap). A playfield with several windows takes the element any of
them names. Usage: tools/re/py extract_elements.py"""
import glob, json, os

import rdb


def varint(b, o):
    r = s = 0
    while True:
        c = b[o]
        o += 1
        r |= (c & 0x7F) << s
        s += 7
        if c < 0x80:
            return r, o


def parse(b):
    """protobuf fields as (number, value) pairs"""
    o, out = 0, []
    while o < len(b):
        k, o = varint(b, o)
        f, t = k >> 3, k & 7
        if t == 0:
            v, o = varint(b, o)
        elif t == 2:
            n, o = varint(b, o)
            v, o = b[o:o + n], o + n
        elif t == 5:
            v, o = b[o:o + 4], o + 4
        elif t == 1:
            v, o = b[o:o + 8], o + 8
        else:
            raise ValueError((t, o))
        out.append((f, v))
    return out

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
CLIENT = os.path.join(ROOT, "../clients/linux/LEGO Minifigures Online")
NAMES = {1: "light", 2: "shadow", 3: "tech", 4: "magic", 5: "nature"}
# a test playfield's window
SKIP = {32000}


def elements():
    d = rdb.body(1000644, 1, rdb.index(CLIENT))
    for start in range(40):
        try:
            top = parse(d[start:])
            if all(f == 1 for f, _ in top):
                break
        except Exception:
            pass
    out = {}
    for _, w in top:
        for f, e in parse(w):
            if f == 2:
                entry = dict(parse(e))
                if entry.get(18) and entry[1] not in SKIP:
                    out.setdefault(entry[1], entry[18])
    return out


def main():
    found = elements()
    for path in sorted(glob.glob(os.path.join(ROOT, "content/worlds/*/world.json"))):
        with open(path) as f:
            world = json.load(f)
        element = NAMES.get(found.get(world["playfield"]))
        if world.get("element") == element:
            continue
        keys = list(world)
        if element:
            world["element"] = element
        else:
            world.pop("element", None)
        # after `kind`, as the record's fields go
        order = keys[: keys.index("kind") + 1] + ["element"] + [k for k in keys[keys.index("kind") + 1 :] if k != "element"]
        world = {k: world[k] for k in order if k in world}
        with open(path, "w") as f:
            json.dump(world, f, indent=2)
            f.write("\n")
        print(world["playfield"], element)


if __name__ == "__main__":
    main()
