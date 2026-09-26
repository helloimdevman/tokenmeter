#!/usr/bin/env python3
"""Codex 실데이터 비교의 알려진 차이를 날짜별로 낸다(계획 Task 3.B, 스펙 13절).

usage: codex_diff.py --since YYYY-MM-DD [--until YYYY-MM-DD] [--root ~/.codex/sessions]

- compaction: `compacted` 줄의 `latest_token_usage_record`(compaction_response_id마다 한 번). 우리만 센다.
- replay: 포크·서브에이전트 파일이 부모 기록을 재생한 `token_count` 줄 중 ccusage와 우리가 다르게 센 분
  (ccusage 값 − 우리 값). 우리는 머리 줄 뒤 1초 안의 줄(`replay_gate: 1`)과 다른 파일에서 먼저 본
  누적치 키를 거른다. ccusage(@2d8dea8)는 부모 기록과 앞부분이 같은 줄만 거르고, 부모가 없으면
  고쳐 쓴 묶음(1초 간격)을 거른다(`replay.rs`, `parser.rs:84-220`). 그 규칙을 여기서 흉내 낸다.

날짜는 레코드 시각의 로컬 날짜, 파일은 `doctor --since`처럼 mtime이 시작일 이후인 것을 경로 순으로 읽는다.
칸은 우리 칸(input은 두 캐시를 뺀 값). 경로나 내용은 출력하지 않는다.
"""

import argparse
import json
import os
import sys
from datetime import datetime
from pathlib import Path

CELLS = ("input", "cache_read", "cache_write", "output")
GATE = 1.0  # adapters/codex.yaml replay_gate
BURST_MS = 1000  # ccusage CODEX_REWRITTEN_BURST_PAUSE_MS


def cells(u):
    u = u or {}
    n = lambda k: int(u.get(k) or 0)
    inp, cr, cw = n("input_tokens"), n("cached_input_tokens"), n("cache_write_input_tokens")
    if cr + cw <= inp:
        inp -= cr + cw
    return dict(zip(CELLS, (inp, cr, cw, n("output_tokens"))))


