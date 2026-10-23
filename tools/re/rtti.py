"""MSVC RTTI helpers: class name for a vtable, and vtables written by a constructor."""
import re
from pe import *

def vtable_class(vt):
    """vtable[-1] -> CompleteObjectLocator -> +0xc TypeDescriptor -> +8 mangled name."""
    try:
        col = u32(vt - 4)
        td = u32(col + 12)
        name = cstr(td + 8, 200)
    except Exception:
        return None
    m = re.match(r'^\.\?A[VU](.+)@@$', name)
    return m.group(1) if m else None

def demangle_simple(n):
    """'Foo@ns@@' style nested names -> ns::Foo (templates left alone)."""
    if n is None or '?$' in n:
        return n
    return '::'.join(reversed([p for p in n.split('@') if p]))

def ctor_vtables(fn, maxlen=0x600):
    """Classes of vtables stored by `mov dword ptr [reg], imm32` in a function."""
    out = []
    for ins in md.disasm(rd(fn, maxlen), fn):
        m = re.match(r'^dword ptr \[e[a-ds][xip]\], (0x[0-9a-f]+)$', ins.op_str)
        if ins.mnemonic == 'mov' and m:
            c = vtable_class(int(m.group(1), 16))
            if c: out.append(demangle_simple(c))
        if ins.mnemonic == 'ret':
            break
    return out

def factory_classes(factory):
    """IIR factories: alloc, then call the constructor (2nd direct call). Returns the classes
    of all vtables the constructor stores (the object first, then members)."""
    calls = []
    for ins in md.disasm(rd(factory, 0x80), factory):
        if ins.mnemonic == 'call' and ins.op_str.startswith('0x'):
            calls.append(int(ins.op_str, 16))
        if ins.mnemonic == 'ret':
            break
    for c in calls[1:]:
        v = ctor_vtables(c)
        if v: return v
    return []
