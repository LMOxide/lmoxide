import pefile, capstone, struct, sys, re, functools
import os
# LMO.exe: $LMO_EXE, else the extracted client next to this repo (../client/...)
EXE = os.environ.get("LMO_EXE") or os.path.join(os.path.dirname(os.path.abspath(__file__)),
    "../../../clients/windows/LEGO Minifigures Online/LMO.exe")
pe = pefile.PE(EXE, fast_load=True)
BASE = pe.OPTIONAL_HEADER.ImageBase
img = pe.get_memory_mapped_image()
text = [s for s in pe.sections if s.Name.startswith(b'.text')][0]
TS, TE = BASE+text.VirtualAddress, BASE+text.VirtualAddress+text.Misc_VirtualSize
md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32); md.detail=False
def rd(va, n): return img[va-BASE:va-BASE+n]
def u32(va): return struct.unpack('<I', rd(va,4))[0]
def find_bytes(b):
    out=[]; i=img.find(b)
    while i!=-1: out.append(BASE+i); i=img.find(b,i+1)
    return out
def find_str(s): return find_bytes(s.encode()+b'\0') if isinstance(s,str) else find_bytes(s)
def str_start(va):
    while img[va-BASE-1]!=0: va-=1
    return va
@functools.lru_cache(None)
def textbytes(): return img[TS-BASE:TE-BASE]
def xrefs(va):
    """code locations containing imm32==va"""
    b=struct.pack('<I',va); t=textbytes(); out=[]; i=t.find(b)
    while i!=-1: out.append(TS+i); i=t.find(b,i+1)
    return out
def func_start(va, maxback=0x10000):
    """nearest preceding 'push ebp; mov ebp,esp' (or sub esp/push) that follows CC/C3 padding"""
    a=va
    lo=max(TS, va-maxback)
    while a>lo:
        a-=1
        b=img[a-BASE:a-BASE+3]
        if img[a-BASE-1] in (0xCC,) and (b==b'\x55\x8b\xec' or b[:1] in (b'\x83',b'\x81',b'\x56',b'\x53',b'\x57',b'\x51',b'\x6a',b'\x8b',b'\xa1',b'\x64')):
            return a
    return None
def dis(va, n=400, stop_ret=True):
    out=[]
    code=rd(va,n)
    for ins in md.disasm(code, va):
        out.append(ins)
        if stop_ret and ins.mnemonic=='ret': break
    return out
def cstr(va, maxlen=300):
    b=rd(va,maxlen); return b.split(b'\0')[0].decode('latin1')
