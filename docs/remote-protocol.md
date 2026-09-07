# Fletch remote protocol (mobile ↔ desktop)

Status: v1 contract. Both the desktop `remote` module (`src-tauri/src/remote/`)
and the mobile client (`mobile/src/remote/`) implement exactly this document.
Deviations are made here first, then in code.

## Concept

The phone is a control panel. Code and agents live on the Mac. The desktop app
("host") runs a small WebSocket server; a paired phone connects, receives the
workspace snapshot, then a live event stream, and sends operations. Every op
mirrors an existing Tauri command by name, argument keys and result DTO, so
the host dispatcher calls the same `Supervisor` functions the commands do and
the phone reuses the desktop's TypeScript DTOs (`src/api/types/*`) and chat
adapters (`src/adapters/*`) unchanged.

## Transport

- WebSocket, text frames, JSON, UTF-8. One connection per device.
- Host listens on TCP port `47285` by default (setting `remote.port`), all
  interfaces, path `/ws`. `ws://<host>:<port>/ws`. No TLS in v1; intended for
  LAN and Tailscale. A relay and TLS are out of scope for v1.
- Host sends a WebSocket ping every 20 s and closes the connection after two
  missed pongs. Client reconnects with exponential backoff (1 s, 2 s, 4 s … 30 s).
- Frames larger than 4 MiB are rejected (close code 1009).

## Envelope

Client → host request:

```json
{ "id": "3f9c…", "op": "send_user_message", "args": { "agentId": "arabia", "turnId": "…", "text": "…", "attachments": [] } }
```

Host → client response (exactly one per request, any order):

```json
{ "id": "3f9c…", "ok": true,  "result": { … } }
{ "id": "3f9c…", "ok": false, "error": "human-readable message" }
```

Host → client event (no id, never acknowledged):

```json
{ "event": "agent:status", "payload": { … } }
```

`id` is a client-chosen unique string (UUID v4). `args` is always an object,
possibly empty. `result` is the command's return value serialized exactly as
Tauri would serialize it for `invoke` (snake_case DTO fields, camelCase arg
keys, `null` for `Option::None`, `null` result for `()`).

## Authentication and pairing

The first frame on a connection MUST be `pair` or `hello`. Any other first
frame closes the connection with code `4001`. Any request before a successful
`hello` closes with `4003`. A bad or revoked device token closes with `4003`.

### `pair`

Desktop Settings → "Mobile devices" → "Pair a device" calls the Tauri command
`remote_begin_pairing`, which mints a one-time pairing token (8 chars from
`A-Z2-9`, no ambiguous glyphs), valid 5 minutes, single use. Settings shows it
as text and as a QR code encoding:

```
fletch://pair?host=<ip-or-hostname>&port=<port>&token=<token>&name=<url-encoded host name>
```

Client request:

```json
{ "id": "…", "op": "pair", "args": { "token": "K7PQ2M9X", "device": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" } } }
```

Result:

```json
{ "deviceId": "uuid", "deviceToken": "43-byte base64url secret", "host": { "name": "Alex's MacBook Pro", "appVersion": "0.7.23", "os": "macos" } }
```

The host persists `{ deviceId, name, platform, tokenHash (sha256), createdAt, lastSeenAt }`
in `<app_data_dir>/remote/devices.json`. Plain tokens are never stored on the host.
After `pair` the connection is authenticated as if `hello` had succeeded and
the host starts forwarding events. The host does NOT push a snapshot; the
client issues `get_workspace` itself right after a successful `pair`.

### `hello`

```json
{ "id": "…", "op": "hello", "args": { "deviceToken": "…", "client": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" } } }
```

Result:

```json
{ "host": { "name": "…", "appVersion": "…", "os": "macos" }, "workspace": <Workspace | null> }
```

`workspace` is the exact `get_workspace` result. After the response the host
starts forwarding events for this connection.

## Operations (v1 allowlist)

Op name, argument keys and result type are identical to the Tauri command of
the same name (see `src/api/domains/*.ts` for the argument keys and
`src/api/types/*.ts` for the DTOs). The host dispatches through an explicit
allowlist; any op not listed returns `{ ok: false, error: "unknown op" }`.

