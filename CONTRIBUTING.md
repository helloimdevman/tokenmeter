# Contributing to TokenMeter

Thanks for helping. TokenMeter is a small Rust project, so most changes are quick to review. Issues and pull requests in English or Korean are both welcome.

## Ways to help

- **Add or fix an agent adapter.** Most agents are a YAML block in `native/meter/services.yaml`. Follow [docs/add-service.md](docs/add-service.md), add a sample log under `tests/fixtures/<agent>/`, and add a test that parses it.
- **Report a provider or route** (an API base URL, gateway, or local model server) with the "Provider or route" issue form.
- **Fix bugs**, starting with issues labeled `good first issue`.
- **Answer questions** in [Discussions](https://github.com/helloimdevman/tokenmeter/discussions).

## Development

You need Rust stable on macOS or Linux. On Linux, install the overlay's system libraries first:

```bash
sudo apt-get install -y libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev pkg-config
```

Then:

```bash
cargo test --manifest-path native/Cargo.toml
cargo build --release --manifest-path native/Cargo.toml
./native/target/release/tokenmeter install --dry-run
```

The tests never touch your real configuration. They point `HOME` and the state and config directories at temporary folders. Do the same when you try `install` or `uninstall` by hand.

## Layout

- `native/meter`: the `tokenmeter` binary (daemon, log watcher, engine, overlay, CLI)
- `native/hook`: `tokenmeter-hook`, the fast hook every agent calls
- `native/meter/services.yaml`: agent adapters and default settings
- `tests/fixtures`: sample agent logs used by the tests
- `install.sh`: the release installer

## Rules that keep users safe

- Never store or send prompts, responses, tool commands, file names, or full paths. Public output (`status --json`, `doctor --json`, league rows) stays on an allowlist, and the privacy tests fail if something leaks.
- Hooks must exit 0 and must never block an agent.
- Don't add network calls outside the install, update, quota, and league paths.

## Pull requests

- Keep each pull request to one change, and include tests.
- Write commit messages as `type: summary`, where type is `feat`, `fix`, `test`, `docs`, `ci`, `refactor`, or `chore`.
- Attach a screenshot for overlay changes.
- By contributing, you agree that your work is released under the [MIT License](LICENSE).
