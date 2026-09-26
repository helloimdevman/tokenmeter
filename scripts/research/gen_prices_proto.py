#!/usr/bin/env python3
"""Prototype: models.dev api.json + LiteLLM -> prices.tsv + routes.tsv (stdlib only).

usage: python3 scripts/research/gen_prices_proto.py MODELS_DEV_API_JSON LITELLM_JSON OUT_DIR
  (https://models.dev/api.json, LiteLLM model_prices_and_context_window.json)
"""
import json, re, sys
from urllib.parse import urlsplit

md = json.load(open(sys.argv[1]))
ll = json.load(open(sys.argv[2]))
out = sys.argv[3]

# vendor family -> (models.dev owner catalogs, LiteLLM owner namespaces). Mirrors pricing::vendor_of.
FAMILY = [
    (r"^claude-", "anthropic", ["anthropic"], ["anthropic"]),
    (r"^(gpt-|o1|o3|o4|codex-|chatgpt-)", "openai", ["openai"], ["openai"]),
    (r"^(gemini-|gemma-)", "google", ["google"], ["gemini"]),
    (r"^grok-", "xai", ["xai"], ["xai"]),
    (r"^deepseek-", "deepseek", ["deepseek"], ["deepseek"]),
    (r"^kimi-", "moonshotai", ["moonshotai"], ["moonshot"]),
    (r"^glm-", "zai", ["zai", "zhipuai"], ["zai"]),
    (r"^minimax-", "minimax", ["minimax"], ["minimax"]),
    (r"^(qwen|qwq|qvq)", "alibaba", ["alibaba"], ["dashscope"]),
    (r"^(mistral-|devstral-|codestral-|magistral-|ministral-|pixtral-|voxtral-|open-mistral|open-mixtral)", "mistral", ["mistral"], ["mistral"]),
    (r"^command-", "cohere", ["cohere"], ["cohere", "cohere_chat"]),
    (r"^sonar", "perplexity", ["perplexity"], ["perplexity"]),
    (r"^mimo-", "xiaomi", ["xiaomi"], ["xiaomi_mimo"]),
    (r"^step-", "stepfun", ["stepfun-ai"], []),
]

def family(mid):
    for pat, vendor, owners, llns in FAMILY:
        if re.match(pat, mid):
            return vendor, owners, llns
    return None, [], []

def usable(c):
    return c and isinstance(c.get("input"), (int, float)) and isinstance(c.get("output"), (int, float)) \
        and (c["input"] > 0 or c["output"] > 0)

def ll_row(e):
    i, o = e.get("input_cost_per_token"), e.get("output_cost_per_token")
    if not isinstance(i, (int, float)) or not isinstance(o, (int, float)) or (i <= 0 and o <= 0):
        return None
    if e.get("mode") not in ("chat", "responses", "completion", None):
        return None
    m = lambda k: (e.get(k) * 1e6) if isinstance(e.get(k), (int, float)) else None
    row = {"input": i * 1e6, "output": o * 1e6, "cache_read": m("cache_read_input_token_cost"),
           "cache_write": m("cache_creation_input_token_cost"), "context": e.get("max_input_tokens")}
    for t in ("200k", "272k", "256k", "128k"):
        ai = m(f"input_cost_per_token_above_{t}_tokens")
        if ai:
            row["tier"] = (int(t[:-1]) * 1000, ai, m(f"output_cost_per_token_above_{t}_tokens"),
                           m(f"cache_read_input_token_cost_above_{t}_tokens"),
                           m(f"cache_creation_input_token_cost_above_{t}_tokens"))
            break
    return row

def md_row(m):
    c = m["cost"]
    row = {k: c.get(k) for k in ("input", "output", "cache_read", "cache_write")}
    row["context"] = (m.get("limit") or {}).get("context")
    o = c.get("context_over_200k")
    if o:
        row["tier"] = (200000, o.get("input"), o.get("output"), o.get("cache_read"), o.get("cache_write"))
    elif c.get("tiers"):
        t = c["tiers"][0]
        row["tier"] = ((t.get("tier") or {}).get("size"), t.get("input"), t.get("output"), t.get("cache_read"), t.get("cache_write"))
    return row

def norm(s):
    return re.sub(r"[._@: ]", "-", s.lower())

rows = {}  # canonical lowercase id -> (label, row, src)

# 1. models.dev owner catalogs
for pid, p in md.items():
    for key, m in p["models"].items():
        mid = m.get("id") or key
        vendor, owners, _ = family(mid.lower())
        if pid in owners and usable(m.get("cost")):
            rows.setdefault(mid.lower(), (mid, md_row(m), "m"))

