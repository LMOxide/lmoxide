"""RDB (resource database) helper: index lookup and resource extraction.

  rdb.py types                    resource counts per type
  rdb.py ids TYPE                 ids of one type
  rdb.py find ID                  every type that has a resource with this id
  rdb.py get TYPE ID [out]        print (or write) one resource body

RDB/RDBHashIndex.bin: "RDHI", u32 version (7), u32 resource count, u32 type count, then per
type: u32 type, u32 n, n records of 47 bytes: u32 id, u32 size, u32 ?, md5[16] (of the
uncompressed body; the local file is RDB/<type>/<MD5>.rdbres), u8 flags, 18 bytes ?. A .rdbres is u32 type, u32 id (high bits
are flags), u32 ?, then the body. Missing resources come from --cache (the server's rdb-cache,
IOz1-wrapped as served by the patch server).
"""
import os, struct, sys, zlib
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
CLIENT = os.environ.get("LMO_CLIENT") or os.path.join(HERE, "../../../clients/windows/LEGO Minifigures Online")
CACHE = os.environ.get("LMO_RDB_CACHE") or os.path.join(HERE, "../../../var/rdb-cache")
REC = 47


def index(client=None):
    d = open(os.path.join(client or CLIENT, "RDB/RDBHashIndex.bin"), "rb").read()
    assert d[:4] == b"RDHI", d[:4]
    ntypes = struct.unpack_from("<I", d, 12)[0]
    out = {}
    o = 16
    for _ in range(ntypes):
        t, n = struct.unpack_from("<2I", d, o)
        o += 8
        for _ in range(n):
            rid, size = struct.unpack_from("<2I", d, o)
            out[(t, rid)] = (size, d[o + 12 : o + 28].hex().upper())
            o += REC
    # (a second section follows: per-resource dependency lists, not decoded)
    return out


def unwrap(b):
    if b[:4] == b"IOz1":
        return zlib.decompress(b[8:])
    return b


def body(t, rid, idx=None, client=None):
    idx = idx or index(client)
    size, h = idx[(t, rid)]
    p = os.path.join(client or CLIENT, f"RDB/{t}/{h}.rdbres")
    if os.path.exists(p):
        b = open(p, "rb").read()
        # most .rdbres files start with (type, id|flags, 1); some are the bare body
        return b[12:] if struct.unpack_from("<I", b)[0] == t else b
    c = os.path.join(CACHE, "rdb/res", h[:2].lower(), h[2].lower(), h[3:].lower())
    for cand in (c, c.upper()):
        if os.path.exists(cand):
            return unwrap(open(cand, "rb").read())
    # fall back to any cached file named by the hash
    for root, _, files in os.walk(CACHE):
        for f in files:
            if f.lower() == h[3:].lower():
                return unwrap(open(os.path.join(root, f), "rb").read())
    raise FileNotFoundError(f"{t}/{rid} ({h}) not available locally")


def main():
    cmd = sys.argv[1]
    idx = index()
    if cmd == "types":
        for t, n in sorted(Counter(t for t, _ in idx).items()):
            print(t, n)
    elif cmd == "ids":
        t = int(sys.argv[2])
        print(" ".join(str(i) for tt, i in sorted(idx) if tt == t))
    elif cmd == "find":
        rid = int(sys.argv[2])
        for (t, i), (size, h) in sorted(idx.items()):
            if i == rid:
                print(t, i, size, h)
    elif cmd == "get":
        b = body(int(sys.argv[2]), int(sys.argv[3]), idx)
        if len(sys.argv) > 4:
            open(sys.argv[4], "wb").write(b)
        else:
            sys.stdout.buffer.write(b)


if __name__ == "__main__":
    main()
