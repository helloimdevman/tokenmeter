# Dump protobuf DescriptorProto field tables embedded in a binary (Go registers raw descriptors).
# usage: python3 scripts/research/descdump.py <binary> <MessageName>...
import sys, mmap
TYPES={1:'double',2:'float',3:'int64',4:'uint64',5:'int32',8:'bool',9:'string',11:'message',12:'bytes',13:'uint32',14:'enum',16:'sint32',17:'sint64',18:'sfixed32',6:'fixed64',7:'fixed32',15:'sfixed32'}
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
            if i+l>end: raise ValueError('len')
            yield fn,bytes(b[i:i+l]); i+=l
        elif wt==1: i+=8
        elif wt==5: i+=4
        else: raise ValueError('wt')
def dump(b,name):
    nb=name.encode(); pat=b'\x0a'+bytes([len(nb)])+nb
    start=0; found=0
    while True:
        p=b.find(pat,start)
        if p<0: break
        start=p+1
        # DescriptorProto begins at p; find length from preceding tag 0x22/0x1a(nested) + varint len
        for back in range(2,6):
            q=p-back
            if b[q] not in (0x22,0x1a): continue
            try:
                l,i=varint(b,q+1)
            except Exception: continue
            if i!=p: continue
            try:
                fl=[]
                for fn,v in fields(b,p,p+l):
                    if fn==2 and isinstance(v,bytes):
                        d=dict((k,x) for k,x in fields(v,0,len(v)))
                        fl.append((d.get(3),d.get(1,b'').decode(),TYPES.get(d.get(5),d.get(5)),(d.get(6) or b'').decode()))
                if fl:
                    found+=1
                    print(f'## {name} (offset {p})')
                    for f in sorted(fl): print('  ',f)
            except Exception as e:
                pass
    if not found: print(f'## {name}: not found')
b=open(sys.argv[1],'rb').read()
for n in sys.argv[2:]: dump(b,n)