# 2. LiteLLM owner namespaces (bare keys whose litellm_provider is the owner, or "<owner-ns>/<id>")
for key, e in ll.items():
    if not isinstance(e, dict):
        continue
    parts = key.split("/")
    mid = parts[-1]
    vendor, _, llns = family(mid.lower())
    if not vendor:
        continue
    ns = parts[0] if len(parts) == 2 else (e.get("litellm_provider") if len(parts) == 1 else None)
    if ns in llns and mid.lower() not in rows:
        r = ll_row(e)
        if r:
            rows[mid.lower()] = (mid, r, "l")

# 2b. LiteLLM platform copies (Bedrock / Vertex / Azure) at list price: the only place retired ids survive
PLAT = re.compile(r"^(?:(?:bedrock|bedrock_converse|vertex_ai|azure)/)?(?:[a-z]{2}-[a-z]+-\d/)?(?:(?:us|eu|apac|global|jp|au)\.)?(?:anthropic|moonshot|moonshotai|deepseek|qwen|openai|minimax|zai|mistral|meta|cohere|xai|google)\.")
for key, e in ll.items():
    if not isinstance(e, dict) or e.get("litellm_provider") not in ("bedrock", "bedrock_converse", "vertex_ai-anthropic_models", "vertex_ai-language-models", "azure"):
        continue
    mid = PLAT.sub("", key)
    mid = mid.split("/")[-1]
    mid = re.sub(r"-v\d+(:\d+)?$", "", mid)
    mid = mid.split("@")[0]
    vendor, _, _ = family(mid.lower())
    if vendor and mid.lower() not in rows:
        r = ll_row(e)
        if r:
            rows[mid.lower()] = (mid, r, "p")

# 3. OpenRouter pass-through for public ids nobody above priced (e.g. retired or open-weight models)
for key, m in md.get("openrouter", {}).get("models", {}).items():
    mid = (m.get("id") or key).split("/")[-1]
    mid = mid.split(":")[0]
    if norm(mid) not in {norm(k) for k in rows} and usable(m.get("cost")) and re.search(r"\d", mid):
        rows[mid.lower()] = (mid, md_row(m), "o")

def fmt(x):
    if x is None:
        return ""
    s = f"{x:.6f}".rstrip("0").rstrip(".")
    return s or "0"

with open(f"{out}/prices.tsv", "w") as f:
    f.write("# id\tinput\toutput\tcache_read\tcache_write\tcontext\tsrc\ttier_at\ttin\ttout\ttcr\ttcw  (USD per 1M tokens)\n")
    for k in sorted(rows):
        label, r, src = rows[k]
        cols = [label, fmt(r["input"]), fmt(r["output"]), fmt(r["cache_read"]), fmt(r["cache_write"]),
                str(int(r["context"])) if r.get("context") else "", src]
        if r.get("tier") and r["tier"][0]:
            t = r["tier"]
            cols += [str(int(t[0])), fmt(t[1]), fmt(t[2]), fmt(t[3]), fmt(t[4])]
        f.write("\t".join(cols).rstrip("\t") + "\n")

# routes: models.dev api URLs (provider + model-level overrides) -> "host/path-prefix  provider  plan-hint"
PLAN = re.compile(r"(coding-plan|token-plan|step-plan|code-plan|-pass$|^github-copilot$)")
def route_rows():
    for pid, p in md.items():
        apis = [p.get("api")] + [(m.get("provider") or {}).get("api") for m in p["models"].values()]
        for api in filter(None, apis):
            yield pid, api
routes = {}
for pid, api in sorted(route_rows(), key=lambda r: (len(r[0]), r[0])):   # shortest id wins a shared URL
    host_t, _, path = api.split("://", 1)[-1].partition("/")
    if host_t.startswith("${") and "}." in host_t:
        host = "*." + host_t.split("}.", 1)[1]              # ${AZURE_RESOURCE_NAME}.services.ai.azure.com -> *.services.ai.azure.com
    elif "${" in host_t:
        continue                                             # whole-host template: nothing public to match
    else:
        host = host_t.lower().split(":")[0]
    if host in ("127.0.0.1", "localhost") or "${" in path:
        continue
    path = re.sub(r"(/chat/completions)?/?$", "", "/" + path)
    path = re.sub(r"/v\d+[a-z0-9]*$", "", path)             # version segment: users often omit it
    hint = "sub" if PLAN.search(pid) else "-"
    routes.setdefault(host + path, (pid, hint))
    routes.setdefault(host, (pid, hint if host + path == host else "-"))
with open(f"{out}/routes-modelsdev.tsv", "w") as f:
    for k in sorted(routes):
        f.write(f"{k}\t{routes[k][0]}\t{routes[k][1]}\n")

src = {}
for _, _, s in rows.values():
    src[s] = src.get(s, 0) + 1
print("rows", len(rows), "by source", src, "tiers", sum(1 for _, r, _ in rows.values() if r.get("tier")))
print("routes", len(routes), "hosts", len({k.split('/')[0] for k in routes}))
