# Counts Korean string literals in non-test Rust code per file (tr keys, println!, format!, other).
# usage: python3 scripts/research/ko_scan.py native
import re,sys,os,glob,collections
HANGUL=re.compile(r'[가-힣]')
def lits(src):
    # yields (lineno, literal, line_text) for non-comment string literals; handles r"..." and r#"..."#
    out=[];i=0;n=len(src);line=1
    while i<n:
        c=src[i]
        if c=='\n': line+=1;i+=1;continue
        if src.startswith('//',i):
            j=src.find('\n',i); i=n if j<0 else j; continue
        if src.startswith('/*',i):
            j=src.find('*/',i); line+=src[i:j].count('\n'); i=j+2; continue
        m=re.match(r'r(#*)"',src[i:i+10])
        if m and (i==0 or not (src[i-1].isalnum() or src[i-1]=='_')):
            end='"'+m.group(1); j=src.find(end,i+len(m.group(0)))
            s=src[i+len(m.group(0)):j]; out.append((line,s)); line+=s.count('\n'); i=j+len(end); continue
        if c=="'" :
            m2=re.match(r"'(\\.|[^\\'])'",src[i:i+12])
            if m2: i+=len(m2.group(0)); continue
            i+=1; continue
        if c=='"':
            j=i+1
            while j<n and src[j]!='"':
                if src[j]=='\\': j+=1
                j+=1
            s=src[i+1:j]; out.append((line,s)); line+=s.count('\n'); i=j+1; continue
        i+=1
    return out
i18n=open(sys.argv[1]+'/meter/src/i18n.rs').read()
keys=set(re.findall(r'^\s*"([^"]+)" =>',i18n,re.M))
rows=[]
tot=collections.Counter()
for f in sorted(glob.glob(sys.argv[1]+'/meter/src/**/*.rs',recursive=True)+glob.glob(sys.argv[1]+'/hook/src/*.rs')):
    if f.endswith('i18n.rs'): continue
    src=open(f).read()
    m=re.search(r'#\[cfg\(test\)\]\s*mod tests',src)
    body=src[:m.start()] if m else src
    lines=body.split('\n')
    c=collections.Counter()
    for ln,s in lits(body):
        if not HANGUL.search(s): continue
        ctx=' '.join(lines[max(0,ln-3):ln])
        c['ko']+=1
        if s in keys: c['keyed']+=1
        elif re.search(r'\b(e?println!|e?print!)\s*\(',ctx): c['stdio']+=1
        elif re.search(r'\bformat!\s*\(',lines[ln-1]): c['format']+=1
        else: c['other']+=1
    if c['ko']:
        rows.append((os.path.relpath(f,sys.argv[1]),c)); tot.update(c)
print(f"{'file':34} ko keyed stdio format other")
for f,c in sorted(rows,key=lambda x:-x[1]['ko']): print(f"{f:34} {c['ko']:3} {c['keyed']:5} {c['stdio']:5} {c['format']:6} {c['other']:5}")
print(f"{'TOTAL':34} {tot['ko']:3} {tot['keyed']:5} {tot['stdio']:5} {tot['format']:6} {tot['other']:5}")
print('i18n keys',len(keys))
