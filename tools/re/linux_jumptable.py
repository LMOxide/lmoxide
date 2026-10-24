"""Resolve a switch jump table in the Linux LMO build to the first named call of each case.
    tools/re/linux_jumptable.py 0x2efb4a8 201      (table VA, number of entries)
Used for *Receiver::DispatchMethod (`jmp [reg*8+TABLE]`) to verify method ids."""
import re, struct, subprocess, sys, os
ROOT = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get('LMO_LINUX_BIN', os.path.join(ROOT, '../../../clients/downloads/linux_557666/LMO'))
table, n = int(sys.argv[1], 0), int(sys.argv[2], 0)
# map VA -> file offset via program headers
data = open(BIN, 'rb').read()
phoff, = struct.unpack_from('<Q', data, 0x20); phnum, = struct.unpack_from('<H', data, 0x38)
def off(va):
    for i in range(phnum):
        p = phoff + i * 56
        typ, _, o, v, _, fs = struct.unpack_from('<IIQQQQ', data, p)
        if typ == 1 and v <= va < v + fs: return va - v + o
    raise ValueError(hex(va))
targets = struct.unpack_from('<%dQ' % n, data, off(table))
for i, t in enumerate(targets):
    out = subprocess.run(['objdump', '-d', '-C', '--no-show-raw-insn', '-M', 'intel',
                          '--start-address=%d' % t, '--stop-address=%d' % (t + 64), BIN],
                         capture_output=True, text=True).stdout
    m = re.search(r'call\s+\w+ <([^>]*)>', out)
    print('0x%02x %x %s' % (i, t, m.group(1).replace('@@Base', '') if m else '?'))
