#!/usr/bin/env python3
"""내장 가격·경로 표(native/meter/data/{prices,routes}.tsv)를 만든다. 표준 라이브러리만. 스펙 7.1-7.2.

python3 scripts/gen-prices.py [--models-dev PATH|URL] [--litellm PATH|URL] [--curated-only] [--check] [--self-test]

  --curated-only  네트워크 없이 지금 생성 파일에 scripts/prices/의 손본 행만 다시 입힌다.
  --check         결과가 디스크와 다르면 종료 코드 1(쓰지 않는다).
  --self-test     스크립트 안의 작은 입력으로 1-7단계와 지키는 조건을 단언한다.

지키는 조건을 어기면 종료 코드 1. 가격이 5배 넘게 움직인 행은 표준 출력의 `## Large moves` 절에 적는다.
"""
import argparse
import json
import math
import re
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "native/meter/data"
CURATED = ROOT / "scripts/prices"
MODELS_DEV = "https://models.dev/api.json"
LITELLM = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"

MIN_ROWS = 400
BUDGET = 64 * 1024
MODEL_ID = re.compile(r"[A-Za-z0-9._:@+-]{1,96}")
ROUTE_ID = re.compile(r"[a-z0-9.-]{1,64}")
ROUTE_KEY = re.compile(r"(\*[.-])?[a-z0-9._~/-]{1,200}")
PRICES_HEAD = "# id\tvendor\tinput\toutput\tcache_read\tcache_write\tcontext\tsrc\t[tier_at\tt_in\tt_out\tt_cr\tt_cw]  USD/1M tokens, scripts/gen-prices.py\n"
ROUTES_HEAD = "# key\tprovider\tplan hint(sub|local|-)  scripts/gen-prices.py\n"
FIELDS = ("input", "output", "cache_read", "cache_write")

# 계열 → (벤더 = models.dev 소유자 목록 첫째, 소유자 목록들, LiteLLM 소유자 이름공간들). 스펙 7.2의 1단계.
FAMILY = [
    (r"^claude-", ["anthropic"], ["anthropic"]),
    (r"^(gpt-|o1|o3|o4|codex-|chatgpt-)", ["openai"], ["openai"]),
    (r"^(gemini-|gemma-)", ["google"], ["gemini"]),
    (r"^grok-", ["xai"], ["xai"]),
    (r"^deepseek-", ["deepseek"], ["deepseek"]),
    (r"^kimi-", ["moonshotai"], ["moonshot"]),
    (r"^glm-", ["zai", "zhipuai"], ["zai"]),
    (r"^minimax-", ["minimax"], ["minimax"]),
    (r"^(qwen|qwq|qvq)", ["alibaba"], ["dashscope"]),
    (r"^(mistral-|devstral-|codestral-|magistral-|ministral-|pixtral-|voxtral-|open-mistral|open-mixtral)", ["mistral"], ["mistral"]),
    (r"^command-", ["cohere"], ["cohere", "cohere_chat"]),
    (r"^sonar", ["perplexity"], ["perplexity"]),
    (r"^mimo-", ["xiaomi"], ["xiaomi_mimo"]),
    (r"^step-", ["stepfun-ai"], []),
]
# OpenRouter 조직 이름 → models.dev 프로바이더 id(다를 때만). 계열 밖 행의 vendor 칸.
OPENROUTER_ORG = {"x-ai": "xai", "z-ai": "zai", "mistralai": "mistral", "meta-llama": "meta", "qwen": "alibaba"}
PLATFORM_PROVIDERS = ("bedrock", "bedrock_converse", "vertex_ai-anthropic_models", "vertex_ai-language-models", "azure")
PLATFORM = re.compile(r"^(?:(?:bedrock|bedrock_converse|vertex_ai|azure)/)?(?:[a-z]{2}-[a-z]+-\d/)?(?:(?:us|eu|apac|global|jp|au)\.)?"
                      r"(?:anthropic|moonshot|moonshotai|deepseek|qwen|openai|minimax|zai|mistral|meta|cohere|xai|google)\.")
