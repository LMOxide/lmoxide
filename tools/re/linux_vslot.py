"""Resolve virtual-call slots in the Linux LMO build:  linux_vslot.py 'vtable for X' 0x360 0x378 ...
Prints vtable symbol + 0x10 + slot -> function name. Also accepts a raw vtable VA instead of a name.
Caveat: classes with virtual bases have extra header words; slot offsets are from the address point
(vtable symbol + 0x10 for simple classes), so check neighbours if a name looks wrong."""
import struct, subprocess, sys, os, re
ROOT = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get('LMO_LINUX_BIN', os.path.join(ROOT, '../../../clients/downloads/linux_557666/LMO'))
SYMS = os.path.join(ROOT, '../../logs/linux_syms_sized.txt')
names = {}
for line in open(SYMS):
    p = line.rstrip('\n').split(' ', 3)
    if len(p) == 4: names.setdefault(int(p[0], 16), p[3]); 
data = open(BIN, 'rb').read()
phoff, = struct.unpack_from('<Q', data, 0x20); phnum, = struct.unpack_from('<H', data, 0x38)
def off(va):
    for i in range(phnum):
        typ, _, o, v, _, fs = struct.unpack_from('<IIQQQQ', data, phoff + i * 56)
        if typ == 1 and v <= va < v + fs: return va - v + o
    raise ValueError(hex(va))
vt = sys.argv[1]
if vt.startswith('0x'): base = int(vt, 16)
else: base = next(a for a, n in names.items() if n == vt) + 0x10
for s in sys.argv[2:]:
    slot = int(s, 0); t, = struct.unpack_from('<Q', data, off(base + slot))
    print('%#x -> %x %s' % (slot, t, names.get(t, '?')))
