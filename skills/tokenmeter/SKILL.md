---
name: tokenmeter
description: "Install, toggle, and inspect TokenMeter — the local live meter for Claude Code, Codex, OpenCode, and Cursor sessions. Use for token usage, costs, session activity, overlay controls, hook installation, and diagnostics. Korean triggers — 토큰미터, 토큰 미터, 미터기 켜줘/꺼줘, 토큰 측정 켜줘/꺼줘, 토큰 얼마나 썼어, 토큰 사용량, 오버레이 안 떠, 훅 설치해줘."
---

# TokenMeter

Claude Code, Codex, OpenCode, Cursor의 로컬 로그(또는 훅)를 읽어 사용량·비용·라이브 세션을 보여주는 계량기입니다.

## 빠른 명령

| 커맨드 | 하는 일 |
|---|---|
| `/tm` | 사용량·비용·히스토리와 receipt·adapter·league 요청 |
| `/tm-meter on\|off` | 미터 창만 켜고 끔; 측정은 계속 |
| `/tm-measure on\|off` | 측정 자체를 켜고 끔 |
| `/tm-doctor` | 훅·데몬·로그 진단. 이슈에 붙일 때는 `tokenmeter doctor --json` |

모든 커맨드는 `~/.claude/skills/tokenmeter/tm`을 거쳐 설치된 `tokenmeter` 명령을 실행합니다.

## 설치

`tokenmeter` 명령이 없을 때만 다음을 실행합니다.

```bash
curl -fsSL https://raw.githubusercontent.com/helloimdevman/tokenmeter/main/install.sh | sh
```

스크립트가 두 바이너리를 `~/.local/bin`에 넣고 `tokenmeter install`까지 실행합니다.

설치하면 미터 창이 바로 뜹니다. 이미 켜 둔 에이전트(Cursor 포함)는 한 번 완전히 다시 열고
프롬프트를 실행해야 그 세션이 측정됩니다. 확인은 `tokenmeter status`,
문제가 있으면 `tokenmeter doctor`입니다.

## 명령

| 하고 싶은 것 | 명령 |
|---|---|
| 훅 설치 | `tokenmeter install` |
| 훅 해제 | `tokenmeter uninstall` / `uninstall --purge` |
| 측정 끄기/켜기 | `tokenmeter off` / `tokenmeter on` |
| 특정 에이전트 제외 | `tokenmeter off --service codex` |
| 미터 창 끄기/켜기 | `tokenmeter meter off` / `tokenmeter meter on` |
| 사용량·비용 | `tokenmeter status` |
| 공개 상태 JSON | `tokenmeter status --json` |
| 읽기 전용 JSONL 감시 | `tokenmeter watch --jsonl` |
| 최근 세션 영수증 | `tokenmeter receipt --format text\|markdown\|json` |
| 서비스 어댑터 초안 | `tokenmeter adapter init NAME --log PATH` / `tokenmeter adapter check PATH` |
| Token League | `tokenmeter league login` / `open` / `join <id>` |
| 훅 현황 | `tokenmeter services` |
| 진단 | `tokenmeter doctor` / `tokenmeter doctor --json` |
| 모델 단가 | `tokenmeter price` / `tokenmeter price set <모델> --input N --output N` |

서비스 이름은 `claude-code`, `codex`, `opencode`, `grok`, `cursor`입니다.

## 세 가지 끄기

| 요청 | 명령 | 효과 |
|---|---|---|
| 미터기 꺼줘 | `tokenmeter meter off` | 창만 숨김; 측정 계속 |
| 토큰 그만 재 | `tokenmeter off` | 측정과 데몬 정지; 훅 유지 |
| 완전히 제거해 | `tokenmeter uninstall --purge` | 훅+데몬+로컬 상태+기기 토큰. 0으로 끝나면 `tokenmeter`·`tokenmeter-hook` 바이너리를 지웁니다. 1로 끝나면(서버 데이터 삭제 실패) 상태 폴더가 남았으니 바이너리를 지우지 말고 출력된 이유와 재시도 안내를 그대로 전합니다 |

사용자가 단순히 “꺼줘”라고 하면 셋 중 무엇인지 확인합니다.

## 문제 해결

먼저 `tokenmeter status`, `tokenmeter services`, `tokenmeter doctor`를 순서대로 실행합니다.

- `갱신 필요`: `tokenmeter install`을 다시 실행합니다.
- `자동 표시 꺼짐`: `tokenmeter meter on`을 실행합니다.
- `네이티브 훅이 없습니다`: 설치 스크립트를 다시 실행합니다. 체크아웃이면 `cargo build --release --manifest-path native/Cargo.toml`.
- `가격표에 없는 모델`: `tokenmeter price set <모델> --input N --output N`으로 단가를 못 박습니다.
- `리그    : 로그인 안 함`: `tokenmeter league login`. 오버레이에서 로그인하지 않습니다.
- Cursor는 상태만 잽니다. 사용량 숫자는 약속하지 않습니다.
- 금액은 API 환산 추정입니다. 청구서가 아닙니다. Windows는 지원하지 않습니다.
- 훅은 항상 exit 0이므로 에이전트 작업을 막지 않습니다. 원인을 추측하지 말고 진단 출력을 읽습니다.

`~/.claude/settings.json`, `~/.codex/hooks.json`, `~/.cursor/hooks.json`을 직접 편집하지 않습니다.
설치·해제 명령은 TokenMeter 엔트리만 변경하고 `.bak-tokenmeter` 백업을 남깁니다.
