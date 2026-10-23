from pe import *
f=int(sys.argv[1],16)
for loc in find_bytes(struct.pack('<I',f)):
    if TS<=loc<TE: continue
    # walk back to vtable start: previous dword is RTTI COL pointer (non-text)
    a=loc
    while TS<=u32(a-4)<TE: a-=4
    print('ref at',hex(loc),'vtable start',hex(a),'COL',hex(u32(a-4)))
    for i in range(12): print('  [%d] %x'%(i,u32(a+4*i)))
