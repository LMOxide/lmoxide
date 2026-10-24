"""Print the ActionScript 3 constant-pool strings of a client Flash GUI (Data/Gui/Default/Flash/*.swf).

  tools/re/py swf_strings.py FILE.swf [REGEX]

Decompresses CWS (zlib) / FWS, walks the tags, and for each DoABC (82) / DoABC2 (72) tag parses
the ABC constant pool (ints, uints, doubles, strings) and prints the strings (optionally filtered).
Enough to see which C++ methods / LDB keys / events a GUI uses, not its logic.
"""
import re, struct, sys, zlib


def u30(b, o):
    v = s = 0
    while True:
        c = b[o]; o += 1
        v |= (c & 0x7f) << s; s += 7
        if c < 0x80:
            return v, o


def abc_strings(abc):
    o = 4  # minor, major
    n, o = u30(abc, o)
    for _ in range(max(n - 1, 0)):
        _, o = u30(abc, o)
    n, o = u30(abc, o)
    for _ in range(max(n - 1, 0)):
        _, o = u30(abc, o)
    n, o = u30(abc, o)
    o += 8 * max(n - 1, 0)
    n, o = u30(abc, o)
    out = []
    for _ in range(max(n - 1, 0)):
        ln, o = u30(abc, o)
        out.append(abc[o:o + ln].decode("utf8", "replace"))
        o += ln
    return out


def main():
    d = open(sys.argv[1], "rb").read()
    if d[:3] == b"CWS":
        d = d[:8] + zlib.decompress(d[8:])
    elif d[:3] != b"FWS":
        raise SystemExit("not an uncompressed/zlib SWF")
    nbits = d[8] >> 3
    o = 8 + (5 + 4 * nbits + 7) // 8 + 4
    rx = re.compile(sys.argv[2]) if len(sys.argv) > 2 else None
    while o < len(d):
        h = struct.unpack_from("<H", d, o)[0]; o += 2
        code, ln = h >> 6, h & 0x3f
        if ln == 0x3f:
            ln = struct.unpack_from("<I", d, o)[0]; o += 4
        body = d[o:o + ln]; o += ln
        if code == 82:
            p = 4
            p = body.index(b"\0", p) + 1
            body = body[p:]
        elif code != 72:
            continue
        for s in abc_strings(body):
            if not rx or rx.search(s):
                print(s)


if __name__ == "__main__":
    main()
