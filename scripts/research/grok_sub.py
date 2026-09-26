# Shapes of Grok subagent_spawned/subagent_finished updates and whether child ids are session dirs.
# usage: python3 scripts/research/grok_sub.py   (reads ~/.grok/sessions)
import json, glob, os, collections, sys
sys.path.insert(0, os.path.dirname(__file__)); from shape import shape
root = os.path.expanduser("~/.grok/sessions")
files = sorted(glob.glob(root + "/*/*/updates.jsonl"))
sess_ids = {os.path.basename(os.path.dirname(f)) for f in files}
sample_spawn = sample_fin = None; child_ids = set(); fin_usage = 0
for f in files:
    for line in open(f, errors="replace"):
        if "subagent_" not in line: continue
        try: r = json.loads(line)
        except Exception: continue
        u = (r.get("params") or {}).get("update") or {}
        k = u.get("sessionUpdate")
        if k == "subagent_spawned":
            sample_spawn = sample_spawn or shape(u)
            for key in ("sessionId", "childSessionId", "subagentId", "subagent_id", "id"):
                if isinstance(u.get(key), str): child_ids.add(u[key])
        if k == "subagent_finished":
            sample_fin = sample_fin or shape(u)
            if "usage" in json.dumps(u): fin_usage += 1
print("spawn", json.dumps(sample_spawn, indent=1)[:1500]); print("finished", json.dumps(sample_fin, indent=1)[:2500])
print("child ids found", len(child_ids), "that are session dirs", len(child_ids & sess_ids), "finished rows mentioning usage", fin_usage)
