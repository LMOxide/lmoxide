"""Extract RPC method tables from LMO.exe.
Receivers (server->client): DispatchMethod switch on 32-bit method IDs.
Senders (client->server): method index passed to Proxy::SendMethodInvocation via vtable[0]."""
from pe import *
import json, collections

import subprocess
STR_LIST = subprocess.run(['strings', '-n', '5', EXE], capture_output=True, text=True,
                          encoding='latin1').stdout.splitlines()

def funcs_end(start, maxlen=0x4000):
    """disassemble until a ret followed by CC padding"""
    out=[]
    for ins in md.disasm(rd(start,maxlen), start):
        out.append(ins)
        if ins.mnemonic=='ret':
            nxt=img[ins.address+ins.size-BASE]
            if nxt==0xCC: break
    return out

def strrefs_in(insns):
    res=[]
    for ins in insns:
        for m in re.findall(r'0x[0-9a-f]{6,8}', ins.op_str):
            v=int(m,16)
            if BASE<=v<BASE+len(img) and not (TS<=v<TE):
                try:
                    s=cstr(v,400)
                except Exception: continue
                if len(s)>4: res.append(s)
    return res

def sig_of(s, cls):
    s=s.split(cls+'::',1)[1]
    return s.replace(' streamed payload of size','').strip()

def receiver_table(cls):
    key=cls+'::DispatchMethod('
    tbl={}
    for s in find_str(key):
        for x in xrefs(str_start(s)):
            f=func_start(x)
            insns=funcs_end(f, 0x8000)
            addrs={i.address:k for k,i in enumerate(insns)}
            cands=[]  # (method_id, target)
            for k,ins in enumerate(insns):
                m=re.match(r'^dword ptr \[e[a-ds][xi]\*4 \+ (0x[0-9a-f]+)\]$', ins.op_str)
                if ins.mnemonic=='jmp' and m:
                    T=int(m.group(1),16)
                    # find bound from preceding cmp
                    n=None
                    for j in reversed(insns[max(0,k-3):k]):
                        mm=re.match(r'^e[a-ds][xi], (0x[0-9a-f]+|\d+)$', j.op_str)
                        if j.mnemonic=='cmp' and mm: n=int(mm.group(1),0); break
                    if n is not None:
                        for i in range(n+1): cands.append((i, u32(T+4*i)))
            for k,ins in enumerate(insns):
                if ins.mnemonic=='cmp' and re.match(r'^e[a-ds][xi], (0x[0-9a-f]+|\d+)$',ins.op_str):
                    mid=int(ins.op_str.split(', ')[1],0)
                    target=None
                    for j in insns[k+1:k+4]:
                        if j.mnemonic=='je': target=int(j.op_str,16); break
                        if j.mnemonic=='jne': target=j.address+j.size; break
                        if not j.mnemonic.startswith('j'): break
                    if target is None: continue
                    cands.append((mid,target))
            for mid,target in cands:
                if True:
                    disp=None
                    for t in md.disasm(rd(target,48), target):
                        if t.mnemonic in ('ret','jmp'): break
                        if t.mnemonic=='call':
                            try: disp=int(t.op_str,16)
                            except: pass
                            break
                    if disp is None: continue
                    strs=strrefs_in(funcs_end(disp))
                    if not any(st.startswith(cls+'::') for st in strs): continue
                    sig=None; name=None
                    for st in strs:
                        if st.startswith(cls+'::') and 'streamed payload' in st: sig=sig_of(st,cls); break
                    if not sig:
                        for st in strs:
                            m=re.match(re.escape(cls)+r'::Dispatch(\w+) ?\(\)',st)
                            if m: name=m.group(1); break
                    tbl[mid & 0xffffffff]=sig or (name and name+'(?)') or f'?disp@{disp:x}'
    return tbl

def sender_table(cls):
    """find each Sender method func by its 'streamed payload' string, then the push imm before vtable[0] call"""
    tbl={}
    for line in STR_LIST:
        if not (line.startswith(cls+'::') and 'streamed payload of size' in line): continue
        for s in find_str(line+'\0'[:0]) or find_str(line):
            for x in xrefs(str_start(s)):
                # instruction containing the imm starts 1 byte before (push imm32)
                insns=funcs_end(x-1, 0x3000)
                idx=None
                for k,ins in enumerate(insns):
                    if ins.mnemonic=='call' and ins.op_str=='dword ptr [eax]':
                        for j in reversed(insns[max(0,k-6):k]):
                            if j.mnemonic=='push' and re.match(r'^(0x[0-9a-f]+|\d+)$', j.op_str):
                                idx=int(j.op_str,0); break
                        if idx is not None: break
                if idx is not None: tbl[idx]=sig_of(line,cls)
    return tbl

if __name__=='__main__':
    receivers=sorted({l.split('::')[0] for l in STR_LIST if re.match(r'^\w+Receiver::DispatchMethod\(',l)})
    senders=sorted({l.split('::')[0] for l in STR_LIST if re.match(r'^\w+Sender::\w+.*streamed payload',l)})
    out={'receivers':{}, 'senders':{}}
    for c in receivers:
        t=receiver_table(c); out['receivers'][c]={f'0x{k:08x}':v for k,v in sorted(t.items())}
        print(f'{c}: {len(t)} methods', file=sys.stderr)
    for c in senders:
        t=sender_table(c); out['senders'][c]={str(k):v for k,v in sorted(t.items())}
        print(f'{c}: {len(t)} methods', file=sys.stderr)
    json.dump(out, open(sys.argv[1] if len(sys.argv)>1 else 'rpc_methods.json','w'), indent=1)
