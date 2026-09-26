#!/usr/bin/env python3
"""Prototype of the proposed runtime matcher (mirrors what pricing.rs would do).

usage: python3 scripts/research/match_proto.py [prices.tsv [names-file]]   (names-file: one raw model id per line)
Without names-file runs the 25-case self-check (silent on success). Without prices.tsv
the index is SAMPLE, the prototype table's labels near the self-check cases.
"""
import re, sys

DATE = re.compile(r"[-@](\d{8}|\d{4}-\d{2}-\d{2})$")          # -20251101, @20251101, -2025-04-14
WRAP = re.compile(r"^(?:arn:aws:bedrock:[^/]+/)?(?:(?:us|eu|apac|global|jp|au)\.)?"
                  r"(?:anthropic|moonshot|moonshotai|deepseek|qwen|openai|minimax|zai|mistral|meta|cohere|xai|google)\.")
TAIL = re.compile(r"(?:-v\d+(?::\d+)?|:[a-z0-9-]+|\[[^\]]*\])$")  # bedrock -v1:0, openrouter :free, claude [1m]
EFFORT = re.compile(r"-(?:xhigh|high|medium|low|minimal|thinking|fast)$")

def norm(s):
    return re.sub(r"[._@: ]", "-", s.strip().lower())

SAMPLE = """
claude-opus-4-1-20250805 claude-opus-4-20250514 claude-opus-4-5 claude-opus-4-5-20251101 claude-opus-4-6
claude-opus-4-6-20260205 claude-opus-4-7 claude-opus-4-7-20260416 claude-opus-4-8 claude-opus-4.1 claude-opus-5
claude-opus-5-5 claude-sonnet-4-5 claude-sonnet-4-5-20250929 deepseek-chat deepseek-chat-v3-0324 deepseek-chat-v3.1
gemini-2.5-pro gemini-2.5-pro-preview gemini-2.5-pro-preview-tts glm-4.6 glm-4.6v gpt-5-codex gpt-5.3-chat
gpt-5.3-chat-latest gpt-5.3-codex gpt-5.3-codex-spark gpt-5.5 gpt-5.5-2026-04-23 gpt-5.5-2026-04-24 gpt-5.5-cyber
gpt-5.5-pro gpt-5.5-pro-2026-04-23 grok-code-fast grok-code-fast-1 grok-code-fast-1-0825 kimi-k2 kimi-k2-0905
kimi-k2-thinking kimi-k2.5 kimi-k2.6 kimi-k2.7-code kimi-k2.7-code-highspeed MiniMax-M2 qwen3-coder-plus
""".split()

def load(lines):
    idx = {}
    for line in lines:
        if line.startswith("#"):
            continue
        label = line.split("\t", 1)[0]
        idx.setdefault(norm(label), label)
        base = DATE.sub("", label)
        if base != label:
            idx.setdefault(norm(base), base)      # date-less alias -> public, derived from a public id
    return idx

def match(raw, idx):
    s = raw.strip()
    s = s.rsplit("/", 1)[-1]                       # provider/org path prefix, ARN profile path
    s = WRAP.sub("", s)
    for _ in range(3):
        s2 = TAIL.sub("", s)
        if s2 == s:
            break
        s = s2
    n = norm(s)
    for cand in (n, norm(DATE.sub("", s))):
        if cand in idx:
            return idx[cand]
    e = EFFORT.sub("", n)
    if e != n and e in idx:
        return idx[e]
    # longest key that is a boundary prefix of the name; never across a version digit
    best = None
    for k in idx:
        if len(k) < 5 or not re.search(r"\d", k) or not n.startswith(k + "-"):
            continue
        rest = n[len(k) + 1:]
        if k[-1].isdigit() and re.match(r"\d{1,3}(-|$)", rest):
            continue                               # claude-opus-4 must not eat claude-opus-4-5; dates (4+ digits) may follow
        if best is None or len(k) > len(best):
            best = k
    return idx[best] if best else None

if __name__ == "__main__":
    idx = load(open(sys.argv[1]) if len(sys.argv) > 1 else SAMPLE)
    if len(sys.argv) > 2:
        names = [l.strip() for l in open(sys.argv[2]) if l.strip()]
        hit = sum(1 for n in names if match(n, idx))
        print(f"distinct={len(names)} matched={hit} unmatched={len(names) - hit}")
        sys.exit(0)
    cases = {
        "claude-opus-4-5-20251101": "claude-opus-4-5-20251101",
        "anthropic/claude-sonnet-4.5": "claude-sonnet-4-5",
        "gpt-5-codex": "gpt-5-codex",
        "gemini-2.5-pro": "gemini-2.5-pro",
        "glm-4.6": "glm-4.6",
        "kimi-k2": "kimi-k2",
        "qwen3-coder-plus": "qwen3-coder-plus",
        "deepseek-chat": "deepseek-chat",
        "grok-code-fast-1": "grok-code-fast-1",
        "MiniMax-M2": "MiniMax-M2",
        "claude-opus-4-1": "claude-opus-4-1",
        "us.anthropic.claude-opus-4-1-20250805-v1:0": "claude-opus-4-1-20250805",
        "claude-opus-4-5@20251101": "claude-opus-4-5-20251101",
        "claude-opus-4-5[1m]": "claude-opus-4-5",
        "openrouter/moonshotai/kimi-k2:free": "kimi-k2",
        "gpt-5.3-codex-high": "gpt-5.3-codex",
        "z-ai/glm-4.6": "glm-4.6",
        "kimi-k2-0905-preview": "kimi-k2-0905",
        "kimi-k2-1234-preview": "kimi-k2",
        "claude-opus-4-9": None,
        "gpt-5.5-2026-04-23": "gpt-5.5-2026-04-23",
        "gpt-5.5-2031-01-01": "gpt-5.5",
        "acme-internal-llm": None,
        "corp-gpt": None,
        "claude": None,
    }
    for raw, want in cases.items():
        got = match(raw, idx)
        assert got == want, (raw, got, want)
