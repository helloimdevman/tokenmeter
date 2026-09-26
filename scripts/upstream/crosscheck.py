#!/usr/bin/env python3
"""fixture 합계를 고정한 ccusage와 맞춘다(스펙 12절, 표준 라이브러리만).

usage: crosscheck.py [ids...] [--list] [--report out.json]

tests/fixtures/<id>/expected.json 중 `ccusage` 칸이 있는 것만 돈다. files/와 step-2/를
임시 HOME에 펼치고(`*.sql`은 확장자를 뗀 DB에 실행, src/fixture.rs와 같은 배치)
`npx --yes ccusage@<잠금 버전> <cmd> daily --json --offline`의 totals를
`ccusage.totals`(없으면 마지막 단계)와 비교한다. 다르면 종료 코드 1.
"""

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# 우리 칸 → ccusage totals 칸
FIELDS = {
    "input": "inputTokens",
    "output": "outputTokens",
    "cache_read": "cacheReadTokens",
    "cache_write": "cacheCreationTokens",
}
# 사용자 환경에서 넘기는 것. 에이전트 변수(CLAUDE_CONFIG_DIR …)가 실데이터를 가리키지 않게 나머지는 버린다.
KEEP = {"PATH", "TMPDIR", "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"}


def npm_pin(name="ccusage-npm"):
    for line in (ROOT / "scripts/upstream/upstream.lock").read_text().splitlines():
        cols = line.split("\t")
        if cols[0] == name:
            return f"{cols[1]}@{cols[2]}"
    sys.exit(f"upstream.lock: no {name} line")


def lay(src, dst):
    """src 아래 파일을 dst로 복사한다. `*.sql`은 확장자를 뗀 경로의 DB에 실행한다(있으면 행 갱신)."""
    for f in sorted(p for p in src.rglob("*") if p.is_file()):
        target = dst / f.relative_to(src)
        target.parent.mkdir(parents=True, exist_ok=True)
        if f.suffix == ".sql":
            con = sqlite3.connect(target.with_suffix(""))
            con.executescript(f.read_text())
            con.close()
        else:
            shutil.copyfile(f, target)


def ccusage_totals(fixture, cc, pkg):
    with tempfile.TemporaryDirectory(prefix="tokenmeter-crosscheck-") as tmp:
        home = Path(tmp) / "home"
        home.mkdir()
        lay(fixture / "files", home)
        lay(fixture / "step-2", home)
        env = {k: v for k, v in os.environ.items() if k in KEEP or k.lower().startswith("npm_config_")}
        # HOME을 옮겨도 npx가 매번 받지 않게 npm 캐시는 원래 자리를 쓴다.
        env.setdefault("npm_config_cache", str(Path.home() / ".npm"))
        env.update(
            HOME=str(home),
            XDG_CONFIG_HOME=str(home / ".config"),
            XDG_DATA_HOME=str(home / ".local/share"),
            XDG_STATE_HOME=str(home / ".local/state"),
            XDG_CACHE_HOME=str(home / ".cache"),
        )
        env.update({k: v.replace("$HOME", str(home)) for k, v in cc.get("env", {}).items()})
        cmd = ["npx", "--yes", pkg, cc["cmd"], "daily", "--json", "--offline"]
        out = subprocess.run(cmd, env=env, capture_output=True, text=True)
    try:
        totals = json.loads(out.stdout)["totals"]
    except (ValueError, KeyError, TypeError):
        lines = [l for l in (out.stderr or out.stdout).splitlines() if l.strip()]
        raise RuntimeError(" | ".join(lines) or f"exit {out.returncode}")
    return {ours: totals.get(theirs, 0) for ours, theirs in FIELDS.items()}


def fixtures(ids):
    found = {}
    for path in sorted((ROOT / "tests/fixtures").glob("*/expected.json")):
        expected = json.loads(path.read_text())
        if "ccusage" in expected and (not ids or path.parent.name in ids):
            found[path.parent.name] = expected
    missing = set(ids) - set(found)
    if missing:
        sys.exit(f"no ccusage field: {', '.join(sorted(missing))}")
    return found


def compact(d):
    return json.dumps(d, separators=(",", ":"))


def main():
    ap = argparse.ArgumentParser(usage="crosscheck.py [ids...] [--list] [--report out.json]")
    ap.add_argument("ids", nargs="*")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--report")
    args = ap.parse_args()

    found = fixtures(args.ids)
    if args.list:
        for fid, expected in found.items():
            print(f"{fid}\t{expected['ccusage']['cmd']}")
        return 0

    pkg, report, failed = npm_pin(), [], False
    for fid, expected in found.items():
        cc = expected["ccusage"]
        want = cc.get("totals") or expected["steps"][-1]
        ours = {k: want[k] for k in FIELDS if k in want}
        row = {"id": fid, "cmd": cc["cmd"], "ours": ours}
        try:
            theirs = ccusage_totals(ROOT / "tests/fixtures" / fid, cc, pkg)
            row["ccusage"] = {k: theirs[k] for k in ours}
            row["ok"] = row["ccusage"] == ours
            print(f"ok {fid}" if row["ok"] else f"DIFF {fid} ours={compact(ours)} ccusage={compact(row['ccusage'])}")
        except RuntimeError as e:
            row.update(ok=False, error=str(e))
            print(f"DIFF {fid} ours={compact(ours)} ccusage=error: {e}")
        failed |= not row["ok"]
        report.append(row)

    if args.report:
        Path(args.report).write_text(json.dumps(report, indent=2) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
