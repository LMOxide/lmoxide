# demo timeline: every RPC, both directions, with method names; IIR record types in PassBlobs
import sys, struct, json, pickle
sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from pblib import parse, demo_events
R='/home/dgmastertemple/Dropbox/Development/projects/lego-minifigures-online/lmoxide-server/'
CL='/home/dgmastertemple/Dropbox/Development/projects/lego-minifigures-online/client/LEGO Minifigures Online/Data/default_windowsx86.demo'
rm=json.load(open(R+'data/rpc_methods.json'))
def jh(s):
    h=0
    for c in s: h=(h*31+ord(c))&0xffffffff
    return h
names={}
for side in ('receivers','senders'):
    for iface,ms in rm[side].items():
        base=iface.replace('Receiver','').replace('Sender','')
        names[jh(base)]=(base,ms)
iir={int(e['type_id'],16):e['name'] for e in json.load(open(R+'data/iir_types.json'))}
ev=demo_events(open(CL,'rb').read())
out=[]
for i,(o,t,hdr,st,pay) in enumerate(ev):
    if t not in (3,4): continue
    ms=struct.unpack_from('<I',hdr,4)[0]
    hl=struct.unpack_from('>I',pay,0)[0]
    h={f:v for f,_,v in parse(pay[4:4+hl])}
    ep={f:v for f,_,v in parse(h[2])}
    typ=struct.unpack('<I',ep[1])[0]
    meth=h[4]
    nm,mt=names.get(typ,(hex(typ),{}))
    m=mt.get('0x%08x'%meth,hex(meth))
    args=pay[4+hl:]
    out.append(dict(i=i,ms=ms,dir='C>S' if t==3 else 'S>C',st=st,iface=nm,meth=m.split('(')[0],args=args,inst=ep.get(2)))
pickle.dump(out,open(sys.argv[1]+'/tl.pkl','wb'))
print(len(out))