REGIONAL = re.compile(r"/[a-z]{2}-[a-z]+-\d/|(?:^|/)(?:us|eu|apac|global|jp|au)[./]")
PLAN_ID = re.compile(r"(coding-plan|token-plan|step-plan|code-plan|-pass$|^github-copilot$)")


def family(mid):
    for pat, owners, llns in FAMILY:
        if re.match(pat, mid.lower()):
            return owners[0], owners, llns
    return None, [], []


def norm(s):
    return re.sub(r"[._@: ]", "-", s.strip().lower())


def num(x):
    return x if isinstance(x, (int, float)) and not isinstance(x, bool) else None


def row(mid, vendor, src, p):
    return {"id": mid, "vendor": vendor, "src": src, **p}


def md_price(m):
    c = m["cost"]
    p = {k: num(c.get(k)) for k in FIELDS}
    p["context"] = num((m.get("limit") or {}).get("context"))
    p["tier"] = None
    # tiers[0]이 구간 크기를 가진다. context_over_200k는 크기와 상관없이 같은 값을 되풀이하는 옛 칸이다.
    t = next((t for t in c.get("tiers") or [] if (t.get("tier") or {}).get("type") == "context"), None)
    if t and num(t["tier"].get("size")) and num(t.get("input")) is not None:
        p["tier"] = (t["tier"]["size"], *(num(t.get(k)) for k in FIELDS))
    elif isinstance(c.get("context_over_200k"), dict) and num(c["context_over_200k"].get("input")) is not None:
        o = c["context_over_200k"]
        p["tier"] = (200000, *(num(o.get(k)) for k in FIELDS))
    return p


def md_usable(m):
    c = m.get("cost") or {}
    i, o = num(c.get("input")), num(c.get("output"))
    return i is not None and o is not None and (i > 0 or o > 0)


def ll_price(e):
    """LiteLLM 항목 → 1M 토큰당 가격. chat·responses·completion이 아니거나 값이 없으면 None."""
    if e.get("mode") not in ("chat", "responses", "completion"):
        return None
    m = lambda k: num(e.get(k)) * 1e6 if num(e.get(k)) is not None else None
    i, o = m("input_cost_per_token"), m("output_cost_per_token")
    if i is None or o is None or (i <= 0 and o <= 0):
        return None
    p = {"input": i, "output": o, "cache_read": m("cache_read_input_token_cost"),
         "cache_write": m("cache_creation_input_token_cost"), "context": num(e.get("max_input_tokens")), "tier": None}
    for t in ("200k", "272k", "256k", "128k"):
        if m(f"input_cost_per_token_above_{t}_tokens") is not None:
            p["tier"] = (int(t[:-1]) * 1000, m(f"input_cost_per_token_above_{t}_tokens"), m(f"output_cost_per_token_above_{t}_tokens"),
                         m(f"cache_read_input_token_cost_above_{t}_tokens"), m(f"cache_creation_input_token_cost_above_{t}_tokens"))
            break
    return p


