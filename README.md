# TokenMeter

**See when your AI coding agents are working, waiting, or running out of context.**

TokenMeter is a local-first desktop meter for Claude Code, Codex, OpenCode, and Cursor. It discovers active sessions, shows `확인` (needs attention), `작업` (working), `대기` (waiting), or `종료` (done), and keeps usage history locally.

[한국어](README.ko.md) · [Advanced reference](docs/reference.ko.md) · [Add an agent](docs/add-service.md)

```text
┌────────────────────────────────────────────────────┐
│ TOKENMETER                                    TODAY │
│ 478 total output tok/s       API-equivalent $15.8599 │
│ ██████████████░░░░░░░░░░░░░░░░░░░░░░░░           ▏ │
│ in 1.2M         out 84.0k      cache 23.5M         │
├────────────────────────────────────────────────────┤
│ STATUS  PROJECT        MAIN/s      TOTAL    CONTEXT  │
│ 작업    api-server       412/s      84.0k      31%  │
│ 대기    web-client          —      21.7k      75%  │
│ 확인    mobile              3/s       8.1k  95% · high │
└────────────────────────────────────────────────────┘
```

## Why TokenMeter

- **Know what needs attention.** Sessions use the actual UI labels `확인`, `작업`, `대기`, and `종료`.
- **See context pressure.** Context pressure changes color at 70% and 90%; Context Runway or compaction prediction is not implemented.
- **Understand usage locally.** Inspect tokens, estimated API-equivalent cost, cache savings, projects, models, and daily history.
- **Stop watching terminals.** A desktop notification fires only on an explicit transition to `확인`.

TokenMeter reads local agent logs. It does not require an API key or store prompt contents. Metering stays local. The optional quota view uses already-logged-in Claude, Codex, or Grok credentials to read remaining plan windows.

## Install

Requirements: **macOS or Linux** (Windows is not supported). TokenMeter is two native binaries, `tokenmeter` and `tokenmeter-hook`. The install script downloads both from the latest GitHub Release, checks them against the release `SHA256SUMS`, puts them in `~/.local/bin` (override with `TOKENMETER_INSTALL_DIR`), and runs `tokenmeter install`:

```bash
curl -fsSL https://raw.githubusercontent.com/helloimdevman/tokenmeter/main/install.sh | sh
```

Then activate your first measurement:

1. Fully restart Claude Code, Codex, OpenCode, or Cursor if they were already running. Sessions started before the restart are not measured.
2. The overlay should appear immediately. If it does not, run `tokenmeter doctor`.
3. Run one new prompt so that session is measured. Cursor shows status only.

Existing TokenPet hooks are detected and replaced in place.

## Supported agents

| Agent | Local usage | Automatic lifecycle hook |
|---|---:|---:|
| Claude Code | Yes | Yes |
| Codex | Yes | Yes |
| OpenCode | Yes | Yes, generated plugin |
| Grok CLI | Yes | Yes, dedicated `~/.grok/hooks` plus Claude-compat remap when the session id matches |
| Cursor (IDE · CLI) | **Status only** (`확인`/`작업`/`대기`/`종료`) — no token or cost totals | Yes, `~/.cursor/hooks.json`. CLI omits `stop` usage hooks |

Adding another log-based agent is configuration-only. See [the service guide](docs/add-service.md).

## Commands

```bash
tokenmeter status --json
tokenmeter watch --jsonl
tokenmeter receipt --format markdown
tokenmeter adapter init gemini-cli --log ~/.gemini/tmp
tokenmeter adapter check ./gemini-cli-adapter
tokenmeter share on|off|preview   # anonymous usage stats (opt-in)
tokenmeter account delete         # delete what this device sent
tokenmeter league login           # Token League: sign in with GitHub (device code)
tokenmeter league open            # open a room and print its invite link
tokenmeter league join <link>     # join a friend's room
tokenmeter league match start --minutes 60   # host: a one-hour match (output tokens)
tokenmeter league match                      # standings (provisional until final)
tokenmeter quota                  # remaining Claude/Codex/Grok plan windows
tokenmeter services               # detected logs and hook state
tokenmeter doctor                 # validate parsers and installation
tokenmeter meter off              # hide overlay; keep measuring
tokenmeter update on              # opt in to daily stable-release updates
tokenmeter off                    # stop measuring; keep hooks
tokenmeter on                     # resume measurement
tokenmeter uninstall              # remove only TokenMeter hooks
tokenmeter uninstall --purge      # hooks + daemon + local state + league tokens
tokenmeter doctor --json          # support paste (no home paths or prompts)
```

Drag the overlay to move it. The wheel scrolls when it is over a list row and resizes the window elsewhere. Use the visible `S/M/L` controls to switch between simple (meter only), normal (Sessions, Projects, Quota), and detail (adds Speed and Daily). `⌘K`/`Ctrl+K` is an optional quick search. Theme, reduced transparency, and reduced motion live in the settings window (`⋯` or right-click). `×` hides only the overlay, so measurement continues. On macOS the meter also lives in the menu bar as a short LED bar (left-click folds or unfolds the window, right-click opens its menu), and there is no Dock icon or ⌘Q. To stop measurement, choose `TokenMeter 종료 · 측정 중지` from settings or the menu bar menu.

