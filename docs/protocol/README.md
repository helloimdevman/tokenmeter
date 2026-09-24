# TokenMeter ↔ Token League server protocol

The client sends usage only when you turn sharing on (`tokenmeter share on`).
The server code is private; this folder is the public contract. The JSON Schemas here are
generated from `native/protocol` and a test fails when they drift.

## Endpoints (v1)

| Method and path | Auth | Body → reply |
|---|---|---|
| `GET /v1/config` | none | → `config.schema.json` |
| `POST /v1/devices` | none | `device-request` → 201 `device-response` |
| `PUT /v1/usage` | `Bearer <device token>` | `usage-upload` → 204 |
| `DELETE /v1/account` | `Bearer <device token>` | → 204 |

Errors are `error.schema.json` (`{error, message}`) with codes `invalid`, `unauthorized`,
`rate_limited` (with `Retry-After`) and `upgrade_required` (426). The client sends
`User-Agent: tokenmeter/<version>`; the server answers 426 to versions it no longer accepts.

## What is sent

| Setting | Sent |
|---|---|
| sharing off | nothing |
| sharing on | device token, platform and version, and per local hour: tool, route label, plan, model family, token counts, request count, estimated cost, timing sums |
| never | prompts, code, file paths, project names, session ids, private endpoint hostnames, custom service or model names |

- Route labels are public API hosts (`api.anthropic.com`), `bedrock`, `vertex`, `azure-openai`,
  `self-hosted` for every other address, or `unknown`.
- Model names are built-in price families (`claude-opus-5`); anything else is `other`.
- `t` is the Unix second at the start of your local hour (a multiple of 900).
- An upload replaces whole hours, so sending the same body twice changes nothing.
- `tokenmeter share preview` prints the next upload. `tokenmeter account delete` deletes this device's data on the server.