def generate(md, ll):
    """스펙 7.2의 1-4단계와 6단계(정규화 키로 중복 제거, 먼저 온 출처가 이긴다). 정규화 키 → 행."""
    rows = {}

    def put(mid, vendor, src, p):
        if MODEL_ID.fullmatch(mid):  # 라벨이 될 수 없는 id는 싣지 않는다
            rows.setdefault(norm(mid), row(mid, vendor, src, p))

    # LiteLLM 소유자 이름공간: 소유자가 litellm_provider인 맨 키, 또는 `<소유자>/<id>`
    owned = {}
    for key, e in ll.items():
        if not isinstance(e, dict):
            continue
        parts = key.split("/")
        mid = parts[-1]
        vendor, _, llns = family(mid)
        ns = parts[0] if len(parts) == 2 else (e.get("litellm_provider") if len(parts) == 1 else None)
        if vendor and ns in llns and norm(mid) not in owned:
            p = ll_price(e)
            if p:
                owned[norm(mid)] = (mid, vendor, p)

    # 1. models.dev 소유자 목록. 구간이 없으면 LiteLLM 소유자 행의 구간으로 채운다.
    for pid, prov in md.items():
        for key, m in (prov.get("models") or {}).items():
            mid = m.get("id") or key
            vendor, owners, _ = family(mid)
            if pid in owners and md_usable(m):
                p = md_price(m)
                if not p["tier"] and norm(mid) in owned:
                    p["tier"] = owned[norm(mid)][2]["tier"]
                put(mid, vendor, "m", p)

    # 2. LiteLLM 소유자 이름공간
    for mid, vendor, p in owned.values():
        put(mid, vendor, "l", p)

    # 3. 플랫폼(Bedrock·Vertex·Azure) 목록가. 지역 없는 키를 먼저 본다(지역 키는 웃돈이 붙는다).
    plat = [k for k, e in ll.items() if isinstance(e, dict) and e.get("litellm_provider") in PLATFORM_PROVIDERS]
    for key in sorted(plat, key=lambda k: (bool(REGIONAL.search(k)), k)):
        mid = PLATFORM.sub("", key).split("/")[-1]
        mid = re.sub(r"-v\d+(:\d+)?$", "", mid).split("@")[0]
        vendor, _, _ = family(mid)
        p = ll_price(ll[key])
        if vendor and p:
            put(mid, vendor, "p", p)

    # 4. OpenRouter 통과가: 위에서 못 정한 공개 id 중 숫자가 든 것
    for key, m in ((md.get("openrouter") or {}).get("models") or {}).items():
        full = m.get("id") or key
        mid = full.split("/")[-1].split(":")[0]
        if re.search(r"\d", mid) and md_usable(m):
            org = full.split("/")[0].lstrip("~") if "/" in full else ""
            put(mid, family(mid)[0] or OPENROUTER_ORG.get(org, org) or "unknown", "o", md_price(m))
    return rows


def parse_prices(text, curated=False):
    """TSV → (행 목록, 별칭 목록). 손본 파일에는 src 칸이 없고 `id<TAB>=대상`으로 별칭을 쓴다."""
    rows, aliases = [], []
    for n, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        cols = line.split("\t")
        if curated and len(cols) == 2 and cols[1].startswith("="):
            aliases.append((cols[0], cols[1][1:]))
            continue
        if curated:
            cols = cols[:7] + [""] * (7 - len(cols)) + ["c"] + cols[7:]
        cols += [""] * (13 - len(cols))
        try:
            f = [float(x) if x else None for x in cols[2:6] + cols[9:13]]
            p = {"input": f[0], "output": f[1], "cache_read": f[2], "cache_write": f[3],
                 "context": int(cols[6]) if cols[6] else None,
                 "tier": (int(cols[8]), *f[4:]) if cols[8] else None}
        except ValueError as err:
            sys.exit(f"{'curated' if curated else 'prices'} line {n}: {err}")
        rows.append(row(cols[0], cols[1], cols[7], p))
    return rows, aliases


def merge(base, curated_text, old):
    """5. 손본 행과 별칭이 이긴다. 7. 예전 행은 지우지 않는다(사라진 행은 src=k)."""
    rows = dict(base)
    plain, aliases = parse_prices(curated_text, curated=True)
    for r in plain:
        rows[norm(r["id"])] = r
    for mid, target in aliases:
        t = rows.get(norm(target))
        if not t:
            sys.exit(f"curated alias {mid}: no row {target}")
        rows[norm(mid)] = {**t, "id": mid, "src": "c"}
    for r in old:
        rows.setdefault(norm(r["id"]), {**r, "src": "k"})
    return rows


