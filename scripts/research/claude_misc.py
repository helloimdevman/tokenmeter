# Counts assistant uuids and usage objects outside message.usage (main/subagent/journal) in Claude logs; key paths only.
# usage: python3 scripts/research/claude_misc.py $(find ~/.claude/projects -name '*.jsonl' -mtime -7)
import json, sys, collections
def kp(o, pre="", depth=0):
    out=[]
    if isinstance(o, dict) and depth<4:
        for k,v in o.items():
            p=f"{pre}.{k}" if pre else k
            out.append(p+":"+type(v).__name__); out+=kp(v,p,depth+1)
    return out
uu=collections.Counter(); c=collections.Counter(); shapes=collections.Counter()
for f in sys.argv[1:]:
    j = f.endswith("journal.jsonl"); sub = "/subagents/" in f
    for line in open(f, errors="replace"):
        try: o=json.loads(line)
        except: continue
        if not isinstance(o,dict): continue
        if o.get("type")=="assistant": uu[o.get("uuid")]+=1
        m=o.get("message")
        if '"usage"' in line and not (isinstance(m,dict) and isinstance(m.get("usage"),dict)):
            c[("usage_elsewhere", o.get("type"), "journal" if j else ("sub" if sub else "main"))]+=1
            for p in kp(o):
                if "usage" in p.lower() or "token" in p.lower() or p.count(".")<1: shapes[(o.get("type"),p)]+=1
        if o.get("type") in ("cost-state",):
            for p in kp(o): shapes[("cost-state",p)]+=1
        if o.get("type")=="assistant": c[("assistant", "sub" if sub else "main", "sidechain" if o.get("isSidechain") else "main")]+=1
print("assistant lines", sum(uu.values()), "unique uuids", len(uu))
for k,v in sorted(c.items(), key=str): print(k,v)
for k,v in sorted(shapes.items(), key=str): print(k,v)
