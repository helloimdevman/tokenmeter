# Classifies Grok turn_completed rows that share an eventId (file, prompt_id, usage, time). Numbers only.
# usage: python3 scripts/research/grok_dups.py   (reads ~/.grok/sessions)
import json, glob, os, collections
files = glob.glob(os.path.expanduser("~/.grok/sessions") + "/*/*/updates.jsonl")
rows = collections.defaultdict(list)  # eventId -> list of (fileidx, lineno, prompt_id, usage json, agentTs, method, sessionId)
tc_methods = collections.Counter(); sess_mismatch = 0; dir_vs_sid = collections.Counter()
for fi, f in enumerate(files):
    sdir = os.path.basename(os.path.dirname(f))
    for ln, line in enumerate(open(f, errors="replace")):
        try: r = json.loads(line)
        except Exception: continue
        p = r.get("params") or {}; u = p.get("update") or {}
        if p.get("sessionId") is not None: dir_vs_sid["eq" if p.get("sessionId") == sdir else "ne"] += 1
        if u.get("sessionUpdate") != "turn_completed": continue
        tc_methods[r.get("method")] += 1
        m = p.get("_meta") or {}
        rows[m.get("eventId")].append((fi, ln, u.get("prompt_id"), json.dumps(u.get("usage"), sort_keys=True), m.get("agentTimestampMs"), p.get("sessionId"), u.get("stop_reason")))
print("turn_completed methods", dict(tc_methods)); print("params.sessionId vs session dir name", dict(dir_vs_sid))
cls = collections.Counter()
for e, lst in rows.items():
    if len(lst) < 2: continue
    same_file = len({x[0] for x in lst}) == 1
    same_prompt = len({x[2] for x in lst}) == 1
    same_usage = len({x[3] for x in lst}) == 1
    same_ts = len({x[4] for x in lst}) == 1
    same_sid = len({x[5] for x in lst}) == 1
    no_usage = sum(1 for x in lst if x[3] == "null")
    cls[(len(lst), "samefile" if same_file else "crossfile", "sameprompt" if same_prompt else "diffprompt", "sameusage" if same_usage else "diffusage", "samets" if same_ts else "diffts", "samesid" if same_sid else "diffsid", "nousage=%d" % no_usage)] += 1
for k, v in sorted(cls.items(), key=lambda kv: -kv[1]): print(v, k)
# stop_reason distribution
sr = collections.Counter(x[6] for lst in rows.values() for x in lst); print("stop_reason", dict(sr))
