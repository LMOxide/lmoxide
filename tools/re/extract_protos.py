"""Find serialized FileDescriptorProto blobs embedded in LMO.exe and emit .proto sources."""
import sys, re, os
from google.protobuf import descriptor_pb2
data=open(sys.argv[1],'rb').read(); outdir=sys.argv[2]; os.makedirs(outdir,exist_ok=True)
def varint(b,i):
    r=s=0
    while True:
        c=b[i]; i+=1; r|=(c&0x7f)<<s; s+=7
        if c<0x80: return r,i
found={}
for m in re.finditer(rb'\x0a([\x01-\x7f])([A-Za-z0-9_/]+\.proto)', data):
    if len(m.group(2))!=m.group(1)[0]: continue
    start=m.start()
    # try increasing lengths: parse fields until invalid
    best=None
    i=start; ok=True
    try:
        while i<len(data):
            key,j=varint(data,i); wt=key&7; fn=key>>3
            if fn==0 or fn>20 or wt not in (0,2): break
            if wt==0: _,j=varint(data,j)
            else:
                l,j=varint(data,j); j+=l
                if l>200000: break
            i=j
            fd=descriptor_pb2.FileDescriptorProto()
            try:
                fd.ParseFromString(data[start:i])
                if fd.name and (fd.message_type or fd.enum_type): best=(i,fd)
            except Exception: pass
    except IndexError: pass
    if best:
        found[best[1].name]=best[1]
print(len(found),'descriptors')
LBL={1:'optional',2:'required',3:'repeated'}
T={1:'double',2:'float',3:'int64',4:'uint64',5:'int32',6:'fixed64',7:'fixed32',8:'bool',9:'string',12:'bytes',13:'uint32',15:'sfixed32',16:'sfixed64',17:'sint32',18:'sint64'}
def fmt_msg(m,ind,pkg):
    o=[f'{ind}message {m.name} {{']
    for e in m.enum_type: o+=fmt_enum(e,ind+'  ')
    for n in m.nested_type: o+=fmt_msg(n,ind+'  ',pkg)
    for f in m.field:
        t=T.get(f.type) or f.type_name.lstrip('.')
        if pkg and t.startswith(pkg+'.'): t=t[len(pkg)+1:]
        dv=f.default_value
        if f.type in (9,12): dv='"'+dv.replace('\\','\\\\').replace('"','\\"')+'"'
        d=f' [default = {dv}]' if f.HasField('default_value') else ''
        o.append(f'{ind}  {LBL[f.label]} {t} {f.name} = {f.number}{d};')
    o.append(ind+'}'); return o
def fmt_enum(e,ind):
    return [f'{ind}enum {e.name} {{']+[f'{ind}  {v.name} = {v.number};' for v in e.value]+[ind+'}']
for name,fd in sorted(found.items()):
    if name.startswith('google/'): continue
    o=[f'syntax = "proto2";']
    if fd.package: o.append(f'package {fd.package};')
    for d in fd.dependency: o.append(f'import "{d}";')
    for e in fd.enum_type: o+=fmt_enum(e,'')
    for m in fd.message_type: o+=fmt_msg(m,'',fd.package)
    open(os.path.join(outdir,os.path.basename(name)),'w').write('\n'.join(o)+'\n')
    print(name, fd.package, [m.name for m in fd.message_type][:6])
