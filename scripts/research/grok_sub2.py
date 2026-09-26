# Whether Grok child sessions are persisted and whether the parent's usage.json includes them. Numbers only.
# usage: python3 scripts/research/grok_sub2.py   (reads ~/.grok/sessions)
import json, glob, os, collections
root = os.path.expanduser("~/.grok/sessions")
files = sorted(glob.glob(root + "/*/*/updates.jsonl"))
by_sid = {}; children = collections.defaultdict(list); fin_tokens = collections.defaultdict(int)
child_types = collections.Counter(); child_exists_by_type = collections.Counter()
for f in files:
    sid = os.path.basename(os.path.dirname(f)); tot = 0
    for line in open(f, errors="replace"):
        try: r = json.loads(line)
        except Exception: continue
        u = (r.get("params") or {}).get("update") or {}; k = u.get("sessionUpdate")
        if k == "turn_completed" and u.get("usage"): tot += u["usage"].get("totalTokens", 0)
        if k == "subagent_spawned": children[sid].append((u.get("child_session_id"), u.get("subagent_type")))
        if k == "subagent_finished": fin_tokens[sid] += u.get("tokens_used") or 0
    by_sid[sid] = (f, tot)
for sid, ch in children.items():
    for c, t in ch:
        child_types[t] += 1
        if c in by_sid: child_exists_by_type[t] += 1
print("child sessions persisted: by type (exists/total)", {t: (child_exists_by_type[t], n) for t, n in child_types.items()} if False else [(child_exists_by_type[t], n) for t, n in child_types.items()])
for sid, ch in children.items():
    f, tot = by_sid[sid]; uj = os.path.join(os.path.dirname(f), "usage.json")
    if not os.path.exists(uj): continue
    s = json.load(open(uj)).get("session") or {}
    ch_sum = sum(by_sid[c][1] for c, _ in ch if c in by_sid)
    missing = [c for c, _ in ch if c not in by_sid]
    print("parent: usage.json - updates =", (s.get("totalTokens") or 0) - tot, "| existing children updates sum", ch_sum, "| children missing", len(missing), "of", len(ch), "| subagent_finished.tokens_used sum", fin_tokens[sid])
# where child session dirs live relative to parent (same cwd dir?)
same = diff = 0
for sid, ch in children.items():
    pd = os.path.dirname(os.path.dirname(by_sid[sid][0]))
    for c, _ in ch:
        if c in by_sid:
            if os.path.dirname(os.path.dirname(by_sid[c][0])) == pd: same += 1
            else: diff += 1
print("child dir under same cwd dir as parent", same, "different", diff)
# do child updates carry a marker of being a subagent?
