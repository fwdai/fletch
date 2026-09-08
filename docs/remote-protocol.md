# Fletch remote protocol (mobile ↔ desktop)

Status: v2 contract (v1 plus the secure channel). Both the desktop `remote`
module (`src-tauri/src/remote/`) and the mobile client (`mobile/src/remote/`
and `mobile/src-tauri/src/remote/`) implement exactly this document.
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

- WebSocket carrying encrypted binary frames (see "Secure channel"); the
  plaintext inside each frame is one JSON document, UTF-8. One connection per
  device.
- Host listens on TCP port `47285` by default (setting `remote.port`), all
  interfaces, path `/ws`. `ws://<host>:<port>/ws`. The WebSocket is a plain
  carrier: the secure channel, not the transport, provides confidentiality and
  authentication, so the same frames travel unchanged through the relay (see
  "Relay"). The phone dials the LAN address first and falls back to the relay.
- Host sends a WebSocket ping every 20 s and closes the connection after two
  missed pongs. Client reconnects with exponential backoff (1 s, 2 s, 4 s … 30 s).
- WebSocket messages larger than 4 MiB are rejected (close code 1009).
- Everything the host holds for a connection is bounded, and ends with it. The
  outbound queue holds 64 frames: a client that stops reading while the host
  still has frames for it has its socket dropped (no close frame — the queue is
  what one would travel through), and reconnects. At most 8 requests may be in
  flight per connection; further requests are answered immediately with
  `{ ok: false, error: "too many in-flight requests" }` and never dispatched.
  Requests still running when the socket goes away are abandoned.

## Secure channel

Every connection starts with a Noise handshake and carries only encrypted
frames afterwards. The WebSocket, and any relay between the two ends, sees
ciphertext.

- Pattern `Noise_XX_25519_ChaChaPoly_BLAKE2s`, prologue the ASCII bytes
  `fletch-remote-v2`, empty handshake payloads. The phone is the initiator and
  the host the responder. The three handshake messages travel as three
  WebSocket **binary** messages: `-> e`, `<- e, ee, s, es`, `-> s, se`.
- Identities are static X25519 keys. The host generates its keypair on first
  use and keeps the 32-byte private key at `<app_data_dir>/remote/host_key`
  (mode 0600). The phone generates a device keypair on first use and keeps it
  in its own app data dir. Private keys never leave the device that made them.
  There are no tokens. Both key files follow one rule: only a *missing* file
  means a new identity. A file of the wrong length, or one that cannot be read,
  is an error the user sees (`remote_status.error` on the host, the connection
  error on the phone), never a silent regeneration, because regenerating would
  invalidate every pairing. Deleting the file is the deliberate way to do that.
  Writes are atomic (temp file, 0600, rename), so a crash mid-write cannot
  leave a partial key to be mistaken for corruption.
- The **host ID** is the host's public key, base64url without padding
  (43 characters). It is what the pairing link carries and what the relay will
  route on.
- After the handshake, every protocol frame — request, response or event — is
  one WebSocket binary message holding the JSON encrypted in chunks: repeated
  `u16 big-endian ciphertext length || ciphertext`, each chunk encrypting at
  most 65 519 plaintext bytes (Noise's 65 535-byte message cap minus the
  16-byte tag). The receiver decrypts the chunks in order and concatenates the
  plaintext. Empty plaintext is one chunk holding only the tag; a zero-byte
  frame is malformed. Ping/pong stay at the WebSocket level, unencrypted, and
  are tolerated during the handshake, which the host times out after 10 s.
- A handshake that fails, a text frame at any point, or a frame that does not
  decrypt (truncated header or body, a chunk shorter than a tag, a bad tag)
  closes the connection with `4001`. The secure channel failing is the same
  class of failure as never having established it.