The global meter label is **전체 출력** (aggregate output throughput, including sub-agents). Session column **메인** excludes sub-agent output. Both rates are log-delta arrival rates, not a provider streaming benchmark. Session rows keep status, cumulative output, and context usage in separate columns. The social surface is Token League. The leftover self-hosted `leaderboard.endpoint` / `team` command stays in [the reference](docs/reference.ko.md) and is hidden while offline.

## Agent skill

The optional skill lets compatible coding agents operate TokenMeter in natural language.

```bash
npx skills add helloimdevman/tokenmeter -g -a claude-code
```

It provides `/tm`, `/tm-meter`, `/tm-measure`, and `/tm-doctor`.

## Privacy and data

- Metering **reads** agent logs (those files may contain prompts) and **stores** allowlisted metadata only: no prompt, response, tool command, or filename.
- Public JSON and team output omit internal paths, session IDs, routing URLs, and session content.
- Runtime state: `~/Library/Application Support/tokenmeter` (macOS) or `${XDG_STATE_HOME:-~/.local/state}/tokenmeter` (Linux).
- User overrides: `${XDG_CONFIG_HOME:-~/.config}/tokenmeter`.
- Quota (`tokenmeter quota`) reuses already-stored Claude/Codex/Grok credentials to read remaining plan windows. Session logs are not sent.
- Dollar amounts are **API-list estimates**, not invoices.
- Anonymous usage sharing is opt-in: the installer asks once, and `tokenmeter share on|off` changes it. It sends hourly token counts per tool, route label and model family to the TokenMeter server, from the hour you turn it on in (that hour is sent whole, hours that ended before it never are, and off and on again starts over) — never prompts, code, paths, project names, session ids, private hostnames or custom model names. `tokenmeter share preview` shows the next upload and `tokenmeter account delete` removes what was sent. Details: [docs/protocol](docs/protocol/README.md).
- Token League is opt-in too. `tokenmeter league login` signs in with GitHub; the GitHub token is checked once by the server, revoked, and never stored. The server keeps your GitHub id and login, your rooms and an iroh endpoint id per device; with sharing off it gets one total cell per hour for matches. Room members see your login and live output rate. When the host starts a match, every member plays, and members see how many output tokens or how much estimated cost (USD) you added during it. While you are in a room, anyone who knows your meter's endpoint id (current or past room members) gets your public IP and local addresses when they connect, whether or not a direct path opens; leaving a room or logging out changes the key. `tokenmeter league logout` unlinks this device; `tokenmeter account delete` deletes the account.
- A leftover self-hosted `leaderboard.endpoint` stays off until you set it.

## Token League

Token League shows friends' live output rate in shared rooms. `tokenmeter league login` signs in with GitHub, `tokenmeter league open` prints an invite link (`https://tokenmeter.online/j/<id>`), and a friend runs `tokenmeter league join <link>`. The overlay's league panel then lists each member's tok/s. Rates travel directly between members' meters over QUIC (iroh) and fall back to the TokenMeter relay when a direct path is blocked; the server keeps only the room list. A room holds 20 people and you can be in 8 rooms. `tokenmeter league` shows your rooms and invite link; `leave` and `close` end them. On macOS with the firewall on, you may be asked to allow incoming connections; Deny keeps live rates working through the relay. The host can start a match (`tokenmeter league match start --minutes 60 --rule output|cost`); the server scores how much each member's output or estimated cost grew during it and finalizes an hour after it ends. `tokenmeter league match` prints the standings, the overlay's league panel shows the time left, and a desktop notice shows the final podium.

## Update or remove

Automatic updates are off by default. Opt in to check once per day when the daemon starts, or update immediately:

```bash
tokenmeter update on
tokenmeter update now
```

Only stable GitHub Releases are installed, and only when both binaries match the release `SHA256SUMS`. The binaries are replaced in place. Turn it back off with `tokenmeter update off`. Running the install script again also updates.

Remove hooks before deleting the two binaries:

```bash
tokenmeter uninstall
rm ~/.local/bin/tokenmeter ~/.local/bin/tokenmeter-hook
```

Hooks + local state + league tokens:

```bash
tokenmeter uninstall --purge
```

## Development

```bash
git clone https://github.com/helloimdevman/tokenmeter.git
cd tokenmeter
cargo test --manifest-path native/Cargo.toml
cargo build --release --manifest-path native/Cargo.toml
./native/target/release/tokenmeter install --dry-run
```

## Contributing

New agent adapters, provider entries, bug reports, and fixes are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md), and ask questions in [Discussions](https://github.com/helloimdevman/tokenmeter/discussions). Report security issues privately as described in [SECURITY.md](SECURITY.md).

TokenMeter is created and maintained by imdevman and licensed under the [MIT License](LICENSE).