def gen_routes(md):
    """models.dev 프로바이더 `api`와 모델별 `provider.api`에서 경로 행을, 프로바이더 id마다 맨 id 행을 만든다."""
    seen = {}  # 키 → {id: 요금제 행인가}. 한 번이라도 아니면 아니다.

    def add(key, pid, plan):
        ids = seen.setdefault(key, {})
        ids[pid] = ids.get(pid, True) and plan

    for pid, prov in md.items():
        apis = [prov.get("api")] + [(m.get("provider") or {}).get("api") for m in (prov.get("models") or {}).values()]
        for api in filter(None, apis):
            host, _, path = api.split("://", 1)[-1].partition("/")
            if host.startswith("${") and "}." in host:
                host, path = "*." + host.split("}.", 1)[1].lower(), ""  # ${AZURE_RESOURCE_NAME}.services.ai.azure.com → *.services.ai.azure.com
            elif "${" in host:
                continue  # 통째로 템플릿인 URL
            else:
                host = host.lower().split(":")[0]
            if host in ("localhost", "0.0.0.0") or host.startswith(("127.", "[")):
                continue  # 루프백과 IP 리터럴
            path = "" if "${" in path else re.sub(r"(/chat/completions)?/?$", "", "/" + path)
            path = re.sub(r"/v\d+[a-z0-9]*$", "", path)  # 버전 조각은 사용자가 흔히 뺀다
            plan = bool(PLAN_ID.search(pid))
            add(host + path, pid, plan)
            if path:
                add(host, pid, False)  # 호스트만 있는 행
        add(pid, pid, bool(PLAN_ID.search(pid)))  # 맨 id 행
    out = {}
    for key, ids in seen.items():
        pid = min(ids, key=lambda i: (len(i), i))  # URL 하나를 여러 id가 쓰면 짧은 id가 이기고 요금제는 모른다
        out[key] = (pid, "sub" if len(ids) == 1 and ids[pid] else "-")
    return out


def parse_routes(text):
    out = {}
    for line in text.splitlines():
        if line.strip() and not line.startswith("#"):
            cols = line.split("\t") + ["", ""]
            out[cols[0]] = (cols[1], cols[2] or "-")
    return out


def fmt(x):
    if x is None:
        return ""
    return f"{x:.6f}".rstrip("0").rstrip(".") or "0"


def render_prices(rows):
    lines = []
    for r in sorted(rows.values(), key=lambda r: (r["id"].lower(), r["id"])):
        cols = [r["id"], r["vendor"], *(fmt(r[k]) for k in FIELDS), str(int(r["context"])) if r["context"] else "", r["src"]]
        if r["tier"]:
            cols += [str(int(r["tier"][0])), *(fmt(x) for x in r["tier"][1:])]
        lines.append("\t".join(cols).rstrip("\t") + "\n")
    return PRICES_HEAD + "".join(lines)


def render_routes(routes):
    return ROUTES_HEAD + "".join(f"{k}\t{pid}\t{hint}\n" for k, (pid, hint) in sorted(routes.items()))


def problems(rows, routes, prices_text, routes_text, min_rows=MIN_ROWS):
    errs = []
    if len(rows) < min_rows:
        errs.append(f"only {len(rows)} price rows (< {min_rows})")
    vendors = {r["vendor"] for r in rows.values()}
    ids = {pid for pid, _ in routes.values()}
    for need in ("anthropic", "openai", "google"):
        if need not in vendors or need not in ids:
            errs.append(f"{need} missing")
    for r in rows.values():
        if not MODEL_ID.fullmatch(r["id"]):
            errs.append(f"bad model id {r['id']!r}")
        if not ROUTE_ID.fullmatch(r["vendor"]):
            errs.append(f"{r['id']}: bad vendor {r['vendor']!r}")
        if r["input"] is None or r["output"] is None:
            errs.append(f"{r['id']}: input and output are required")
        tier = list(r["tier"] or [])
        for x in [r[k] for k in FIELDS] + [r["context"]] + tier:
            if x is not None and not (math.isfinite(x) and x >= 0):
                errs.append(f"{r['id']}: bad number {x}")
    for key, (pid, hint) in routes.items():
        if not ROUTE_KEY.fullmatch(key):
            errs.append(f"bad route key {key!r}")
        if not ROUTE_ID.fullmatch(pid):
            errs.append(f"{key}: bad provider id {pid!r}")
        if hint not in ("sub", "local", "-"):
            errs.append(f"{key}: bad plan hint {hint!r}")
    size = len(prices_text.encode()) + len(routes_text.encode())
    if size > BUDGET:
        errs.append(f"embedded tables are {size} bytes (> {BUDGET})")
    return errs