- Host authentication: when the phone knows the host's public key (it came in
  the QR) it aborts the handshake if the responder's static key differs. The
  check happens on message 2, before message 3 is sent, so an impostor never
  learns the device's key. When the phone has no key (host address and code
  typed by hand) it pins the key it sees on first contact and refuses a
  different one on every later connection. A mismatch is not retried; the user
  has to re-pair.
- Device authentication is the handshake itself. The host learns the phone's
  static key from message 3 and either registers it (`pair`) or requires it to
  be registered already (`hello`). Revoking a device deletes its key from the
  host; there is nothing on the phone to invalidate.
- The secure channel lives in each app's Rust layer (`snow` on both sides).
  The mobile webview speaks plain JSON to its own Rust layer, which is also why
  the browser dev loop (`bun run dev` outside Tauri) can only reach the mock
  host.

## Relay

Off-network access goes through a relay that both ends dial outbound. The
relay is a dumb pipe: it routes by host ID and sees nothing but the ciphertext
the secure channel already produces. The reference implementation is a
Cloudflare Worker fronting one Durable Object per host ID (`relay/` in this
repo); anyone can run their own and point both apps at it.

- **Base URL.** Host setting `remote.relay_url`; absent or empty means no
  relay. The desktop's suggested default is `wss://relay.fletch.app`. The
  pairing link carries the URL as `relay=<url-encoded>` when set, and the phone
  persists it with the host.
- **Endpoints.** `wss://<relay>/v1/host/<hostId>` for the Mac,
  `wss://<relay>/v1/device/<hostId>` for a phone. `hostId` is the host public
  key, base64url. Anything else is `404`.
- **Host link authentication** proves possession of the host key with a DH
  challenge, as three JSON text frames within 10 s of the upgrade:
  1. relay → host `{ "type": "challenge", "nonce": "<base64url, 32 bytes>", "relayKey": "<base64url X25519 public key>" }`
  2. host → relay `{ "type": "proof", "proof": "<base64url SHA-256( shared || nonce || hostKey )>" }`
     where `shared = X25519(hostPrivate, relayKey)`, `nonce` and `hostKey` are
     the raw 32-byte values, and `||` is concatenation.
  3. relay → host `{ "type": "ready" }`, or close `4003` on a bad proof, a
     malformed or non-text proof frame, or no proof within the 10 s.
  The relay stores nothing: the host ID *is* the public key it verifies
  against. A second host link for the same ID replaces the first — closed with
  `4409`, its devices closed with `4404` — but only once the newcomer's proof
  has verified. An unauthenticated link is a claim, not a host: it changes
  nothing for the current host and its devices, whether it fails, times out,
  hangs up or is cut for breaking a limit; only the authenticated host's
  departure closes devices with `4404`. The host ID is public, so anything less
  would let anyone who knows it knock the real host or its devices offline. The deadline is enforced when the
  proof arrives as well as by the alarm. A known-
  answer vector for the proof lives in `relay/test-vector.json`; both the
  relay's and the host's implementations are tested against it.
- **Device link.** No relay-level authentication: the Noise handshake is the
  authentication, end to end. The phone speaks to the relay exactly as it
  would to the host on the LAN — same handshake, same frames, same close codes
  from the host. The relay closes a device link with `4404` ("host offline")
  when no host link is attached, `4429` when the host already has 8 device
  links, `1009` for a message over 4 MiB, and `1008` for more than 100
  messages in 10 s.
