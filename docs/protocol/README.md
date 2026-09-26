# TokenMeter ↔ Token League server protocol

The client talks to the server only when you turn sharing on (`tokenmeter share on`) or log in to
Token League (`tokenmeter league login`). The server code is private; this folder is the public
contract. The JSON Schemas here are generated from `native/protocol` and a test fails when they drift.

## Endpoints (v1)

| Method and path | Auth | Body → reply |
|---|---|---|
| `GET /v1/config` | none | → `config.schema.json` |
| `POST /v1/devices` | none | `device-request` → 201 `device-response` |
| `PUT /v1/usage` | `Bearer <device token>` | `usage-upload` → 204 |
| `DELETE /v1/account` | `Bearer <device token>` | → 204 |
| `POST /v1/auth/github` | `Bearer <device token>` | `auth-request` → `auth-response` |
| `POST /v1/auth/logout` | `Bearer <device token>` | → 204 |
| `GET /v1/rooms` | logged-in device | → `rooms` |
| `POST /v1/rooms` | logged-in device | → 201 `room` |
| `POST /v1/rooms/{id}/join` | logged-in device | → `room` |
| `POST /v1/rooms/{id}/leave` | logged-in device | → 204 |
| `DELETE /v1/rooms/{id}` | the room's host | → 204 |

Errors are `error.schema.json` (`{error, message}`) with codes `invalid` (400), `unauthorized` (401),
`forbidden` (403), `not_found` (404), `room_full` and `too_many_rooms` (409), `rate_limited` (429, with
`Retry-After`), `upgrade_required` (426), `internal` (500) and `upstream` (502, GitHub did not answer).
The client sends `User-Agent: tokenmeter/<version>`; the server answers 426 to versions it no longer
accepts or cannot read.

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
| Token League login | GitHub id and login, the rooms you are in, and an iroh endpoint id per device. With sharing off, one total cell (`*` labels) per hour from the hour you log in, used for matches |
| never | prompts, code, file paths, project names, session ids, private endpoint hostnames, custom service or model names |

- Hours that ended before you turned sharing on are never sent; the hour you turn it on in is sent
  whole. Turning it off and on again starts over from that hour.
- Route labels are public API hosts (`api.anthropic.com`), `bedrock`, `vertex`, `azure-openai`,
  `self-hosted` for every other address, or `unknown`.
- Model names are built-in price families (`claude-opus-5`); anything else is `other`.
- `t` is the Unix second at the start of your local hour (a multiple of 900).
- An upload replaces whole hours, so sending the same body twice changes nothing.
- `tokenmeter share preview` prints the next upload. `tokenmeter account delete` deletes this device's data on the server.

## Login

`tokenmeter league login` runs the GitHub device flow for the TokenMeter OAuth app (no scopes). The
client sends the GitHub token once to `POST /v1/auth/github`; the server checks it with GitHub,
revokes it and never stores it. The client never writes it to disk.

## Live rates (peer to peer)

Room members' meters connect to each other with [iroh](https://github.com/n0-computer/iroh) QUIC,
ALPN `tokenleague/live/1`, directly when the network allows it and through
`relay.tokenmeter.online` otherwise. The relay admits only endpoint ids of logged-in devices. Each
side sends on a connection it opened: one unidirectional stream of JSON lines
(`live-line.schema.json`, at most 1 KB each, e.g. `{"tps": 123.4}`). Lines carry no names: the
receiver matches the connection's authenticated endpoint id to the room's member list and stamps the
time itself.

While a meter is in a room, anyone who knows its endpoint id (current or past members of its rooms)
receives its public IP address and local addresses when connecting, whether or not a direct path
opens: iroh sends address candidates as soon as a connection is set up, before the member check.
Leaving a room or logging out makes a new key. Only a relay-only mode (planned with public rooms)
would hide addresses. On macOS with the firewall on, you may be asked to allow incoming connections;
Deny keeps live rates working through the relay.
