# split every PassBlob (both directions) into IIR records by scanning for known type ids
import sys, struct, json, pickle
R=__import__('os').path.join(__import__('os').path.dirname(__import__('os').path.abspath(__file__)), '../../../')
iir={int(e['type_id'],16):e['name'] for e in json.load(open(R+'data/iir_types.json'))}
# also UpdateMessage etc. may be missing; add by java hash
def jh(s):
    h=0
    for c in s: h=(h*31+ord(c))&0xffffffff
    return h
for n in ['UpdateMessage']: iir.setdefault(jh(n),n)
t=pickle.load(open(sys.argv[1]+'/tl.pkl','rb'))
out=[]
for e in t:
    if e['meth'] not in ('PassBlob','0x3'): continue
    a=e['args']; n=struct.unpack_from('>I',a,0)[0]; b=a[4:4+n]
    starts=[]
    for k in range(0,len(b)-12):
        ty=struct.unpack_from('>I',b,k)[0]
        if ty in iir and b[k+12]<=1:
            it=struct.unpack_from('>I',b,k+4)[0]
            if 0<it<200000 or it==0:
                starts.append(k)
    # drop starts that lie inside a previous record's first 13 bytes
    clean=[]
    for k in starts:
        if clean and k<clean[-1]+13: continue
        clean.append(k)
    for j,k in enumerate(clean):
        end=clean[j+1] if j+1<len(clean) else len(b)
        ty,it,ii=struct.unpack_from('>III',b,k)
        out.append(dict(i=e['i'],ms=e['ms'],dir=e['dir'],name=iir[ty],id=(it,ii),flag=b[k+12],body=b[k+13:end],lead=b[:clean[0]] if j==0 else b''))
pickle.dump(out,open(sys.argv[1]+'/recs.pkl','wb'))
from collections import Counter
print(Counter((r['dir'],r['name']) for r in out).most_common(100))