def large_moves(old, rows):
    """같은 행의 가격이 5배 넘게 움직였으면 한 줄씩."""
    out = []
    for r in old:
        new = rows.get(norm(r["id"]))
        for k in FIELDS:
            a, b = r[k], new and new[k]
            if a and b and max(a, b) / min(a, b) > 5:
                out.append(f"- {r['id']} {k}: {fmt(a)} -> {fmt(b)}")
    return out


def load(src):
    if re.match(r"https?://", src):
        with urllib.request.urlopen(src, timeout=120) as resp:
            return json.load(resp)
    with open(src) as f:
        return json.load(f)


def build(md, ll, curated_text, curated_routes_text, old_prices_text, old_routes_text):
    """md가 None이면 --curated-only: 지금 생성 파일이 바탕이다."""
    old, _ = parse_prices(old_prices_text)
    if md is None:
        base = {norm(r["id"]): r for r in old if r["src"] != "c"}  # 손본 행에서 빠진 c 행은 7단계가 k로 남긴다
        # ponytail: 손본 경로 행을 지워도 --curated-only는 디스크 행을 남긴다. 봇의 전체 실행이 지운다.
        routes = parse_routes(old_routes_text)
    else:
        base = generate(md, ll)
        routes = gen_routes(md)
    rows = merge(base, curated_text, old)
    routes.update(parse_routes(curated_routes_text))
    return old, rows, routes


