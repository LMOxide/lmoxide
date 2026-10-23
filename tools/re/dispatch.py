from pe import *
name=sys.argv[1]
for s in find_str(name):
    s0=str_start(s)
    print('str',hex(s0),cstr(s0))
    for x in xrefs(s0):
        print(' xref',hex(x),'func',hex(func_start(x) or 0))
