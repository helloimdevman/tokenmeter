# TokenMeter

**여러 AI 코딩 에이전트가 일하는지, 기다리는지, 컨텍스트가 찼는지 한눈에.**

TokenMeter는 Claude Code, Codex, OpenCode, Cursor를 위한 로컬 우선 데스크톱 미터입니다. 여러 세션을 **확인·작업·대기·종료** 상태로 보여주고 사용량 히스토리를 로컬에 기록합니다.

[English](README.md) · [상세 레퍼런스](docs/reference.ko.md) · [새 에이전트 추가](docs/add-service.md)

```text
┌────────────────────────────────────────────────────┐
│ TOKENMETER                                    오늘 │
│ 478 전체 출력 tok/s                 API 환산 $15.8599 │
│ ██████████████░░░░░░░░░░░░░░░░░░░░░░░░           ▏ │
│ 입력 1.2M         출력 84.0k        캐시 23.5M     │
├────────────────────────────────────────────────────┤
│ 상태   프로젝트       메인/s      누적    컨텍스트  │
│ 작업   api-server       412/s      84.0k      31%  │
│ 대기   web-client          —      21.7k      75%  │
│ 확인   mobile              3/s       8.1k  95% · 높음 │
└────────────────────────────────────────────────────┘
```

## TokenMeter를 쓰는 이유

- **무엇이 실제로 일하는지 확인합니다.** 세션을 확인·작업·대기·종료 상태로 보여줍니다.
- **컨텍스트 압력을 봅니다.** 컨텍스트 점유율이 70%, 90%를 넘으면 색이 바뀝니다. Context Runway나 압축 시점 예측은 구현하지 않았습니다.
- **사용량을 로컬에서 이해합니다.** 토큰, API 환산 비용, 캐시 절감, 프로젝트, 모델, 일별 기록을 확인합니다.
- **터미널을 계속 보지 않아도 됩니다.** 세션이 명시적으로 `확인`으로 전환될 때만 데스크톱 알림이 옵니다.

TokenMeter는 로컬 에이전트 로그를 읽습니다. API 키가 필요 없고 프롬프트 내용도 저장하지 않습니다. 측정 자체는 로컬에서만 이뤄집니다. 선택형 한도 화면은 이미 로그인된 Claude, Codex, Grok 자격 증명으로 잔여 플랜 창을 읽습니다.

## 설치

요구사항은 **macOS 또는 Linux**입니다(Windows는 지원하지 않음). TokenMeter는 네이티브 바이너리 두 개(`tokenmeter`, `tokenmeter-hook`)입니다. 설치 스크립트가 최신 GitHub 릴리스에서 둘을 받아 릴리스의 `SHA256SUMS`로 확인하고, `~/.local/bin`(바꾸려면 `TOKENMETER_INSTALL_DIR`)에 넣은 뒤 `tokenmeter install`을 실행합니다.

```bash
curl -fsSL https://raw.githubusercontent.com/helloimdevman/tokenmeter/main/install.sh | sh
```

첫 측정을 활성화합니다.

1. 이미 켜 둔 Claude Code, Codex, OpenCode, Cursor는 완전히 다시 엽니다. 재시작 전 세션은 재지 않습니다.
2. 미터 창이 바로 뜹니다. 안 보이면 `tokenmeter doctor`.
3. 새 프롬프트를 실행하면 그 세션부터 측정됩니다. Cursor는 상태만 보입니다.

기존 TokenPet 훅은 감지해 제자리에서 교체합니다.

## 지원 에이전트

| 에이전트 | 로컬 사용량 | 자동 생명주기 훅 |
|---|---:|---:|
| Claude Code | 지원 | 지원 |
| Codex | 지원 | 지원 |
| OpenCode | 지원 | 지원, 플러그인 자동 생성 |
| Grok CLI | 지원 | 지원, `~/.grok/hooks` 전용 훅. Claude compat 훅은 세션 id 가 같을 때만 Grok 으로 옮긴다 |
| Cursor (IDE · CLI) | **상태만** (`확인`/`작업`/`대기`/`종료`) — 토큰·비용 합계 없음 | 지원, `~/.cursor/hooks.json`. CLI 는 `stop` 사용량 훅을 주지 않는다 |

로그가 있는 다른 에이전트도 설정만으로 추가할 수 있습니다. [서비스 추가 가이드](docs/add-service.md)를 참고하세요.

## 주요 명령

