#!/usr/bin/env python3
"""우리 `doctor <id> --since … --json`의 `days`와 ccusage `daily --json`을 날짜마다 비교한다(스펙 13절).

usage: compare_daily.py OURS CCUSAGE [--tolerance 0.001]

칸마다 우리/ccusage 비를 찍고, 한쪽에만 있는 날이나 허용 오차(상대)를 넘는 칸이 있으면 종료 코드 1.
"""

import argparse
import json
import sys

# 우리 칸 → ccusage daily[] 칸
FIELDS = {
    "input": "inputTokens",
    "output": "outputTokens",
    "cache_read": "cacheReadTokens",
    "cache_write": "cacheCreationTokens",
}


def off(ours, theirs, tolerance):
    return abs(ours - theirs) > tolerance * max(abs(theirs), 1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ours")
    ap.add_argument("ccusage")
    ap.add_argument("--tolerance", type=float, default=0.001)
    a = ap.parse_args()
    with open(a.ours) as f:
        ours = json.load(f).get("days", {})
    with open(a.ccusage) as f:
        theirs = {d["date"]: d for d in json.load(f).get("daily", [])}
    bad = False
    print("date        " + "".join(f"{k:>26}" for k in FIELDS))
    for date in sorted(set(ours) | set(theirs)):
        mine, cc = ours.get(date, {}), theirs.get(date, {})
        cells = []
        for k, ck in FIELDS.items():
            o, t = mine.get(k, 0), cc.get(ck, 0)
            ratio = f"{o / t:.4f}x" if t else ("=" if o == 0 else "only-ours")
            miss = off(o, t, a.tolerance)
            bad |= miss
            cells.append(f"{o:>10} {t:>10} {ratio:>4}{'!' if miss else ' '}")
        print(f"{date}  " + "".join(f"{c:>26}" for c in cells))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