- **Multiplexing on the host link.** Every device link becomes a numbered
  virtual connection on the one host link. Binary frames on the host link are
  `type (1 byte) || connId (u32 big-endian) || payload`:

  | type | direction | payload |
  |---|---|---|
  | `0x01` OPEN | relay → host | empty; a device link attached |
  | `0x02` DATA | both | one device WebSocket **binary** message, verbatim |
  | `0x03` CLOSE | both | `code (u16 big-endian) || reason (UTF-8)`; the link is over |
  | `0x04` TEXT | relay → host | one device WebSocket **text** message, verbatim, so the host can apply its own `4001` rule |

  A host-sent CLOSE makes the relay close the device link with that code and
  reason. A device link that drops produces a CLOSE toward the host with
  `1006`, and a device link the relay closed itself (`1008`, `1009`) produces
  a CLOSE with that code, so the host always frees the virtual connection.
  Frames the relay does not understand (unknown connId, truncated, a text
  frame on the host link after `ready`) are ignored, not fatal. The host link
  accepts messages up to 4 MiB + 5 bytes, so a legal 4 MiB device message fits
  inside a DATA frame. The host serves each virtual connection through the
  same code path as a LAN socket; WebSocket ping/pong is per hop (host↔relay
  and relay↔device), never forwarded, and the host answers its own liveness
  pings to a virtual connection locally. The relay originates no pings of its
  own (they would keep the Durable Object awake); the host pings the relay,
  and the runtime answers device pings without waking the object.
- **Host side.** The Mac keeps the host link up whenever remote access is
  enabled and a relay URL is set, reconnecting with backoff (1 s … 60 s) when
  it drops, `4409` included: two Macs sharing one host key is a
  misconfiguration, and the alternating link surfaces it in `relay.error`
  rather than silently picking a winner. `remote_status.relay` reports the
  link (see host commands). The host hashes the nonce it is given whatever its
  length (the relay always sends 32 bytes), ignores frames for a connId it
  does not know, and closes a single virtual connection with `1008` if that
  device outruns the host's inbound queue for it. Disabling
  remote access drops the link, which closes every relayed device with `4404`
  from the relay's side; the host's own `4004` goes out first over the virtual
  connections, as on the LAN.
