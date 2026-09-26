# Simulates TokenMeter engine variants (keys, delta/cumulative, ccusage max) on local Claude logs; prints totals only.
# usage: python3 scripts/research/claude_sim.py $(find ~/.claude/projects -name '*.jsonl' -mtime -7)
import json, sys, collections
files=sorted(sys.argv[1:])
def V(u): return [(u.get(k) or 0) for k in ("input_tokens","cache_read_input_tokens","cache_creation_input_tokens","output_tokens")]
seen_uuid=set(); seen_mid=set(); seen_comp=set()
A=B=C=D=0
base=collections.defaultdict(dict)   # file -> mid -> vec (cumulative, per file)
gmax={}                               # (mid,rid) -> vec   (ccusage/tokscale max)
svc=dict()                            # (mid|rid) -> vec   (proposed service-scope replace: emit positive diff)
E=0
for f in files:
    for line in open(f, errors="replace"):
        if '"usage"' not in line: continue
        try:o=json.loads(line)
        except: continue
        if o.get("type")!="assistant": continue
        m=o.get("message") or {}; u=m.get("usage")
        if not isinstance(u,dict): continue
        v=V(u); s=sum(v)
        if s<=0: continue
        uid=o.get("uuid"); mid=m.get("id"); rid=o.get("requestId")
        # A: current spec (delta, key uuid, global first-wins)
        if uid not in seen_uuid: seen_uuid.add(uid); A+=s
        # B: delta, key message.id (first-wins global)
        if mid not in seen_mid: seen_mid.add(mid); B+=s
        # C: cumulative, key message.id (per-file baseline)
        prev=base[f].get(mid); base[f][mid]=v
        if prev is None: C+=s
        else:
            d=[a-b for a,b in zip(v,prev)]
            if all(x>=0 for x in d): C+=sum(d)
        # D: ccusage-like: max total per (mid,rid)
        k=(mid,rid)
        if k not in gmax or s>sum(gmax[k]): gmax[k]=v
        # E: proposed replace-on-duplicate (service scope, per-field max, emit diff)
        p=svc.get(k)
        if p is None: svc[k]=v; E+=s
        else:
            n=[max(a,b) for a,b in zip(v,p)]; E+=sum(n)-sum(p); svc[k]=n
D=sum(sum(v) for v in gmax.values())
print("A current(uuid) %d\nB delta message.id %d\nC cumulative message.id %d\nD ccusage max %d\nE proposed %d" % (A,B,C,D,E))
print("ratios vs D: A %.3f B %.4f C %.4f E %.4f" % (A/D,B/D,C/D,E/D))
