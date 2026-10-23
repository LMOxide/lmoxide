from pe import *
va=int(sys.argv[1],16); n=int(sys.argv[2]) if len(sys.argv)>2 else 300
for ins in dis(va, n, stop_ret=False):
    extra=''
    for m in re.findall(r'0x[0-9a-f]{6,8}', ins.op_str):
        v=int(m,16)
        if BASE<=v<BASE+len(img):
            try:
                s=cstr(v,80)
                if len(s)>3 and s.isprintable(): extra+=' ; "'+s+'"'
            except: pass
    print(f'{ins.address:08x}  {ins.mnemonic:6} {ins.op_str}{extra}')
