"""Minimal AVM2 (ActionScript 3) disassembler for the client's Flash GUIs.

  tools/re/py abc_dis.py FILE.swf REGEX     disassemble every method whose trait name
                                           (Class/method) matches REGEX
  tools/re/py abc_dis.py FILE.swf -l REGEX  list matching method names only

Parses DoABC/DoABC2 tags fully (constant pool, methods, instances, classes, scripts, bodies)
and prints each matching method body as opcodes with resolved names/strings. Enough to read
GUI logic (e.g. how a server response code is handled), not a decompiler.
"""
import re, struct, sys, zlib

from swf_strings import u30


def s24(b, o):
    v = b[o] | b[o + 1] << 8 | b[o + 2] << 16
    return (v - (1 << 24) if v & 0x800000 else v), o + 3


class ABC:
    def __init__(s, b):
        s.b = b
        o = 4
        n, o = u30(b, o); s.ints = [0]
        for _ in range(max(n - 1, 0)):
            v, o = u30(b, o); s.ints.append(v - (1 << 32) if v & 0x80000000 else v)
        n, o = u30(b, o); s.uints = [0]
        for _ in range(max(n - 1, 0)):
            v, o = u30(b, o); s.uints.append(v)
        n, o = u30(b, o); s.doubles = [0.0]
        for _ in range(max(n - 1, 0)):
            s.doubles.append(struct.unpack_from("<d", b, o)[0]); o += 8
        n, o = u30(b, o); s.strings = [""]
        for _ in range(max(n - 1, 0)):
            ln, o = u30(b, o); s.strings.append(b[o:o + ln].decode("utf8", "replace")); o += ln
        n, o = u30(b, o); s.ns = [""]
        for _ in range(max(n - 1, 0)):
            o += 1; v, o = u30(b, o); s.ns.append(s.strings[v])
        n, o = u30(b, o)
        for _ in range(max(n - 1, 0)):
            c, o = u30(b, o)
            for _ in range(c):
                _, o = u30(b, o)
        n, o = u30(b, o); s.mn = ["*"]
        for _ in range(max(n - 1, 0)):
            k = b[o]; o += 1
            if k in (0x07, 0x0D):
                a, o = u30(b, o); nm, o = u30(b, o); s.mn.append(s.strings[nm])
            elif k in (0x0F, 0x10):
                nm, o = u30(b, o); s.mn.append(s.strings[nm])
            elif k in (0x11, 0x12):
                s.mn.append("[rtqnl]")
            elif k in (0x09, 0x0E):
                nm, o = u30(b, o); _, o = u30(b, o); s.mn.append(s.strings[nm])
            elif k in (0x1B, 0x1C):
                _, o = u30(b, o); s.mn.append("[mnl]")
            elif k == 0x1D:
                q, o = u30(b, o); c, o = u30(b, o); ps = []
                for _ in range(c):
                    p, o = u30(b, o); ps.append(p)
                s.mn.append(f"{s.mn[q] if q < len(s.mn) else q}.<{ps}>")
            else:
                raise ValueError(f"multiname kind {k:#x} at {o}")
        n, o = u30(b, o); s.methods = []
        for _ in range(n):
            pc, o = u30(b, o); _, o = u30(b, o)
            for _ in range(pc):
                _, o = u30(b, o)
            nm, o = u30(b, o); fl = b[o]; o += 1
            if fl & 0x08:
                c, o = u30(b, o)
                for _ in range(c):
                    _, o = u30(b, o); o += 1
            if fl & 0x80:
                for _ in range(pc):
                    _, o = u30(b, o)
            s.methods.append(s.strings[nm])
        n, o = u30(b, o)
        for _ in range(n):
            _, o = u30(b, o); c, o = u30(b, o)
            for _ in range(c):
                _, o = u30(b, o); _, o = u30(b, o)
        n, o = u30(b, o)
        s.mname = {}
        insts = []
        for _ in range(n):
            nm, o = u30(b, o); _, o = u30(b, o); fl = b[o]; o += 1
            if fl & 0x08:
                _, o = u30(b, o)
            c, o = u30(b, o)
            for _ in range(c):
                _, o = u30(b, o)
            ii, o = u30(b, o)
            cls = s.mn[nm]
            s.mname[ii] = f"{cls}/<init>"
            o = s.traits(o, cls)
            insts.append(cls)
        for i in range(n):
            ci, o = u30(b, o)
            s.mname[ci] = f"{insts[i]}/<cinit>"
            o = s.traits(o, insts[i] + "$")
        n, o = u30(b, o)
        for i in range(n):
            si, o = u30(b, o); s.mname.setdefault(si, f"script{i}/<init>")
            o = s.traits(o, f"script{i}")
        n, o = u30(b, o); s.bodies = {}
        for _ in range(n):
            m, o = u30(b, o)
            for _ in range(4):
                _, o = u30(b, o)
            ln, o = u30(b, o); code = b[o:o + ln]; o += ln
            c, o = u30(b, o)
            for _ in range(c):
                for _ in range(5):
                    _, o = u30(b, o)
            o = s.traits(o, s.mname.get(m, f"m{m}"))
            s.bodies[m] = code

    def traits(s, o, owner):
        b = s.b
        n, o = u30(b, o)
        for _ in range(n):
            nm, o = u30(b, o); k = b[o]; o += 1
            kind = k & 0xF
            if kind in (0, 6):
                _, o = u30(b, o); _, o = u30(b, o); v, o = u30(b, o)
                if v:
                    o += 1
            elif kind in (4,):
                _, o = u30(b, o); _, o = u30(b, o)
            elif kind in (1, 2, 3, 5):
                _, o = u30(b, o); m, o = u30(b, o)
                tag = {1: "", 2: "get ", 3: "set ", 5: "fn "}[kind]
                s.mname[m] = f"{owner}/{tag}{s.mn[nm]}"
            if k >> 4 & 0x4:
                c, o = u30(b, o)
                for _ in range(c):
                    _, o = u30(b, o)
        return o