def secs(ts):
    if isinstance(ts, (int, float)):
        return ts / 1000 if ts > 1e11 else ts
    try:
        return datetime.fromisoformat(str(ts).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def local_date(ts):
    return datetime.fromtimestamp(secs(ts)).date().isoformat()


def rows(path):
    """(줄 번호, 객체) — 깨진 줄은 건너뛴다."""
    with open(path, errors="replace") as fh:
        for i, line in enumerate(fh):
            try:
                o = json.loads(line)
            except ValueError:
                continue
            if isinstance(o, dict):
                yield i, o


def token_info(o):
    p = o.get("payload")
    if o.get("type") == "event_msg" and isinstance(p, dict) and p.get("type") == "token_count":
        if isinstance(p.get("info"), dict):
            return p["info"]
    return None


def raw(u):
    u = u or {}
    n = lambda k: int(u.get(k) or 0)
    i, cr, cw = n("input_tokens"), n("cached_input_tokens"), n("cache_write_input_tokens")
    cr = min(cr, i)
    cw = min(cw, i - cr)
    return (i, cr, cw, n("output_tokens"), n("reasoning_output_tokens"), n("total_tokens"))


def cc_events(path):
    """ccusage가 거르기 전에 보는 사용량 이벤트: [(줄 번호, 시각 ms, raw)]."""
    out, prev = [], None
    for i, o in rows(path):
        info = token_info(o)
        if info is None or secs(o.get("timestamp")) is None:
            continue
        total = info.get("total_token_usage")
        advanced = total is None or total != prev
        if total is not None:
            last = info.get("last_token_usage") if advanced else None
            usage = raw(last) if last is not None else tuple(
                max(0, a - b) for a, b in zip(raw(total), raw(prev) if prev else (0,) * 6))
            prev = total
        else:
            last = info.get("last_token_usage")
            if last is None:
                continue
            usage = raw(last)
        if any(usage[:5]):
            out.append((i, round(secs(o["timestamp"]) * 1000), usage))
    return out


def burst_start(path):
    first = None
    for _, o in rows(path):
        info = token_info(o)
        if info is None or (info.get("last_token_usage") is None and info.get("total_token_usage") is None):
            continue
        t = secs(o.get("timestamp"))
        if t is None:
            continue
        t = round(t * 1000)
        if first is None:
            first = t
        else:
            return first if 0 <= t - first <= BURST_MS else None
    return None


def meta(path):
    try:
        with open(path, errors="replace") as fh:
            o = json.loads(fh.readline())
    except (OSError, ValueError):
        return None, None, None
    p = o.get("payload") if isinstance(o, dict) and o.get("type") == "session_meta" else None
    if not isinstance(p, dict):
        return None, None, None
    src = p.get("source")
    parent = p.get("forked_from_id") or (
        ((src.get("subagent") or {}).get("thread_spawn") or {}).get("parent_thread_id")
        if isinstance(src, dict) else None)
    t = secs(o.get("timestamp"))
    return p.get("id"), parent or None, None if t is None else round(t * 1000)


def cc_counted(path, by_id):
    """ccusage가 세는 줄 번호 → raw (포크 재생 거르기까지)."""
    events = cc_events(path)
    _, parent_id, forked_at = meta(path)
    if not parent_id:
        return {i: u for i, _, u in events}
    ppath = next((c for c in by_id.get(parent_id, []) if c != path), None)
    prefix = []
    if ppath:
        prefix = [(t, u) for _, t, u in cc_events(ppath)]
        if forked_at is not None:
            prefix = [(t, u) for t, u in prefix if t <= forked_at]  # 포크 뒤 부모 기록은 재생되지 않았다
    out, state, idx, prev = {}, "match", 0, None
    for i, t, u in events:
        while True:
            if state == "match":
                if idx < len(prefix) and prefix[idx][1] == u:
                    idx += 1
                    break
                prev = burst_start(path) if idx == 0 else None
                state = "burst" if prev is not None else "done"
            elif state == "burst":
                if 0 <= t - prev <= BURST_MS:
                    prev = t
                    break
                state = "done"
            else:
                out[i] = u
                break
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--since", required=True)
    ap.add_argument("--until", default="9999-12-31")
    ap.add_argument("--root", default=os.path.join(os.environ.get("CODEX_HOME") or "~/.codex", "sessions"))
    a = ap.parse_args()
    start = datetime.fromisoformat(a.since).timestamp()
    root = Path(a.root).expanduser()
    every = sorted(root.rglob("rollout-*.jsonl"))
    by_id = {}
    for f in every:
        sid = meta(f)[0]
        if sid:
            by_id.setdefault(sid, []).append(f)
    files = [p for p in every if p.stat().st_mtime >= start]

    out = {}
    owner = {}  # 누적치 키 → 처음 본 파일
    ledger = {}  # 누적치 키 → 칸별 큰 값(우리 키 장부)
    compactions = set()
    cc_seen = set()  # ccusage 전역 중복 제거(시각, 사용량)

    def add(kind, date, c, sign=1):
        if a.since <= date <= a.until:
            day = out.setdefault(date, {k: dict.fromkeys(CELLS, 0) for k in ("compaction", "replay")})
            for k in CELLS:
                day[kind][k] += sign * c[k]

    for f in files:
        counted = cc_counted(f, by_id)
        first = None
        for i, o in rows(f):
            ts = o.get("timestamp")
            if first is None and secs(ts) is not None:
                first = secs(ts)
            p = o.get("payload")
            if not isinstance(p, dict) or secs(ts) is None:
                continue
            if o.get("type") == "compacted" and isinstance(p.get("latest_token_usage_record"), dict):
                rid = p.get("compaction_response_id")
                if rid and rid not in compactions:
                    compactions.add(rid)
                    add("compaction", local_date(ts), cells(p["latest_token_usage_record"].get("usage")))
                continue
            info = token_info(o)
            if info is None:
                continue
            key = json.dumps(info.get("total_token_usage"), sort_keys=True)
            gated = secs(ts) < first + GATE
            v = cells(info.get("last_token_usage"))
            m = ledger.get(key, dict.fromkeys(CELLS, 0))
            ledger[key] = {k: max(v[k], m[k]) for k in CELLS}
            # 문턱 안의 줄은 배우기만 한다
            mine = {k: 0 if gated else max(0, v[k] - m[k]) for k in CELLS}
            other = owner.setdefault(key, f) != f
            theirs = dict.fromkeys(CELLS, 0)
            if i in counted:
                u = counted[i]
                ck = (round(secs(ts) * 1000), u)
                if ck not in cc_seen:
                    cc_seen.add(ck)
                    i_, cr, cw, out_ = u[:4]
                    theirs = dict(zip(CELLS, (i_ - cr - cw, cr, cw, out_)))
            if gated or other:
                add("replay", local_date(ts), {k: theirs[k] - mine[k] for k in CELLS})
    json.dump(dict(sorted(out.items())), sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
