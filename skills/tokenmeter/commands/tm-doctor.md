---
description: TokenMeter 진단 — 훅이 어디에 붙었나 + 데몬 로그 꼬리
allowed-tools: Bash(~/.claude/skills/tokenmeter/tm:*), Bash(tail:*)
---

!`~/.claude/skills/tokenmeter/tm services`

!`~/.claude/skills/tokenmeter/tm status | head -8`

로그 꼬리:
!`tail -15 "$(~/.claude/skills/tokenmeter/tm status | sed -n 's/^  로그   : //p')"`

지원 붙여넣기가 필요하면 `tokenmeter doctor --json` 만 쓴다 (홈 경로·프롬프트 없음).

위 출력만 보고 진단하라. 훅이 `갱신 필요` 면 `install` 재실행, `네이티브 미터 없음` 이면
GitHub Release 바이너리 또는 체크아웃에서 `cargo build --release --bins`, `자동 표시 꺼짐` 이면
`/tm-meter on`. 리그 줄이 `로그인 안 함`이면 `tokenmeter league login`. 짚이는 게 없으면 없다고 말하라.
추측으로 원인을 만들어내지 마라.