```bash
tokenmeter status --json
tokenmeter watch --jsonl
tokenmeter receipt --format markdown
tokenmeter adapter init gemini-cli --log ~/.gemini/tmp
tokenmeter adapter check ./gemini-cli-adapter
tokenmeter share on|off|preview   # 익명 사용 통계(동의할 때만)
tokenmeter account delete         # 이 기기가 보낸 데이터 삭제
tokenmeter league login           # Token League: GitHub 로그인(기기 코드)
tokenmeter league open            # 방을 열고 초대 링크 출력
tokenmeter league join <링크>     # 친구의 방에 참가
tokenmeter quota                  # Claude/Codex/Grok 잔여 한도
tokenmeter services               # 로그 감지와 훅 상태
tokenmeter doctor                 # 파서와 설치 검증
tokenmeter meter off              # 창만 숨기고 측정은 유지
tokenmeter update on              # 하루 한 번 정식 릴리스 자동 업데이트
tokenmeter off                    # 측정을 멈추고 훅은 유지
tokenmeter on                     # 측정 재개
tokenmeter uninstall              # TokenMeter 훅만 제거
tokenmeter uninstall --purge      # 훅+데몬+로컬 상태+리그 토큰
tokenmeter doctor --json          # 지원용 (홈 경로·프롬프트 없음)
```

오버레이는 드래그로 옮깁니다. 휠은 목록 행 위에서 목록을 스크롤하고, 그 밖에서는 창 배율을 바꿉니다. 화면의 `S/M/L`로 심플(계기판만)·보통(세션·프로젝트·한도)·상세(속도·일별까지)를 바로 오갑니다. `⌘K`/`Ctrl+K`는 선택형 빠른 검색입니다. 테마, 투명도·모션 감소는 `⋯` 또는 우클릭으로 열리는 설정 창에 있습니다. `×`는 오버레이만 숨기며 측정은 계속됩니다. macOS에서는 메뉴바에도 짧은 LED 막대로 떠 있습니다(왼쪽 클릭 = 창 접기·펴기, 오른쪽 클릭 = 메뉴). Dock 아이콘과 ⌘Q는 없습니다. 측정까지 끝내려면 설정이나 메뉴바 메뉴의 `TokenMeter 종료 · 측정 중지`를 고릅니다.

계기판의 **전체 출력**은 서브에이전트를 포함한 출력 처리량입니다. 세션 칸 **메인**은 서브에이전트를 뺀 메인 모델 처리량입니다. 둘 다 제공자의 실제 스트리밍 생성 속도 벤치마크가 아니라 로그 델타 도착률입니다. 세션 표는 상태·누적 출력·컨텍스트 점유율을 서로 다른 칸에 표시합니다. 소셜 면은 Token League입니다. 남은 자체 호스팅 `leaderboard.endpoint` / `team` 명령은 [레퍼런스](docs/reference.ko.md)에만 있고, endpoint가 없으면 숨습니다.

## 에이전트 스킬

선택형 스킬을 설치하면 호환되는 코딩 에이전트가 자연어로 TokenMeter를 조작할 수 있습니다.

```bash
npx skills add helloimdevman/tokenmeter -g -a claude-code
```

`/tm`, `/tm-meter`, `/tm-measure`, `/tm-doctor`가 추가됩니다.

## 개인정보와 데이터

