# Explains sessions where Grok usage.json totals differ from the sum of updates.jsonl turns. Numbers only.
# usage: python3 scripts/research/grok_usagejson.py   (reads ~/.grok/sessions)
import json, glob, os, collections
files = sorted(glob.glob(os.path.expanduser("~/.grok/sessions") + "/*/*/updates.jsonl"))
pid_files = collections.defaultdict(set)
per = {}
for f in files:
    d = os.path.dirname(f); kinds = collections.Counter(); turns = []
    for line in open(f, errors="replace"):
        try: r = json.loads(line)
        except Exception: continue
        u = (r.get("params") or {}).get("update") or {}; k = u.get("sessionUpdate"); kinds[k] += 1
        if k == "turn_completed" and u.get("usage"):
            turns.append(u["usage"].get("totalTokens", 0)); pid_files[u.get("prompt_id")].add(f)
    per[f] = (kinds, turns)
for f, (kinds, turns) in per.items():
    uj = os.path.join(os.path.dirname(f), "usage.json")
    if not os.path.exists(uj): continue
    d = json.load(open(uj)); s = d.get("session") or {}; jt = d.get("turns") or []
    diff = (s.get("totalTokens") or 0) - sum(turns)
    if diff == 0: continue
    copied = sum(1 for p, fs in pid_files.items() if f in fs and len(fs) > 1)
    turn_totals_json = [t.get("totalTokens") for t in jt]
    print("diff", diff, "| updates turns", len(turns), "usage.json turns", len(jt), "turnCount", s.get("turnCount"),
          "| subagent_spawned", kinds.get("subagent_spawned", 0), "compaction", kinds.get("auto_compact_completed", 0),
          "user_msgs", kinds.get("user_message_chunk", 0), "tc rows", kinds.get("turn_completed", 0),
          "| copied-prompt turns", copied,
          "| json turn totals ⊆ updates turn totals:", sum(1 for t in turn_totals_json if t in turns), "/", len(turn_totals_json))
# usage.json turnNumber semantics & endedAt type
d = json.load(open(glob.glob(os.path.expanduser("~/.grok/sessions") + "/*/*/usage.json")[0]))
print("turn keys", sorted((d.get("turns") or [{}])[0].keys()))
# sessions with usage.json but also a subagent: does usage.json include subagent usage?
n_sub = 0; n_sub_eq = 0
for f, (kinds, turns) in per.items():
    uj = os.path.join(os.path.dirname(f), "usage.json")
    if kinds.get("subagent_spawned") and os.path.exists(uj):
        n_sub += 1; s = json.load(open(uj)).get("session") or {}
        if (s.get("totalTokens") or 0) == sum(turns): n_sub_eq += 1
print("sessions with subagents & usage.json", n_sub, "where usage.json == updates sum", n_sub_eq)
print("sessions without usage.json", sum(1 for f in per if not os.path.exists(os.path.join(os.path.dirname(f), "usage.json"))), "of which have usage turns", sum(1 for f,(k,t) in per.items() if t and not os.path.exists(os.path.join(os.path.dirname(f), "usage.json"))))