| op | args | result |
|---|---|---|
| `get_workspace` | `{}` | `Workspace \| null` |
| `allocate_draft_name` | `{ drafts: string[] }` | `string` |
| `spawn_agent` | as command; host forces `view: "custom"`, ignores `purpose`, `skills`, `mcpServers`, `customAgentId` in v1 | `AgentRecord` |
| `send_user_message` | `{ agentId, turnId, text, attachments: [] }` | `boolean` |
| `answer_tool_use` | `{ agentId, requestId, updatedInput, behavior, message? }` | `null` |
| `stop_agent` | `{ agentId }` | `null` |
| `resume_agent` | `{ agentId }` | `null` |
| `archive_agent` | `{ agentId }` | `null` |
| `set_agent_model` | `{ agentId, model }` | `null` |
| `set_agent_effort` | `{ agentId, effort }` | `null` |
| `read_session_records` | `{ agentId }` | `SessionRecord[]` |
| `read_user_turns` | `{ agentId }` | `UserTurn[]` |
| `get_git_state` | `{ agentId }` | `GitState \| null` |
| `get_agent_diff_stats` | `{ agentId }` | `DiffStats` |
| `list_checkout_tree` | as command | `CheckoutFile[]` |
| `read_checkout_file` | `{ agentId, path, baseMode? }` | `CheckoutFileContents` |
| `get_file_diff` | as command | `string` |
| `commit_agent` | as command | `null` |
| `push_agent` | as command | `string` |
| `create_pr` | as command | `PrState` |
| `get_pr_state` | as command | `PrState \| null` |
| `get_pr_checks` | as command | `PrChecks \| null` |
| `get_pr_live` | as command | `PrLive \| null` |
| `list_repo_branches` | `{ repoPath }` | `string[]` |
| `repo_default_branch` | `{ repoPath }` | `string` |
| `discover_supported_models` | as command | `AgentModels[]` |

Never exposed, by design: the generic `db_*` table bridge, every file mutation
(`write_checkout_file`, `rename_*`, `delete_*`, `create_*`, `copy_*`), shell
ops (`open_agent_shell`, `write_to_shell`, …), `write_to_agent` (raw PTY),
editor/log/telemetry/provider-install ops, workflow, roadmap and run ops.
Adding an op means adding a row here and a match arm in the dispatcher.

The spawn flow is the desktop's: `allocate_draft_name` → `spawn_agent` →
wait for `agent:status` to leave `spawning` → `send_user_message` with the
prompt as the first turn (`turnId` = client UUID). The phone does not send
the prompt through `instructions`.

## Events (v1 whitelist)

Forwarded verbatim with the desktop event name and payload (see
`src/api/events.ts` for payload types). The host taps the Tauri event bus once
with `Listener::listen_any` and forwards only these names to every
authenticated connection:

```
agent:event            agent:status           agent:task
agent:branch           agent:model            agent:effort
agent:repo_added       agent:git-action       session:records-appended
turn:started           workspace:changed      pr:state_changed
verify:report          publish:approval-requested
```

`agent:event` is forwarded unfiltered, including the provider's
`control_request` records: that is the only way a held tool-use approval
reaches the phone, and `answer_tool_use` needs the `request_id` it carries.
On the `error` status transition, `agent:status` must carry the real
`last_error`, since both clients keep the previous error when it is null.

Never forwarded: `agent:output`, `shell:output` (raw PTY bytes), `run:*`,
`wf:*`, `roadmap:*`, `dictation:*`, `docker:*`, `agent-install:*`.

Delivery is best effort, exactly like the desktop frontend: the phone must
refetch `get_workspace` on reconnect and on returning to the foreground, and
`read_session_records` when it opens an agent.

## Errors

Host errors are strings (the `Display` of the Rust `Error`). Auth failures are
WebSocket close codes, not error responses: `4001` bad first frame, `4003`
unauthenticated or revoked, `4004` host has remote access disabled.

## Host-side settings and commands (desktop Tauri commands, not remote ops)

| command | purpose |
|---|---|
| `remote_status` | `{ enabled, listening, port, addresses: string[], devices: RemoteDevice[] }` |
| `remote_set_enabled` | `{ enabled }` start/stop the listener; persists setting `remote.enabled` |
| `remote_begin_pairing` | `{ token, url, expiresAt }` |
| `remote_revoke_device` | `{ deviceId }` |

`RemoteDevice = { deviceId, name, platform, createdAt, lastSeenAt, connected }`.

## Out of scope for v1 (tracked, not built)

TLS or a relay for off-network access, push notifications (needs a relay and
APNs), QR scanning on the phone (manual entry of host, port and token in v1;
`fletch://pair` deep link parsing is fine to include), Add project / clone,
voice, attachments, Run scripts, Keychain storage of the device token on the
phone (v1 stores it in the app data dir).
