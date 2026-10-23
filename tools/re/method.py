"""Look up RPC methods by name, signature substring, or ID, and locate them in LMO.exe.

    method.py GetStartupData
    method.py 0x5dc18991
    method.py PlayerInterface::            (every method of an interface)

For server->client methods (`*Interface`, `GameserverToClientRPC`) it prints the client's
Dispatch function (which deserializes the arguments) and the argument read order taken from
its "stream bad after '<arg>'" error strings. For client->server methods (`*Agent`) it prints
the Sender function that serializes them."""
import json, os, re, sys
from pe import find_str, str_start, xrefs, func_start, cstr, img, BASE

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = json.load(open(os.path.join(HERE, '../../data/rpc_methods.json')))

def strings_with(prefix):
    """(va, text) for NUL-terminated strings starting with `prefix`."""
    out = []
    b = prefix.encode()
    i = img.find(b)
    while i != -1:
        if img[i - 1] == 0:
            out.append((BASE + i, cstr(BASE + i, 600)))
        i = img.find(b, i + 1)
    return out

def funcs_referencing(va):
    return sorted({func_start(x) for x in xrefs(va)} - {None})

def describe(section, cls, mid, sig):
    iface = cls[:-len('Receiver')] if section == 'receivers' else cls[:-len('Sender')]
    name = sig.split('(')[0].strip()
    direction = 'server->client' if section == 'receivers' else 'client->server'
    print(f'{iface}::{sig}')
    print(f'  id {mid}  ({direction})')
    if section == 'receivers':
        bad = strings_with(f'{cls}::Dispatch{name}')
        args = [re.search(r"after '([^']+)'", s).group(1) for _, s in bad if "after '" in s]
        funcs = sorted({f for va, _ in bad for f in funcs_referencing(va)})
        if not funcs:  # no arguments -> no error strings; fall back to the payload log string
            funcs = sorted({f for va, _ in strings_with(f'{cls}::{name}(') for f in funcs_referencing(va)})
        print(f'  dispatch fn: {", ".join(hex(f) for f in funcs) or "?"}')
        if args:
            print(f'  read order:  {", ".join(args)}')
    else:
        funcs = sorted({f for va, _ in strings_with(f'{cls}::{name}(') for f in funcs_referencing(va)})
        print(f'  sender fn:   {", ".join(hex(f) for f in funcs) or "?"}')

def main():
    q = sys.argv[1]
    want_id = int(q, 16) if re.fullmatch(r'0x[0-9a-fA-F]+', q) else (int(q) if q.isdigit() else None)
    iface_q, _, meth_q = q.partition('::') if '::' in q else ('', '', q)
    n = 0
    for section in ('receivers', 'senders'):
        for cls, table in DATA[section].items():
            iface = re.sub(r'(Receiver|Sender)$', '', cls)
            for k, sig in table.items():
                mid = int(k, 16) if k.startswith('0x') else int(k)
                if want_id is not None:
                    hit = mid == want_id
                elif iface_q:
                    hit = iface == iface_q and meth_q.lower() in sig.lower()
                else:
                    hit = q.lower() in sig.lower()
                if hit:
                    describe(section, cls, f'0x{mid:08x}', sig)
                    n += 1
    if not n:
        print('no match', file=sys.stderr)
        sys.exit(1)

if __name__ == '__main__':
    main()
