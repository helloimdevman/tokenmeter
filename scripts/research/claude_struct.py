# Structure-only scanner of Claude usage lines: prints key paths and type counts. Never prints string values.
# usage: python3 scripts/research/claude_struct.py $(find ~/.claude/projects -name '*.jsonl' -mtime -7)
import json, os, sys, glob, collections
def paths(o, pre=""):
    out=set()
    if isinstance(o, dict):
        for k,v in o.items():
            p=f"{pre}.{k}" if pre else k
            out.add(p+":"+type(v).__name__)
            if k in ("content","toolUseResult","text","input","snapshot","data") and pre in ("message","") and k!="data": continue
            out |= paths(v,p)
    elif isinstance(o, list) and o and pre.endswith("iterations"):
        for e in o: out |= paths(e, pre+"[]")
    return out
files=sys.argv[1:]
tc=collections.Counter(); pc=collections.Counter()
for f in files:
    for line in open(f, errors="replace"):
        try: o=json.loads(line)
        except: tc["<bad>"]+=1; continue
        if not isinstance(o, dict): continue
        t=o.get("type","<none>"); tc[t]+=1
        if isinstance(o.get("message"),dict) and "usage" in o["message"]:
            for p in paths(o): pc[(t,p)]+=1
print("types:", dict(tc))
for (t,p),n in sorted(pc.items()):
    if any(s in p for s in ("usage","model","id","requestId","sessionId","isSidechain","timestamp","cwd","version","costUSD","agentId","uuid","parentUuid","userType","entrypoint","gitBranch","slug","isApiErrorMessage","provider","role","type","stop_reason","effort","isMeta","teamName","iterations","speed","service_tier","container","context_management","requestDuration","durationMs")):
        print(f"{t}\t{p}\t{n}")