def self_test():
    md = {
        "anthropic": {"models": {
            "claude-x-1-5": {"cost": {"input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75}, "limit": {"context": 200000}},
            "claude-x-2": {"cost": {"input": 2, "output": 10, "tiers": [{"input": 4, "output": 20, "tier": {"type": "context", "size": 272000}}],
                                    "context_over_200k": {"input": 4, "output": 20}}},
            "gpt-5": {"cost": {"input": 9, "output": 9}},  # 소유자 목록이 아니다
        }},
        "openai": {"api": "https://api.openai.com/v1", "models": {
            "gpt-5": {"cost": {"input": 1.25, "output": 10}},
            "gpt-free": {"cost": {"input": 0, "output": 0}},
        }},
        "google": {"models": {"gemini-3-pro": {"cost": {"input": 2, "output": 12}}}},
        "openrouter": {"api": "https://openrouter.ai/api/v1/chat/completions", "models": {
            "anthropic/claude-x-1.5": {"id": "anthropic/claude-x-1.5", "cost": {"input": 99, "output": 99}},
            "moonshotai/kimi-z2:free": {"id": "moonshotai/kimi-z2:free", "cost": {"input": 0.5, "output": 2}},
            "meta-llama/llama-9-8b": {"id": "meta-llama/llama-9-8b", "cost": {"input": 0.1, "output": 0.1}},
            "sakana/fugu": {"id": "sakana/fugu", "cost": {"input": 1, "output": 1}},  # 숫자 없음
        }},
        "minimax": {"api": "https://api.minimax.io/anthropic/v1", "models": {}},
        "minimax-coding-plan": {"api": "https://api.minimax.io/anthropic/v1", "models": {}},
        "zai-coding-plan": {"api": "https://api.z.ai/api/coding/paas/v4", "models": {}},
        "azure": {"models": {"m": {"provider": {"api": "https://${AZURE_RESOURCE_NAME}.services.ai.azure.com/models"}}}},
        "vertex": {"api": "https://${GOOGLE_VERTEX_ENDPOINT}/v1", "models": {}},
        "lab": {"api": "http://127.0.0.1:1234/v1", "models": {}},
        "cf": {"api": "https://api.cloudflare.com/client/v4/accounts/${ID}/ai/v1", "models": {}},
    }
    ll = {
        "claude-x-1-5": {"litellm_provider": "anthropic", "mode": "chat", "input_cost_per_token": 5e-6, "output_cost_per_token": 5e-5,
                         "input_cost_per_token_above_200k_tokens": 6e-6, "output_cost_per_token_above_200k_tokens": 2.25e-5},
        "xai/grok-9": {"litellm_provider": "xai", "mode": "chat", "input_cost_per_token": 2e-6, "output_cost_per_token": 6e-6,
                       "cache_read_input_token_cost": 5e-7, "max_input_tokens": 256000},
        "xai/grok-9-embed": {"litellm_provider": "xai", "mode": "embedding", "input_cost_per_token": 1e-6, "output_cost_per_token": 1e-6},
        "au.anthropic.claude-old-1-20240101-v1:0": {"litellm_provider": "bedrock_converse", "mode": "chat",
                                                    "input_cost_per_token": 1.1e-5, "output_cost_per_token": 1e-4},
        "bedrock/anthropic.claude-old-1-20240101-v1:0": {"litellm_provider": "bedrock", "mode": "chat",
                                                         "input_cost_per_token": 1e-5, "output_cost_per_token": 1e-4},
        "vertex_ai/claude-old-2@20240202": {"litellm_provider": "vertex_ai-anthropic_models", "mode": "chat",
                                            "input_cost_per_token": 3e-6, "output_cost_per_token": 1.5e-5},
    }
    curated = "# c\nclaude-x-2\tanthropic\t1\t5\t\t\t200000\nx-alias\t=gpt-5\n"
    curated_routes = "api.openai.com\topenai\t-\nollama\tlocal\tlocal\n"
    old = PRICES_HEAD + "gpt-5\topenai\t0.2\t10\t\t\t\tm\ngone-model-1\tacme\t1\t2\t\t\t\tl\n"

    _, rows, routes = build(md, ll, curated, curated_routes, old, "")
    by = {r["id"]: r for r in rows.values()}
    # 1. 소유자 목록만, 공짜 행 없음. 구간은 tiers[0]의 크기로, 없으면 LiteLLM에서 채운다.
    assert by["gpt-5"]["src"] == "m" and by["gpt-5"]["input"] == 1.25, by["gpt-5"]
    assert "gpt-free" not in by
    assert by["claude-x-1-5"]["src"] == "m" and by["claude-x-1-5"]["input"] == 3
    assert by["claude-x-1-5"]["tier"][:3] == (200000, 6, 22.5), by["claude-x-1-5"]["tier"]
    # 2. LiteLLM 소유자 이름공간, chat만
    assert by["grok-9"]["src"] == "l" and by["grok-9"]["vendor"] == "xai" and by["grok-9"]["cache_read"] == 0.5
    assert by["grok-9"]["context"] == 256000 and "grok-9-embed" not in by
    # 3. 플랫폼 목록가: 지역·anthropic.·-v1:0·@날짜를 떼고, 지역 없는 키가 이긴다
    assert by["claude-old-1-20240101"]["src"] == "p" and by["claude-old-1-20240101"]["input"] == 10
    assert by["claude-old-2"]["src"] == "p"
    # 4. OpenRouter: 숫자가 든 id만, vendor는 계열이나 조직
    assert by["kimi-z2"]["src"] == "o" and by["kimi-z2"]["vendor"] == "moonshotai"
    assert by["llama-9-8b"]["vendor"] == "meta" and "fugu" not in by
    # 5. 손본 행과 별칭이 이긴다
    assert by["claude-x-2"]["src"] == "c" and by["claude-x-2"]["input"] == 1 and by["claude-x-2"]["tier"] is None
    assert by["x-alias"]["src"] == "c" and by["x-alias"]["input"] == 1.25 and by["x-alias"]["vendor"] == "openai"
    # 6. 정규화 키로 한 행(claude-x-1.5는 먼저 온 anthropic 행에 합쳐진다)
    assert "claude-x-1.5" not in by and len({norm(i) for i in by}) == len(by)
    # 7. 사라진 행은 k로 남고, 5배 넘는 움직임은 적힌다
    assert by["gone-model-1"]["src"] == "k"
    assert large_moves(parse_prices(old)[0], rows) == ["- gpt-5 input: 0.2 -> 1.25"]

    # 경로: 버전·/chat/completions 떼기, 짧은 id가 이기고 요금제는 모름, ${VAR}. → *., 루프백·통째 템플릿 버림
    assert routes["api.openai.com"] == ("openai", "-")
    assert routes["openrouter.ai/api"] == ("openrouter", "-") and routes["openrouter.ai"] == ("openrouter", "-")
    assert routes["api.minimax.io/anthropic"] == ("minimax", "-")
    assert routes["api.z.ai/api/coding/paas"] == ("zai-coding-plan", "sub") and routes["api.z.ai"] == ("zai-coding-plan", "-")
    assert routes["*.services.ai.azure.com"] == ("azure", "-")
    assert routes["api.cloudflare.com"] == ("cf", "-")
    assert not any(k.startswith(("127.", "$")) or "${" in k for k in routes)
    assert routes["zai-coding-plan"] == ("zai-coding-plan", "sub") and routes["minimax"] == ("minimax", "-")
    assert routes["ollama"] == ("local", "local")

    # 지키는 조건
    pt, rt = render_prices(rows), render_routes(routes)
    assert problems(rows, routes, pt, rt, min_rows=5) == [], problems(rows, routes, pt, rt, min_rows=5)
    assert problems(rows, routes, pt, rt) == [f"only {len(rows)} price rows (< {MIN_ROWS})"]
    bad = dict(rows)
    bad["x"] = row("bad id", "Acme", "c", {"input": -1, "output": float("inf"), "cache_read": None, "cache_write": None, "context": None, "tier": None})
    errs = problems(bad, {**routes, "h.example": ("Bad_Id", "paid")}, pt, rt + "x" * BUDGET, min_rows=5)
    for want in ("bad model id", "bad vendor", "bad number -1", "bad number inf", "bad provider id", "bad plan hint", "embedded tables"):
        assert any(want in e for e in errs), (want, errs)
    no_google = {k: r for k, r in rows.items() if r["vendor"] != "google"}
    assert any("google missing" in e for e in problems(no_google, routes, pt, rt, min_rows=5))

    # 렌더 → 읽기가 같고, --curated-only를 다시 돌려도 같다
    again, _ = parse_prices(pt)
    assert render_prices({norm(r["id"]): r for r in again}) == pt
    _, rows2, routes2 = build(None, None, curated, curated_routes, pt, rt)
    assert render_prices(rows2) == pt and render_routes(routes2) == rt
    # 손본 행에서 뺀 c 행은 k로 남는다
    _, rows3, _ = build(None, None, "", "", pt, rt)
    assert {r["id"]: r["src"] for r in rows3.values()}["x-alias"] == "k"
    print("self-test ok")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--models-dev", default=MODELS_DEV)
    ap.add_argument("--litellm", default=LITELLM)
    ap.add_argument("--curated-only", action="store_true")
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()

    read = lambda p: p.read_text() if p.exists() else ""
    prices_path, routes_path = DATA / "prices.tsv", DATA / "routes.tsv"
    md = ll = None
    if not a.curated_only:
        md, ll = load(a.models_dev), load(a.litellm)
    old, rows, routes = build(md, ll, read(CURATED / "curated.tsv"), read(CURATED / "curated-routes.tsv"),
                              read(prices_path), read(routes_path))
    prices_text, routes_text = render_prices(rows), render_routes(routes)
    errs = problems(rows, routes, prices_text, routes_text)
    if errs:
        print("\n".join(errs), file=sys.stderr)
        return 1
    moves = large_moves(old, rows)
    if moves:
        print("## Large moves\n\n" + "\n".join(moves))
    src = {}
    for r in rows.values():
        src[r["src"]] = src.get(r["src"], 0) + 1
    print(f"prices: {len(rows)} rows {dict(sorted(src.items()))}, routes: {len(routes)} rows, "
          f"{len(prices_text.encode()) + len(routes_text.encode())} bytes", file=sys.stderr)
    if a.check:
        stale = [p.name for p, t in ((prices_path, prices_text), (routes_path, routes_text)) if read(p) != t]
        if stale:
            print(f"out of date: {', '.join(stale)} (run python3 scripts/gen-prices.py --curated-only)", file=sys.stderr)
            return 1
        return 0
    DATA.mkdir(parents=True, exist_ok=True)
    prices_path.write_text(prices_text)
    routes_path.write_text(routes_text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
