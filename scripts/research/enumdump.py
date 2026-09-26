# Dump protobuf EnumDescriptorProto value tables embedded in a binary.
# usage: python3 scripts/research/enumdump.py <binary> <EnumName> [value-regex]
import sys, re
def varint(b,i):
    r=s=0
    while True:
        x=b[i]; i+=1; r|=(x&0x7f)<<s; s+=7
        if not x&0x80: return r,i
        if s>63: raise ValueError
def fields(b,i,end):
    while i<end:
        t,i=varint(b,i); fn,wt=t>>3,t&7
        if wt==0: v,i=varint(b,i); yield fn,v
        elif wt==2:
            l,i=varint(b,i)
            if i+l>end: raise ValueError
            yield fn,bytes(b[i:i+l]); i+=l
        elif wt==1: i+=8
        elif wt==5: i+=4
        else: raise ValueError
def sint(v): return v-(1<<64) if v>=1<<63 else v
b=open(sys.argv[1],'rb').read()
name=sys.argv[2].encode(); want=re.compile(sys.argv[3]) if len(sys.argv)>3 else None
pat=b'\x0a'+bytes([len(name)])+name; start=0
while True:
    p=b.find(pat,start)
    if p<0: break
    start=p+1
    for back in range(2,6):
        q=p-back
        if b[q] not in (0x2a,0x22): continue
        try: l,i=varint(b,q+1)
        except Exception: continue
        if i!=p: continue
        try:
            vals=[]
            for fn,v in fields(b,p,p+l):
                if fn==2 and isinstance(v,bytes):
                    d=dict(fields(v,0,len(v))); vals.append((sint(d.get(2,0)),d.get(1,b'').decode()))
            if vals:
                print(f'## enum {name.decode()} @{p}: {len(vals)} values')
                for n,s in sorted(vals):
                    if want is None or want.search(s) or want.search(str(n)): print('  ',n,s)
        except Exception: pass