- 측정은 에이전트 로그를 **읽습니다** (로그에 프롬프트가 있을 수 있음). **저장**하는 것은 허용된 메타데이터뿐입니다. 프롬프트·응답·툴 명령·파일명은 남기지 않습니다.
- 공개 JSON과 팀 출력은 내부 경로·세션 ID·라우팅 URL·세션 내용을 뺍니다.
- macOS 상태 경로: `~/Library/Application Support/tokenmeter`. Linux: `${XDG_STATE_HOME:-~/.local/state}/tokenmeter`.
- 사용자 설정: `${XDG_CONFIG_HOME:-~/.config}/tokenmeter`.
- 한도(`tokenmeter quota`)는 이미 저장된 Claude/Codex/Grok 자격 증명으로 잔여 창만 읽습니다. 세션 로그는 보내지 않습니다.
- 금액은 **API 환산 추정**입니다. 청구서가 아닙니다.
- 익명 사용 통계는 동의할 때만 보냅니다. 설치할 때 한 번 묻고, `tokenmeter share on|off`로 바꿉니다. 공유를 켠 시간의 칸부터(그 칸은 통째로 가고, 그 전에 끝난 칸은 보내지 않으며, 껐다 켜면 그 시간부터 다시) 도구·경로 라벨·모델 계열별 시간당 토큰 수를 TokenMeter 서버로 보내며, 프롬프트·코드·경로·프로젝트명·세션 ID·사설 호스트 이름·사용자 모델 이름은 보내지 않습니다. `tokenmeter share preview`는 다음에 보낼 내용을 보여 주고, `tokenmeter account delete`는 보낸 데이터를 지웁니다. 자세한 내용: [docs/protocol](docs/protocol/README.md)
- Token League도 직접 켤 때만 씁니다. `tokenmeter league login`은 GitHub로 로그인하며, GitHub 토큰은 서버가 한 번 확인하고 폐기할 뿐 저장하지 않습니다. 서버는 GitHub id와 login, 들어간 방, 기기마다 iroh EndpointId를 갖고, 공유가 꺼져 있으면 경기 계산용으로 한 시간에 합계 셀 하나만 받습니다. 방 멤버는 내 login과 실시간 출력 속도를 봅니다. 미터가 방에 있는 동안, 내 EndpointId를 아는 사람(지금이나 예전의 방 멤버)은 연결할 때 내 공인 IP와 로컬 주소를 받습니다. 직접 연결 여부와는 상관없고, 방을 나가거나 로그아웃하면 키를 바꿉니다. `tokenmeter league logout`은 이 기기의 연결을 끊고, `tokenmeter account delete`는 계정을 지웁니다.
- 레거시 `leaderboard.endpoint`는 직접 켜기 전까지 꺼져 있습니다.

## Token League

Token League는 공유 방에서 친구의 실시간 출력 속도를 보여 줍니다. `tokenmeter league login`으로 GitHub 로그인을 하고, `tokenmeter league open`이 초대 링크(`https://tokenmeter.online/j/<id>`)를 출력하면, 친구가 `tokenmeter league join <링크>`로 들어옵니다. 오버레이의 리그 판에 멤버마다 tok/s가 뜹니다. 값은 멤버의 미터끼리 QUIC(iroh)으로 직접 오가고, 직접 경로가 막히면 TokenMeter 릴레이를 거칩니다. 서버는 방 목록만 갖습니다. 방 하나에 20명, 한 사람이 방 8개까지입니다. `tokenmeter league`는 내 방과 초대 링크를 보여 주고, `leave`·`close`로 나가거나 닫습니다. macOS 방화벽을 켜 두었다면 들어오는 연결 허용 창이 뜰 수 있습니다. 거부해도 실시간 값은 릴레이로 오갑니다.

## 업데이트와 제거

자동 업데이트는 기본적으로 꺼져 있습니다. 데몬 시작 시 하루 한 번 확인하도록 켜거나 지금 바로 업데이트할 수 있습니다.

```bash
tokenmeter update on
tokenmeter update now
```

GitHub의 정식 릴리스만, 두 바이너리가 릴리스 `SHA256SUMS`와 맞을 때만 제자리에서 교체합니다. 다시 끄려면 `tokenmeter update off`를 실행합니다. 설치 스크립트를 다시 실행해도 업데이트됩니다.

바이너리 두 개를 지우기 전에 훅을 먼저 제거합니다.

```bash
tokenmeter uninstall
rm ~/.local/bin/tokenmeter ~/.local/bin/tokenmeter-hook
```

훅과 로컬 상태·리그 토큰까지:

```bash
tokenmeter uninstall --purge
```

## 개발

```bash
git clone https://github.com/helloimdevman/tokenmeter.git
cd tokenmeter
cargo test --manifest-path native/Cargo.toml
cargo build --release --manifest-path native/Cargo.toml
./native/target/release/tokenmeter install --dry-run
```

## 기여하기

새 에이전트 어댑터, 프로바이더 등록, 버그 제보와 수정을 환영합니다. [CONTRIBUTING.md](CONTRIBUTING.md)부터 읽어 주세요. 질문은 [Discussions](https://github.com/helloimdevman/tokenmeter/discussions)에 남기면 됩니다. 보안 문제는 [SECURITY.md](SECURITY.md)에 적힌 대로 비공개로 알려 주세요.

TokenMeter는 imdevman이 만들고 관리하며 [MIT 라이선스](LICENSE)로 배포됩니다.
