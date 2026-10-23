"""Regex search over LMO.exe strings, with the functions that reference each match.

    xref.py 'login problem'
    xref.py -w 'Patch'        (UTF-16 strings too)
    xref.py -n 20 'e_\\w+'    (limit matches)
"""
import argparse, re
from pe import img, BASE, xrefs, func_start

ap = argparse.ArgumentParser()
ap.add_argument('pattern')
ap.add_argument('-w', action='store_true', help='also search UTF-16LE strings')
ap.add_argument('-n', type=int, default=50, help='max matches')
a = ap.parse_args()
rx = re.compile(a.pattern)

def ascii_strings():
    for m in re.finditer(rb'[\x20-\x7e\t]{4,}', img):
        yield BASE + m.start(), m.group().decode('latin1')

def wide_strings():
    for m in re.finditer(rb'(?:[\x20-\x7e]\x00){4,}', img):
        yield BASE + m.start(), m.group().decode('utf-16le')

shown = 0
for gen in [ascii_strings] + ([wide_strings] if a.w else []):
    for va, s in gen():
        if not rx.search(s):
            continue
        refs = xrefs(va)
        funcs = sorted({func_start(x) for x in refs} - {None})
        where = ', '.join(hex(f) for f in funcs[:8]) + (' ...' if len(funcs) > 8 else '')
        print(f'{va:08x}  {s[:140]!r}')
        print(f'          refs: {where or "-"}')
        shown += 1
        if shown >= a.n:
            raise SystemExit
