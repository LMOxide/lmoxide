import struct
def varint(b,o):
    v=0;s=0
    while True:
        c=b[o];o+=1;v|=(c&0x7f)<<s;s+=7
        if c<0x80: return v,o
def parse(b):
    o=0;out=[]
    while o<len(b):
        k,o=varint(b,o);f,w=k>>3,k&7
        if w==0: v,o=varint(b,o)
        elif w==1: v=b[o:o+8];o+=8
        elif w==5: v=b[o:o+4];o+=4
        elif w==2: n,o=varint(b,o);v=b[o:o+n];o+=n
        else: raise ValueError(w)
        if o>len(b): raise ValueError('trunc')
        out.append((f,w,v))
    return out
def demo_events(b):
    o=0; ev=[]
    while o+10<=len(b):
        t=struct.unpack_from('<H',b,o)[0]; hdr=b[o+2:o+10]; p=o+10; st=None
        if t in (0,7,8,10,11,12):
            n=struct.unpack_from('<I',b,p)[0]; pay=b[p+4:p+4+n]; p+=4+n
        elif t in (1,2): st=b[p]; pay=b''; p+=1
        elif t in (3,4): st=b[p]; n=struct.unpack_from('<I',b,p+1)[0]; pay=b[p+5:p+5+n]; p+=5+n
        elif t==5: pay=b[p:p+4]; p+=4
        elif t==6: pay=b[p:p+8]; p+=8
        elif t==9: pay=b''
        elif t==0x64: pay=b[p:p+17]; p+=17
        elif t==0x65: pay=b[p:p+4]; p+=4
        elif t==0x66: pay=b[p:p+28]; p+=28
        else: raise ValueError((t,o))
        ev.append((o,t,hdr,st,pay)); o=p
    return ev