OPS = {0x01: "bkpt", 0x02: "nop", 0x03: "throw", 0x04: ("getsuper", "m"), 0x05: ("setsuper", "m"), 0x06: ("dxns", "s"),
       0x07: "dxnslate", 0x08: ("kill", "u"), 0x09: "label", 0x0C: ("ifnlt", "j"), 0x0D: ("ifnle", "j"), 0x0E: ("ifngt", "j"),
       0x0F: ("ifnge", "j"), 0x10: ("jump", "j"), 0x11: ("iftrue", "j"), 0x12: ("iffalse", "j"), 0x13: ("ifeq", "j"),
       0x14: ("ifne", "j"), 0x15: ("iflt", "j"), 0x16: ("ifle", "j"), 0x17: ("ifgt", "j"), 0x18: ("ifge", "j"),
       0x19: ("ifstricteq", "j"), 0x1A: ("ifstrictne", "j"), 0x1B: ("lookupswitch", "L"), 0x1C: "pushwith", 0x1D: "popscope",
       0x1E: "nextname", 0x1F: "hasnext", 0x20: "pushnull", 0x21: "pushundefined", 0x23: "nextvalue", 0x24: ("pushbyte", "b"),
       0x25: ("pushshort", "u"), 0x26: "pushtrue", 0x27: "pushfalse", 0x28: "pushnan", 0x29: "pop", 0x2A: "dup", 0x2B: "swap",
       0x2C: ("pushstring", "s"), 0x2D: ("pushint", "i"), 0x2E: ("pushuint", "U"), 0x2F: ("pushdouble", "d"),
       0x30: "pushscope", 0x31: ("pushnamespace", "u"), 0x32: ("hasnext2", "uu"), 0x40: ("newfunction", "u"), 0x41: ("call", "u"),
       0x42: ("construct", "u"), 0x43: ("callmethod", "uu"), 0x44: ("callstatic", "uu"), 0x45: ("callsuper", "mu"),
       0x46: ("callproperty", "mu"), 0x47: "returnvoid", 0x48: "returnvalue", 0x49: ("constructsuper", "u"),
       0x4A: ("constructprop", "mu"), 0x4C: ("callproplex", "mu"), 0x4E: ("callsupervoid", "mu"), 0x4F: ("callpropvoid", "mu"),
       0x53: ("applytype", "u"), 0x55: ("newobject", "u"), 0x56: ("newarray", "u"), 0x57: "newactivation", 0x58: ("newclass", "u"),
       0x59: ("getdescendants", "m"), 0x5A: ("newcatch", "u"), 0x5D: ("findpropstrict", "m"), 0x5E: ("findproperty", "m"),
       0x5F: ("finddef", "m"), 0x60: ("getlex", "m"), 0x61: ("setproperty", "m"), 0x62: ("getlocal", "u"), 0x63: ("setlocal", "u"),
       0x64: "getglobalscope", 0x65: ("getscopeobject", "u"), 0x66: ("getproperty", "m"), 0x68: ("initproperty", "m"),
       0x6A: ("deleteproperty", "m"), 0x6C: ("getslot", "u"), 0x6D: ("setslot", "u"), 0x6E: ("getglobalslot", "u"),
       0x6F: ("setglobalslot", "u"), 0x70: "convert_s", 0x71: "esc_xelem", 0x72: "esc_xattr", 0x73: "convert_i", 0x74: "convert_u",
       0x75: "convert_d", 0x76: "convert_b", 0x77: "convert_o", 0x78: "checkfilter", 0x80: ("coerce", "m"), 0x82: "coerce_a",
       0x85: "coerce_s", 0x86: ("astype", "m"), 0x87: "astypelate", 0x90: "negate", 0x91: "increment", 0x92: ("inclocal", "u"),
       0x93: "decrement", 0x94: ("declocal", "u"), 0x95: "typeof", 0x96: "not", 0x97: "bitnot", 0xA0: "add", 0xA1: "subtract",
       0xA2: "multiply", 0xA3: "divide", 0xA4: "modulo", 0xA5: "lshift", 0xA6: "rshift", 0xA7: "urshift", 0xA8: "bitand",
       0xA9: "bitor", 0xAA: "bitxor", 0xAB: "equals", 0xAC: "strictequals", 0xAD: "lessthan", 0xAE: "lessequals",
       0xAF: "greaterthan", 0xB0: "greaterequals", 0xB1: "instanceof", 0xB2: ("istype", "m"), 0xB3: "istypelate", 0xB4: "in",
       0xC0: "increment_i", 0xC1: "decrement_i", 0xC2: ("inclocal_i", "u"), 0xC3: ("declocal_i", "u"), 0xC4: "negate_i",
       0xC5: "add_i", 0xC6: "subtract_i", 0xC7: "multiply_i", 0xD0: "getlocal0", 0xD1: "getlocal1", 0xD2: "getlocal2",
       0xD3: "getlocal3", 0xD4: "setlocal0", 0xD5: "setlocal1", 0xD6: "setlocal2", 0xD7: "setlocal3", 0xEF: ("debug", "D"),
       0xF0: ("debugline", "u"), 0xF1: ("debugfile", "s")}


