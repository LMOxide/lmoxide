"""Extract the stat id -> name table built by the client's static initializer (0x1607bd0):
each entry is `mov [entry], id; mov [entry+4], 0; push "Name"; mov ecx, entry+8; call ...`."""
import json, re, sys
from pe import *

START = 0x1607bd0
out = {}
movs = []  # (address operand, imm) of recent `mov dword ptr [abs], imm`
for ins in md.disasm(rd(START, 0x40000), START):
    m = re.match(r'^dword ptr \[(0x[0-9a-f]+)\], (0x[0-9a-f]+|\d+)$', ins.op_str)
    if ins.mnemonic == 'mov' and m:
        movs.append((int(m.group(1), 16), int(m.group(2), 0)))
        movs = movs[-2:]
    elif ins.mnemonic == 'push' and re.match(r'^0x[0-9a-f]{7}$', ins.op_str):
        try:
            s = cstr(int(ins.op_str, 16), 80)
        except Exception:
            s = ''
        # entry layout: [entry] = id, [entry+4] = 0, then the name
        if len(movs) == 2 and movs[1][0] == movs[0][0] + 4 and s and s.replace('_', '').isalnum():
            out[movs[0][1]] = s
        movs = []
    elif ins.mnemonic == 'ret':
        break
json.dump({str(k): v for k, v in sorted(out.items())}, open(sys.argv[1], 'w'), indent=1)
print(len(out), 'stats', file=sys.stderr)