- **Phone side.** Connection candidates in order: `addr` over `ws://` with a
  3 s open timeout, then `wss://<relay>/v1/device/<hostId>` with no extra
  timeout (it is the last resort; the platform's TCP/TLS timeout applies). The
  relay candidate exists only when the phone holds both the relay URL and the
  host key, since the key is the route. A host-key mismatch on either path
  stops the list at once and is not retried: an impostor must not be able to
  steer the phone onto the other path. The relay URL arrives in the pairing
  link and can be added or changed later in the phone's host sheet without
  re-pairing. The Noise handshake and everything after it are identical on
  both paths, so the app above the transport cannot tell which one it is on
  and does not need to.

## Threat model (v2)

A passive observer of the network sees the WebSocket upgrade, ping/pong timing
and ciphertext sizes. No credential crosses the wire, so a capture cannot be
replayed and the device key is never exposed. An active attacker on the path
during a QR pairing cannot impersonate the host, because the phone already
holds the host's key. During a hand-typed pairing an active attacker on the
same LAN could impersonate the host for that one pairing (trust on first use);
that is the accepted residual risk of manual entry and the reason the QR is the
default path. A stolen phone holds its device key, which the desktop revokes in
Settings (revoke also closes the device's live connections with `4003`). A
stolen `host_key` lets an attacker impersonate the host to paired phones;
deleting the file regenerates the key on next launch, after which every phone
must pair again.

Still in place from v1: pairing needs a single-use code, minted on the desktop
and valid five minutes; turning remote access off closes every connection with
`4004`; ops are an explicit allowlist with no shell, no file writes and no raw
PTY, and events are a whitelist that excludes PTY output. A paired phone can
list directory names anywhere the desktop user can (`list_dir`), add any folder
as a project and clone into any folder — the same reach the desktop's own New
Project dialog has, and no more: it still cannot read files outside an agent's
checkout, and the only writes outside one are the `git init` of a pinned folder
and the clone itself, both into a folder the user chose.

The relay adds a party that sees metadata but no content: which host IDs are
online, when devices connect, and ciphertext sizes and timing. It cannot read
or forge frames, and it cannot impersonate a host, because attaching a host
link requires the host's private key. Anyone who learns a host ID can open
device links to that host and make it run Noise handshakes that fail, which is
why device links per host are capped and rate-limited; a host ID is a random
public key, so it cannot be guessed or enumerated. A hostile relay operator
can deny service and nothing more.

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

The first frame after the handshake MUST be `pair` or `hello`. Any other first
frame closes the connection with code `4001`. Any request before a successful
`pair` or `hello` closes with `4003`. A `hello` from a device key the host does
not have on record (never paired, or revoked) closes with `4003`.

### `pair`

Desktop Settings → "Mobile devices" → "Pair a device" calls the Tauri command
`remote_begin_pairing`, which mints a one-time pairing code (8 chars from
`A-Z2-9`, no ambiguous glyphs), valid 5 minutes, single use. Settings shows it
as text and as a QR code encoding:

```
fletch://pair?host=<host public key, base64url>&addr=<ip>:<port>&relay=<url-encoded relay base URL>&token=<code>&name=<url-encoded host name>
```

`host` is the host ID (its public key) and is the phone's authentication of
the Mac. `addr` is the best LAN address to dial right now; `relay` is present
only when the host has a relay configured (see "Relay"). `host` stays the
identity whichever path is used. A hand-typed pairing supplies only `addr` and
`token`, and pins the host key it meets (see "Secure channel"); it cannot use
the relay until a later QR pairing or manual entry supplies the relay URL.

Client request (first encrypted frame after the handshake):

```json
{ "id": "…", "op": "pair", "args": { "token": "K7PQ2M9X", "device": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" } } }
```

Result:

```json
{ "deviceId": "uuid", "host": { "name": "Alex's MacBook Pro", "appVersion": "0.7.23", "os": "macos" } }
```

The frame carries no credential: the device's identity is the static key the
handshake delivered. The host persists
`{ deviceId, name, platform, publicKey (base64url), createdAt, lastSeenAt }`
in `<app_data_dir>/remote/devices.json`. Records from the token era (with a
`tokenHash` and no `publicKey`) are dropped at load. After `pair` the
connection is authenticated as if `hello` had succeeded and the host starts
forwarding events. The host does NOT push a snapshot; the client issues
`get_workspace` itself right after a successful `pair`.

### `hello`

```json
{ "id": "…", "op": "hello", "args": { "client": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" } } }
```

Result:

```json
{ "host": { "name": "…", "appVersion": "…", "os": "macos" }, "workspace": <Workspace | null> }
```

The host looks up the handshake's remote static key in `devices.json`; a key
it does not know closes with `4003`. `workspace` is the exact `get_workspace`
result. After the response the host starts forwarding events for this
connection.

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
| `list_dir` | `{ path }` (tilde-expanded on the host) | `DirListing` |
| `add_workspace_repo` | `{ repoPath }` | `Workspace` |
| `clone_repo` | `{ spec, destParent }` | `Workspace` |
| `gh_status` | `{}` | `GhStatus` |
| `gh_repo_list` | `{}` | `GhRepoSummary[]` |

Never exposed, by design: the generic `db_*` table bridge, every file mutation
(`write_checkout_file`, `rename_*`, `delete_*`, `create_*`, `copy_*`), shell
ops (`open_agent_shell`, `write_to_shell`, …), `write_to_agent` (raw PTY),
editor/log/telemetry/provider-install ops, workflow, roadmap and run ops.
Adding an op means adding a row here and a match arm in the dispatcher.

The spawn flow is the desktop's: `allocate_draft_name` → `spawn_agent` →
wait for `agent:status` to leave `spawning` → `send_user_message` with the
prompt as the first turn (`turnId` = client UUID). The phone does not send
the prompt through `instructions`.

Adding a project from the phone reuses the desktop's commands unchanged. Two
flows: **open an existing folder** on the Mac (`list_dir` to browse, then
`add_workspace_repo`) and **clone from GitHub** (`gh_status` to know whether
`gh` is signed in, `gh_repo_list` to pick one of the user's repos or a typed
`owner/repo` / URL, `list_dir` to pick the destination parent, then
`clone_repo`). The result of either is the new `Workspace`, which the caller
applies itself; neither command emits `workspace:changed` (the desktop frontend
uses the returned value too), so other connected clients see the project on
their next `get_workspace`. Note `DirListing.entries[].is_dir` is snake_case:
`DirEntry` is serialized as-is, the one non-camelCase payload on the allowlist.
Pinning a folder that is not yet a git repository runs `git init` plus an
initial commit in it, exactly as the desktop dialog does. The phone remembers
the last destination parent per host, as the desktop does. Creating a brand-new
repo from the phone (`create_repo`) is a follow-up.

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

Host errors are strings (the `Display` of the Rust `Error`). Two are reserved:
`"unknown op"` for anything off the allowlist and `"too many in-flight
requests"` for a connection over its concurrency cap.

Auth failures are WebSocket close codes, not error responses: `4001` failed
handshake, text frame, or bad first frame; `4003` unauthenticated, unknown
device key or revoked; `4004` host has remote access disabled. `4003` also arrives unprompted when the host revokes the device this
connection is authenticated as, and `4004` when the host turns remote access
off — in both cases the credential is gone or dormant, so the client should
stop reconnecting until it is paired or the host is enabled again.

Relay close codes reach the phone on the device link and are all retryable
with the normal backoff — the condition is on the host's or relay's side and
may clear: `4404` the Mac is not connected to the relay ("Your Mac is
offline"), `4429` the Mac already has its 8 relayed devices, `1008` the relay
throttled this connection, `1009` a message was over 4 MiB. `4409` is sent
only on the host link (a newer host link replaced this one) and never reaches
a phone.

## Host-side settings and commands (desktop Tauri commands, not remote ops)

| command | purpose |
|---|---|
| `remote_status` | `{ enabled, listening, port, hostId, addresses: string[], devices: RemoteDevice[], relay: RelayStatus, error: string \| null }` — `hostId` is the host public key, base64url |
| `remote_set_enabled` | `{ enabled }` start/stop the listener and the relay link; persists setting `remote.enabled`; disabling closes live connections with `4004` |
| `remote_set_relay` | `{ url: string \| null }` persist setting `remote.relay_url` (null/empty clears it) and connect or drop the host link accordingly; returns `RemoteStatus` |
| `remote_begin_pairing` | `{ token, url, expiresAt }`; refused while the listener is down or `error` is set |
| `remote_revoke_device` | `{ deviceId }`; drops the credential and closes that device's live connections with `4003` |

`RemoteDevice = { deviceId, name, platform, createdAt, lastSeenAt, connected }`,
where `connected` is derived from the live connections, not from `lastSeenAt`.
`RelayStatus = { url: string | null, state: "off" | "connecting" | "connected" | "error", error: string | null }`;
`off` when no URL is set or remote access is disabled, `error` with the last
failure while the link is between reconnect attempts.
`error` is a standing problem with the remote surface itself — "`devices.json`
could not be read or written", or "`host_key` could not be created or read";
either blocks pairing, and without a host key no connection can be served
(`hostId` is empty) — and the Settings pane shows it inline. A failed write is sticky: the in-memory list
stays authoritative (a revoked device is revoked, its sockets are closed), the
error stays in `error`, and every `remote_status` retries the write until it
lands, so the stale file cannot quietly bring a revoked device back at the next
launch once the disk recovers.

## Out of scope (tracked, not built)

Push notifications (the host sends the relay a content-free wake hint, the
relay calls APNs, the phone connects and fetches), QR scanning on the phone
(manual entry of address and code, plus `fletch://pair` deep-link parsing, for
now), creating a brand-new repo from the phone (`create_repo`), voice,
attachments, Run scripts, Keychain storage of the device key on the phone (it
lives in the app data dir).