def dis(a, code):
    o = 0
    out = []
    while o < len(code):
        at = o
        op = code[o]; o += 1
        d = OPS.get(op, f"op_{op:#x}")
        if isinstance(d, str):
            out.append(f"{at:5d} {d}")
            continue
        name, args = d
        parts = []
        for t in args:
            if t == "j":
                v, o = s24(code, o); parts.append(f"->{o + v}")
            elif t == "b":
                v = code[o]; o += 1; parts.append(str(v - 256 if v > 127 else v))
            elif t == "L":
                base = at
                dflt, o = s24(code, o); n, o = u30(code, o); cs = []
                for _ in range(n + 1):
                    v, o = s24(code, o); cs.append(base + v)
                parts.append(f"default->{base + dflt} cases->{cs}")
            elif t == "D":
                o += 1; v, o = u30(code, o); o += 1; _, o = u30(code, o); parts.append(a.strings[v] if v < len(a.strings) else str(v))
            else:
                v, o = u30(code, o)
                if t == "m":
                    parts.append(a.mn[v] if v < len(a.mn) else f"mn{v}")
                elif t == "s":
                    parts.append(repr(a.strings[v]) if v < len(a.strings) else f"s{v}")
                elif t == "i":
                    parts.append(str(a.ints[v]))
                elif t == "U":
                    parts.append(str(a.uints[v]))
                elif t == "d":
                    parts.append(str(a.doubles[v]))
                else:
                    parts.append(str(v))
        if name in ("debugline", "debug"):
            continue
        out.append(f"{at:5d} {name} {' '.join(parts)}")
    return out


def abcs(path):
    d = open(path, "rb").read()
    if d[:3] == b"CWS":
        d = d[:8] + zlib.decompress(d[8:])
    nbits = d[8] >> 3
    o = 8 + (5 + 4 * nbits + 7) // 8 + 4
    while o < len(d):
        h = struct.unpack_from("<H", d, o)[0]; o += 2
        code, ln = h >> 6, h & 0x3f
        if ln == 0x3f:
            ln = struct.unpack_from("<I", d, o)[0]; o += 4
        body = d[o:o + ln]; o += ln
        if code == 82:
            body = body[body.index(b"\0", 4) + 1:]
        elif code != 72:
            continue
        yield ABC(body)


def main():
    path = sys.argv[1]
    lst = sys.argv[2] == "-l"
    rx = re.compile(sys.argv[3] if lst else sys.argv[2])
    for a in abcs(path):
        for m, code in a.bodies.items():
            nm = a.mname.get(m, f"m{m}")
            if rx.search(nm):
                print(f"== {nm}")
                if not lst:
                    print("\n".join(dis(a, code)))


if __name__ == "__main__":
    main()
