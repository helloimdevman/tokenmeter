# Grok updates.jsonl structure: kinds, meta and usage keys, eventId reuse, usage.json vs turn sums. Strings redacted.
# usage: python3 scripts/research/grok_probe.py   (reads ~/.grok/sessions)
import json, glob, os, collections, sys
sys.path.insert(0, os.path.dirname(__file__)); from shape import shape
home = os.path.expanduser("~/.grok/sessions")
files = glob.glob(home + "/*/*/updates.jsonl")
kinds = collections.Counter(); usage_kinds = collections.Counter(); meta_keys = collections.Counter()
top_keys = collections.Counter(); usage_keys = collections.Counter(); mu_keys = collections.Counter()
nmodels = collections.Counter(); ev_dupe_in_file = 0; ev_total = 0; ev_global = collections.Counter()
methods = collections.Counter(); tc_no_event = 0; tc_count = 0; ts_types = collections.Counter()
cost_present = 0; cc_nonzero = 0; top_vs_mu_mismatch = 0; mu_sum_vs_top = 0
tot_eq_in_out = 0; tot_ne = 0; reasoning_gt_out = 0; cached_gt_in = 0
agent_ms = 0; sample = None; upd_meta = collections.Counter(); prompt_id_upd = 0
primary_in_mu = 0; primary_checked = 0; usage_json_vs_updates = []
for f in files:
    evs = collections.Counter()
    tc_sum = collections.Counter()
    for line in open(f, errors="replace"):
        try: r = json.loads(line)
        except Exception: continue
        top_keys.update(r.keys()); methods[r.get("method")] += 1
        p = r.get("params") or {}
        u = p.get("update") or {}
        k = u.get("sessionUpdate"); kinds[k] += 1
        if isinstance(u.get("_meta"), dict): upd_meta.update("update._meta." + x for x in u["_meta"].keys())
        m = p.get("_meta") or {}
        meta_keys.update(m.keys())
        if "usage" in u: usage_kinds[k] += 1
        if k == "turn_completed":
            tc_count += 1
            if "prompt_id" in u: prompt_id_upd += 1
            ts_types[type(r.get("timestamp")).__name__] += 1
            if "agentTimestampMs" in m: agent_ms += 1
            e = m.get("eventId")
            if e is None: tc_no_event += 1
            else: evs[e] += 1; ev_global[e] += 1
            us = u.get("usage") or {}
            usage_keys.update(us.keys())
            if "costUsdTicks" in us: cost_present += 1
            if us.get("cacheCreationTokens"): cc_nonzero += 1
            mu = us.get("modelUsage") or {}
            nmodels[len(mu)] += 1
            for mk, mv in mu.items(): mu_keys.update(mv.keys())
            s = {x: sum((mv.get(x) or 0) for mv in mu.values()) for x in ("inputTokens","outputTokens","cachedReadTokens","reasoningTokens","costUsdTicks")}
            if mu and any(s[x] != (us.get(x) or 0) for x in s): mu_sum_vs_top += 1
            if us.get("totalTokens") is not None:
                if us.get("totalTokens") == (us.get("inputTokens") or 0) + (us.get("outputTokens") or 0): tot_eq_in_out += 1
                else: tot_ne += 1
            if (us.get("reasoningTokens") or 0) > (us.get("outputTokens") or 0): reasoning_gt_out += 1
            if (us.get("cachedReadTokens") or 0) > (us.get("inputTokens") or 0): cached_gt_in += 1
            for x in ("inputTokens","outputTokens","cachedReadTokens","cacheCreationTokens","reasoningTokens","costUsdTicks"): tc_sum[x] += us.get(x) or 0
            if sample is None: sample = shape(r)
    ev_dupe_in_file += sum(1 for v in evs.values() if v > 1)
    uj = os.path.join(os.path.dirname(f), "usage.json")
    if os.path.exists(uj):
        try:
            d = json.load(open(uj)); s = d.get("session") or {}
            pm = s.get("primaryModelId"); primary_checked += 1
            if pm in (s.get("modelUsage") or {}): primary_in_mu += 1
            usage_json_vs_updates.append(tuple((s.get(x) or 0) - tc_sum[x] for x in ("inputTokens","outputTokens","cachedReadTokens","costUsdTicks")) + (len(d.get("turns") or []),))
        except Exception as ex: pass
print("files", len(files)); print("methods", dict(methods)); print("top keys", dict(top_keys))
print("sessionUpdate kinds", dict(kinds)); print("kinds with update.usage", dict(usage_kinds))
print("params._meta keys", dict(meta_keys)); print("update._meta keys", dict(upd_meta))
print("turn_completed", tc_count, "no eventId", tc_no_event, "agentTimestampMs", agent_ms, "envelope ts types", dict(ts_types), "update.prompt_id", prompt_id_upd)
print("usage keys", dict(usage_keys)); print("modelUsage entry keys", dict(mu_keys)); print("#models per turn", dict(nmodels))
print("costUsdTicks present", cost_present, "cacheCreation nonzero", cc_nonzero, "modelUsage-sum != top", mu_sum_vs_top)
print("total==in+out", tot_eq_in_out, "total!=", tot_ne, "reasoning>output", reasoning_gt_out, "cached>input", cached_gt_in)
print("eventId dup within a file", ev_dupe_in_file, "eventIds seen in >1 row globally", sum(1 for v in ev_global.values() if v > 1))
print("usage.json primary in modelUsage", primary_in_mu, "/", primary_checked)
diffs = collections.Counter("zero" if all(x == 0 for x in t[:4]) else "diff" for t in usage_json_vs_updates)
print("usage.json.session - sum(turn_completed)", dict(diffs))
print("examples of diffs (in,out,cache,ticks,nturns)", [t for t in usage_json_vs_updates if any(t[:4])][:8])
print(json.dumps(sample, indent=1))
