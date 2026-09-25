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

Errors are `error.schema.json` (`{error, message}`) with codes `invalid` (400), `unauthorized` (401),
`rate_limited` (429, with `Retry-After`), `upgrade_required` (426) and `internal` (500). The client sends
`User-Agent: tokenmeter/<version>`; the server answers 426 to versions it no longer accepts.

## Limits

The server rejects a whole upload with 400 `invalid` when any part breaks these. The client runs
the same check on each hour before sending and drops the hours that fail.

- At most 48 hours per upload, 200 cells per hour, and a 256 KB body (the client stops near 200 KB).
- `t` is a multiple of 900 within [now − 30 days, now + 1 hour], and appears once per upload.
- Counts and `cost_usd` are between 0 and 10^13.
- Labels: client `[a-z0-9-]{1,32}`, route `[a-z0-9.-]{1,64}`, plan `[a-z0-9_-]{1,32}`,
  model `[A-Za-z0-9._:@+-]{1,96}`, each combination once per hour. With `share: false` every
  cell is a total cell with `*` in all four.
- Request limits: device registration 10 per hour per IP; uploads 2 per minute per device with a burst of 5.

## What is sent

| Setting | Sent |
|---|---|
| sharing off | nothing |
| sharing on | device token, platform and version, and per local hour: tool, route label, plan, model family, token counts, request count, estimated cost, timing sums |
| never | prompts, code, file paths, project names, session ids, private endpoint hostnames, custom service or model names |

- Hours that ended before you turned sharing on are never sent; the hour you turn it on in is sent
  whole. Turning it off and on again starts over from that hour.
- Route labels are public API hosts (`api.anthropic.com`), `bedrock`, `vertex`, `azure-openai`,
  `self-hosted` for every other address, or `unknown`.
- Model names are built-in price families (`claude-opus-5`); anything else is `other`.
- `t` is the Unix second at the start of your local hour (a multiple of 900).
- An upload replaces whole hours, so sending the same body twice changes nothing.
- `tokenmeter share preview` prints the next upload. `tokenmeter account delete` deletes this device's data on the server.
