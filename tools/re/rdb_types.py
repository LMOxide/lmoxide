"""RDB type id -> resource class, from the registration calls (mov edx, factory; mov ecx, type;
call 0xc5f670). Usage: rdb_types.py [registration function addresses...]"""
import re, sys
from pe import *
from rtti import ctor_vtables, factory_classes, vtable_class, demangle_simple

REG = 0xc5f670

def registrations():
    out = {}
    for site in xrefs_call(REG):
        prev = list(md.disasm(rd(site - 10, 10), site - 10))
        edx = ecx = None
        for ins in prev:
            m = re.match(r'^(edx|ecx), (0x[0-9a-f]+)$', ins.op_str)
            if ins.mnemonic == 'mov' and m:
                if m.group(1) == 'edx': edx = int(m.group(2), 16)
                else: ecx = int(m.group(2), 16)
        if edx and ecx: out[ecx] = edx
    return out

def xrefs_call(target):
    t = textbytes(); out = []
    i = t.find(b'\xe8')
    while i != -1:
        if i + 5 <= len(t):
            rel = struct.unpack_from('<i', t, i + 1)[0]
            if TS + i + 5 + rel == target: out.append(TS + i)
        i = t.find(b'\xe8', i + 1)
    return out

def classes(factory):
    c = factory_classes(factory)
    if c: return c
    return ctor_vtables(factory)

if __name__ == '__main__':
    for t, f in sorted(registrations().items()):
        print(t, hex(f), ' '.join(classes(f)[:2]))
