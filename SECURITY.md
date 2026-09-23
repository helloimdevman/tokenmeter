# Security Policy

TokenMeter runs on your machine next to your coding agents. It edits their hook settings, reads their local logs, and can read locally stored CLI credentials for the optional quota view. Reports about any of that are taken seriously.

## Supported versions

Only the latest release receives security fixes.

## Reporting a vulnerability

Report privately through GitHub: **Security → Report a vulnerability** on this repository, or go straight to <https://github.com/helloimdevman/tokenmeter/security/advisories/new>. Please do not open a public issue.

Include your TokenMeter version (`tokenmeter --version`), your OS, and steps to reproduce. Do not include prompts, session logs, credentials, or `league-auth.json`. The output of `tokenmeter doctor --json` is safe to share.

We aim to acknowledge reports within 7 days. Once a fix ships, reporters are credited in the advisory unless they prefer otherwise.

## Scope

In scope:

- Hook installation and removal in agent settings (`~/.claude/settings.json`, `~/.codex/hooks.json`, `~/.cursor/hooks.json`, the generated OpenCode plugin, `~/.grok/hooks`)
- Anything that could expose prompts, file names, full paths, session ids, or credentials, locally or over the network
- The quota view's use of stored Claude, Codex, and Grok credentials
- `install.sh` and `tokenmeter update`, including checksum verification
- Token League network traffic

Out of scope: vulnerabilities in the agents themselves (report those to their vendors), and attacks that require someone who already controls your user account.
