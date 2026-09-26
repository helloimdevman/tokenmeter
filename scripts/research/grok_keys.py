# Simulate dedup strategies over local Grok turn_completed rows (numbers only, no text printed).
# usage: python3 scripts/research/grok_keys.py   (reads ~/.grok/sessions)
import json, glob, os, collections
files = sorted(glob.glob(os.path.expanduser("~/.grok/sessions") + "/*/*/updates.jsonl"))
ev_kinds = collections.defaultdict(set)
tc = []  # (file, eventId, prompt_id, total, input, cached, output)
chunk_ev_first = {}
order = []  # global order of (eventId, kind) as TokenMeter would see file by file
for f in files:
    for line in open(f, errors="replace"):
        try: r = json.loads(line)
        except Exception: continue
        p = r.get("params") or {}; u = p.get("update") or {}; m = p.get("_meta") or {}
        k = u.get("sessionUpdate"); e = m.get("eventId")
        ev_kinds[e].add(k)
        if k in ("turn_completed", "agent_thought_chunk", "agent_message_chunk"):
            order.append((e, k, f, u))
        if k == "turn_completed" and u.get("usage"):
            us = u["usage"]
            tc.append((f, e, u.get("prompt_id"), us.get("totalTokens", 0), us.get("inputTokens", 0), us.get("cachedReadTokens", 0), us.get("outputTokens", 0)))
mixed = sum(1 for e, ks in ev_kinds.items() if "turn_completed" in ks and len(ks) > 1)
print("eventIds used by turn_completed AND another kind:", mixed)
print("turn rows with usage", len(tc), "sum total", sum(t[3] for t in tc))
pids = collections.Counter(t[2] for t in tc); print("distinct prompt_id", len(pids), "prompt_id on >1 usage row", sum(1 for v in pids.values() if v > 1))
# prompt_id duplicates: same file or cross file? same usage?
by_pid = collections.defaultdict(list)
for t in tc: by_pid[t[2]].append(t)
c = collections.Counter()
for pid, lst in by_pid.items():
    if len(lst) > 1: c[("samefile" if len({x[0] for x in lst}) == 1 else "crossfile", "sameusage" if len({x[3:] for x in lst}) == 1 else "diffusage")] += 1
print("prompt_id dup classes", dict(c))
def run(keyf):
    seen = set(); tot = 0; n = 0
    for t in tc:
        k = keyf(t)
        if k is not None and k in seen: continue
        if k is not None: seen.add(k)
        tot += t[3]; n += 1
    return n, tot
print("no dedup", run(lambda t: None))
print("eventId (ccusage eventId|model, 1 model/turn)", run(lambda t: t[1]))
print("prompt_id", run(lambda t: t[2]))
print("eventId|prompt_id", run(lambda t: (t[1], t[2])))
# TokenMeter current: global seen set over turn_completed + chunk rows keyed by eventId
seen = set(); tm_tot = 0; tm_n = 0
for e, k, f, u in order:
    if e in seen: continue
    seen.add(e)
    if k == "turn_completed" and u.get("usage"): tm_tot += u["usage"].get("totalTokens", 0); tm_n += 1
print("TokenMeter current (eventId over chunks+turns, first wins)", (tm_n, tm_tot))
