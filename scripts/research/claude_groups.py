# Counts-only analysis of Claude transcript usage duplication. No string values printed.
# usage: python3 scripts/research/claude_groups.py $(find ~/.claude/projects -name '*.jsonl' -mtime -7)
import json, sys, collections
files=sys.argv[1:]
groups=collections.OrderedDict()  # (mid,rid) -> list of (file_idx, line_no, vec, stop, sidechain, sess)
stats=collections.Counter(); itypes=collections.Counter()
def vec(u):
    return (u.get("input_tokens",0) or 0, u.get("cache_read_input_tokens",0) or 0, u.get("cache_creation_input_tokens",0) or 0, u.get("output_tokens",0) or 0)
for fi,f in enumerate(files):
    for ln,line in enumerate(open(f, errors="replace")):
        if '"usage"' not in line: continue
        try: o=json.loads(line)
        except: continue
        t=o.get("type")
        if t=="progress" or (isinstance(o.get("data"),dict) and isinstance(o["data"].get("message"),dict)):
            stats["progress_like_with_usage"]+=1
        m=o.get("message")
        if not isinstance(m,dict) or not isinstance(m.get("usage"),dict):
            stats["usage_not_in_message"]+=1; continue
        u=m["usage"]; stats["lines_type_"+str(t)]+=1
        if t!="assistant": continue
        if not o.get("requestId"): stats["no_requestId"]+=1
        if not m.get("id"): stats["no_message_id"]+=1
        if m.get("model")=="<synthetic>": stats["synthetic"]+=1
        cc=u.get("cache_creation")
        if isinstance(cc,dict):
            s=(cc.get("ephemeral_5m_input_tokens",0) or 0)+(cc.get("ephemeral_1h_input_tokens",0) or 0)
            stats["cc_obj_eq" if s==(u.get("cache_creation_input_tokens") or 0) else "cc_obj_ne"]+=1
        otd=u.get("output_tokens_details")
        if isinstance(otd,dict) and otd.get("thinking_tokens") is not None:
            stats["thinking_le_output" if otd["thinking_tokens"]<= (u.get("output_tokens") or 0) else "thinking_gt_output"]+=1
            if otd["thinking_tokens"]>0: stats["thinking_nonzero"]+=1
        its=u.get("iterations")
        if isinstance(its,list):
            for it in its: itypes[it.get("type")]+=1
            if its:
                sv=[0,0,0,0]
                for it in its:
                    v=vec(it); sv=[a+b for a,b in zip(sv,v)]
                stats["iter_sum_eq_top" if tuple(sv)==vec(u) else "iter_sum_ne_top"]+=1
                if len(its)>1: stats["iter_len_gt1"]+=1
        key=(m.get("id"),o.get("requestId"))
        groups.setdefault(key,[]).append((fi,ln,vec(u),m.get("stop_reason"),bool(o.get("isSidechain")),o.get("sessionId")))
S=lambda v:sum(v)
tot_lines=tot_max=tot_first=tot_last=tot_fieldmax=0
multi=grow=lastnotmax=firstisfinal=crossfile=0
for k,rows in groups.items():
    vs=[r[2] for r in rows]
    tot_lines+=sum(S(v) for v in vs); tot_first+=S(vs[0]); tot_last+=S(vs[-1])
    tot_max+=max(S(v) for v in vs)
    fm=tuple(max(v[i] for v in vs) for i in range(4)); tot_fieldmax+=S(fm)
    if len(rows)>1:
        multi+=1
        if len(set(vs))>1: grow+=1
        if S(vs[-1])!=max(S(v) for v in vs): lastnotmax+=1
        if S(vs[0])==max(S(v) for v in vs): firstisfinal+=1
        if len(set(r[0] for r in rows))>1: crossfile+=1
    # which fields change
    if len(set(vs))>1:
        for i,name in enumerate(["in","cr","cw","out"]):
            if len(set(v[i] for v in vs))>1: stats["field_varies_"+name]+=1
# sidechain replay: same message id, different requestId
bymid=collections.defaultdict(set)
for (mid,rid) in groups: bymid[mid].add(rid)
stats["mid_with_multiple_rids"]=sum(1 for s in bymid.values() if len(s)>1)
print("files",len(files),"groups",len(groups),"multi_line_groups",multi,"groups_usage_varies",grow,"last_not_max",lastnotmax,"first_is_max",firstisfinal,"cross_file_groups",crossfile)
print("sum_lines",tot_lines,"sum_first",tot_first,"sum_last",tot_last,"sum_max",tot_max,"sum_fieldmax",tot_fieldmax)
print("ratio lines/max %.3f first/max %.4f last/max %.4f" % (tot_lines/tot_max, tot_first/tot_max, tot_last/tot_max))
print(dict(stats)); print("iteration types", dict(itypes))
