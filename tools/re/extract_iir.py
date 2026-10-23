"""List IIR classes registered with the n3 IIR factory (0x7d63b0, fastcall: ecx=name, edx=factory).
The type id on the wire is the Java-style hash (h*31+c) of the name.  Output: JSON list."""
import json, re, struct, sys
from pe import *
from rtti import factory_classes

REGISTER = 0x7d63b0

def jhash(s):
    h = 0
    for c in s.encode():
        h = (h * 31 + (c if c < 128 else c - 256)) & 0xffffffff  # chars are signed
    return h

out = []
t = textbytes()
for m in re.finditer(b'\xe8', t):
    a = TS + m.start()
    rel = struct.unpack('<i', t[m.start() + 1:m.start() + 5])[0]
    if a + 5 + rel != REGISTER:
        continue
    # registration sites are `mov edx, factory; mov ecx, name; call` (BA imm32, B9 imm32)
    name = factory = None
    pre = rd(a - 10, 10)
    for off in (0, 5):
        op, v = pre[off], struct.unpack('<I', pre[off + 1:off + 5])[0]
        if op == 0xB9:
            try: name = cstr(v, 100)
            except Exception: pass
        elif op == 0xBA:
            factory = v
    if name:
        classes = factory_classes(factory) if factory else []
        protos = [c for c in classes if 'Proto' in c or 'Message' in c]
        others = [c for c in classes if c not in protos]
        out.append({'name': name, 'type_id': f'0x{jhash(name):08x}',
                    'class': others[0] if others else None,
                    'proto': next((p for p in protos if p != 'ProtoEmptyMessage'), None),
                    'factory': hex(factory) if factory else None, 'site': hex(a)})
out.sort(key=lambda e: e['name'])
json.dump(out, open(sys.argv[1], 'w') if len(sys.argv) > 1 else sys.stdout, indent=1)
print(f'{len(out)} IIRs', file=sys.stderr)
