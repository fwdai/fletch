# Fletch remote protocol (mobile ↔ desktop)

Status: v2 contract (v1 plus the secure channel). Both the desktop `remote`
module (`src-tauri/src/remote/`) and the client — the shared TypeScript
protocol client (`src/remote/`, used by the mobile app through `@desktop/*`)
plus the mobile Rust transport (`mobile/src-tauri/src/remote/`) — implement
exactly this document. Deviations are made here first, then in code.

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
  missed pongs. The client pings too — every 10 s, closing as abnormal (`1006`,
  reason `pong timeout`) after two misses — because a socket the OS froze under
  a suspended app carries no close frame back and reads on it stay pending, so
  without its own pings the client would learn nothing until TCP gave up. The
  phone additionally probes on returning to the foreground: its workspace
  refresh doubles as a liveness check, and one unanswered for 6 s forces a
  reconnect. Client reconnects with exponential backoff (1 s, 2 s, 4 s … 30 s).
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
- The secure channel lives in each app's Rust layer, and is one shared crate —
  `crates/fletch-proto` (`snow`) — so both ends cannot drift apart.
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
  relay. The desktop's suggested default is `wss://relay.fletch.sh`. The
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
  | `0x05` NOTIFY | host → relay | `connId` 0; UTF-8 JSON push request, see "Push notifications" |

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
  3 s open timeout, then `wss://<relay>/v1/device/<hostId>` with a 15 s one.
  A dial races the host's IPv6 and IPv4 addresses, interleaved by family and
  started 300 ms apart (RFC 8305), so a cellular network whose IPv6 path to
  the relay blackholes cannot spend the whole budget before IPv4 is tried.
  Both budgets cover the dial and the Noise handshake together; the first
  frame after the handshake (`pair` or `hello`, and the snapshot request that
  follows a `pair`) has its own 15 s bound, so a relay that accepted the
  socket for a Mac that has silently gone away turns into an error rather than
  an attempt that never ends. The relay candidate exists only when the phone
  holds both the relay URL and the host key, since the key is the route. A host-key mismatch on either path
  stops the list at once and is not retried: an impostor must not be able to
  steer the phone onto the other path. The relay URL arrives in the pairing
  link and can be added or changed later in the phone's host sheet without
  re-pairing. The Noise handshake and everything after it are identical on
  both paths, so the app above the transport cannot tell which one it is on
  and does not need to.

## Push notifications

The phone cannot keep a socket open in the background, so the two out-of-app
signals the desktop already raises — a turn finishing, an agent waiting on a
tool-use approval — reach it as APNs alerts. Content stays minimal: a fixed
title and the agent's name. Transcript text never leaves the Mac.

- **Registration.** After every successful `pair` or `hello`, and whenever iOS
  hands it a new token, the phone sends `register_push` with
  `{ token: "<APNs device token, lowercase hex>", environment: "sandbox" | "production" }`;
  `{ token: null }` alone clears it (the user turned notifications off), and
  `environment` is only read, and required, when a token is present. The host stores
  `pushToken` and `pushEnvironment` on the device record and never displays
  them. A token is a routing handle, not a credential: with it and the relay's
  APNs key one can send this phone a Fletch-branded alert, nothing more.
- **Triggers (host).** `turn_complete`: an agent's status goes
  `running → idle` and the user did not stop or interrupt it. Native-view
  agents are excluded: their status is read off terminal quiet and can flap
  several times in one turn, and the desktop does not notify for them either
  (phones only spawn the structured view anyway). `needs_input`:
  the first held `control_request` (`can_use_tool`) for an agent while none is
  pending for it — one alert per batch of parallel prompts, cleared when the
  turn ends. Both mirror `signalAway` in `src/store/eventListeners.ts`. The
  host skips a trigger while its own main window has focus (the user is at the
  Mac); otherwise it sends to every device with a token, and iOS itself hides
  the banner when the app is in the foreground. Title is `Turn complete` or
  `Needs your input`; body is the agent's name. No settings in v1.
- **NOTIFY frame (host → relay).** Mux type `0x05`, `connId` 0, payload UTF-8 JSON:

  ```json
  { "tokens": [{ "token": "<hex>", "environment": "sandbox" }], "title": "Turn complete", "body": "Fix login crash", "kind": "turn_complete", "agentId": "…", "collapseId": "<agentId>" }
  ```

  One to 8 tokens; `title`, `body`, `kind` and `agentId` at most 200
  characters each (Apple caps the whole payload at 4 KB). `collapseId` is
  optional and at most 64 bytes, Apple's limit for `apns-collapse-id`; a
  longer one is dropped and the alert still goes out uncoalesced. The relay
  ignores `connId` on this frame. Fire and forget: the relay sends no result
  frame. A relay without APNs configured, or one older than this frame type,
  ignores it; an invalid payload is ignored too. Before `ready` any binary
  frame is a failed proof (`4003`), NOTIFY included.
- **Relay → APNs.** The relay signs an ES256 provider JWT from `APNS_TEAM_ID`,
  `APNS_KEY_ID` and `APNS_PRIVATE_KEY` (the `.p8` PEM), cached and refreshed
  every 50 minutes (Apple wants 20–60), and POSTs to
  `https://api.push.apple.com/3/device/<token>` (`api.sandbox.push.apple.com`
  for `sandbox`) with `apns-topic: <APNS_BUNDLE_ID>`, `apns-push-type: alert`,
  `apns-priority: 10`, `apns-collapse-id: <collapseId>` and
  `apns-expiration` one hour out. Body:

  ```json
  { "aps": { "alert": { "title": "…", "body": "…" }, "sound": "default", "thread-id": "<agentId>" }, "fletch": { "hostId": "<hostId>", "agentId": "…", "kind": "turn_complete" } }
  ```

  Per host at most 30 NOTIFY frames per minute; excess is dropped, never fatal
  to the host link. Apple errors are logged with status and `reason` and
  otherwise dropped in v1 (feeding `410 Unregistered` back to the host is a
  follow-up). The relay persists nothing about tokens.
- **Phone.** Notification permission is requested after the first successful
  pairing, not on launch; the answer is remembered per host and a refusal is
  never asked again. Tapping an alert opens the app on that agent when
  `fletch.hostId` equals the paired host's key (the host ID *is* the host public
  key, the value the phone stores as `hostKey`), otherwise Home; the plugin
  delivers the payload to the webview as event `push://opened` and the token as
  `push://token`, holding both until the app's listeners are attached so a
  cold-start tap is not lost. The token comes from
  `didRegisterForRemoteNotificationsWithDeviceToken`, added at runtime to the
  generated app delegate by a small in-repo Tauri iOS plugin
  (`mobile/src-tauri/plugins/push`), which also inserts `aps-environment` into
  the entitlements at build time and reads the signed profile's value to pick
  `sandbox` or `production`.

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

Push notifications add Apple as a party and hand the relay a little content: a
fixed title, the agent's name and its ID, nothing from the transcript. A token
is stored on the host with its device record, travels to the relay only inside
a NOTIFY frame on the authenticated host link, and is never persisted there;
only the relay's APNs key turns it into an alert. A hostile relay operator
could send paired phones misleading alerts, which stays within the
deny-or-annoy ceiling: tapping one only opens the app, which then talks to the
real host over the secure channel.

Dictation puts the user's voice on the wire, which nothing did before. It
travels inside the same Noise channel as every other frame, so the relay sees
ciphertext sizes and timing (someone is talking, roughly how long) and nothing
else; the Mac holds the audio in memory only until `dictation_end` or the idle
sweep and never writes it out; and the transcript comes back over the same
channel. Apple is not a party: the local engine never calls a speech service.
A paired phone can make the Mac spend CPU on transcription, bounded by the
session cap and the capture cap.

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

Desktop Settings → "Remote control" → "Pair a device" calls the Tauri command
`remote_begin_pairing`, which mints a one-time pairing code (8 chars from
`A-Z2-9`, no ambiguous glyphs), valid 5 minutes, single use. The code carries
the scope preset the pairing will grant (see "Scopes"); the link does not, so
nothing a client says can widen it. Settings shows the code as text and as a QR
code encoding:

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
{ "deviceId": "uuid", "host": { "name": "Alex's MacBook Pro", "appVersion": "0.7.23", "os": "macos" }, "protocol": { "version": 2, "ops": [ … ], "events": [ … ], "features": [] } }
```

`protocol` is what this host answers *this device* — `ops` is already narrowed
to the device's scopes, so a client needs no new gate (see "Scopes" and
"Compatibility"). It is on `pair` as well as `hello` because a client that has
only ever paired must know the surface without a second round trip.

The frame carries no credential: the device's identity is the static key the
handshake delivered. The host persists
`{ deviceId, name, platform, publicKey (base64url), createdAt, lastSeenAt, pushToken?, pushEnvironment?, scopes }`
in `<app_data_dir>/remote/devices.json`. Records from the token era (with a
`tokenHash` and no `publicKey`) are dropped at load; a record with no `scopes`
is read as every scope (see "Scopes"). Pairing again on a key already on file
updates that one record, and the new code's scopes replace the old ones — which
is how a device's access is changed. After `pair` the
connection is authenticated as if `hello` had succeeded and the host starts
forwarding events. The host does NOT push a snapshot; the client issues
`get_workspace` itself right after a successful `pair`.

### `hello`

```json
{ "id": "…", "op": "hello", "args": { "client": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" } } }
```

Result:

```json
{ "host": { "name": "…", "appVersion": "…", "os": "macos" }, "workspace": <Workspace | null>, "protocol": { "version": 2, "ops": [ … ], "events": [ … ], "features": [] } }
```

The host looks up the handshake's remote static key in `devices.json`; a key
it does not know closes with `4003`. `workspace` is the exact `get_workspace`
result. `protocol` is the same descriptor `pair` answers with, narrowed to this
device's scopes (see "Scopes" and "Compatibility"). After the response the host starts forwarding events for this
connection.

## Scopes

A pairing grants a set of scopes, fixed at `pair` and stored on the device
record. An op outside the set is answered `{ ok: false, error: "forbidden" }`;
the connection stays up, because a scope is a standing fact about the device
and not a bad credential.

The six scopes, and what each one covers:

| scope | covers |
|---|---|
| `observe` | every read: the workspace, transcripts, diffs, PR state, the workflow and roadmap boards, `gh_status`, `list_dir`, `dictation_status` |
| `agents` | spawn, message, answer a tool-use prompt, stop/resume/archive/restore/discard, set model and effort, dictation capture, attachment upload, and the working-tree moves that never leave the machine (`commit_agent`, `pull_agent`, `rebase_agent`, `stash_agent`, `discard_agent_changes`, `abort_merge_agent`) |
| `projects` | add, clone, create, rename, relocate, label, attach/detach and delete projects and their repos |
| `workflows` | launch, cancel, resume, retry, approve, reject and delete runs; save, delete and import stored definitions |
| `roadmap` | create, edit, rank, hand off, hold, release, reject, reopen and delete items; accept or reject the PM's proposals |
| `publish` | the five ops that leave this machine under the user's name: `push_agent`, `create_pr`, `merge_pr`, `roadmap_merge_item_pr`, `answer_publish_approval` |

Every op in the table below has exactly one scope. `register_push` is outside
the scheme and always allowed: it writes the calling device's own APNs token
and grants it nothing over the host.

Two presets are offered at pairing:

- **`full`** — every scope. Today's surface, and the default when no preset is
  named.
- **`control`** — every scope except `publish`. Watch and steer agents, approve
  tool use, add projects, drive workflows and the roadmap; but no push, no PR
  opened or merged, and no publish approved.

Because `protocol.ops` is already narrowed to the device's scopes, a client
needs no scope-specific gating: the actions it hides for "this host does not
have that op" are the same actions it hides for "this device may not". The
`forbidden` error is the backstop, for a client that asks anyway.

**Changing a device's scopes is a revoke and a re-pair.** There is no in-place
edit. A re-pair on a key already on file replaces that record's scopes with the
new code's.

**A record with no `scopes` field means every scope.** Every device paired
before scopes existed was paired into the undivided surface, so anything
narrower would silently take away access it already has. A scope name a host
does not recognize is ignored rather than honoured, and does not invalidate the
record.

## Compatibility

Hosts and clients are released on their own schedules, so either side may be
older than the other. The rules that make that safe:

- **The prologue stays `fletch-remote-v2`.** It is the transport's version, not
  the surface's, and it does not change for an added op, event or feature.
- **Changes within v2 are additive.** New ops, new forwarded events and new
  `features` flags are added; an existing op's name, argument keys or result
  shape is not repurposed. A client may therefore ignore anything it does not
  recognize, in a result or in an event, and a host ignores unknown argument
  keys. So `protocol.version` stays **2** across an addition: putting the whole
  `wf_*` / `roadmap_*` surface and its 14 new events on the wire added rows and
  names and repurposed nothing, and an older client is unaffected because it
  gates on the names it knows.
- **`protocol` says what this host answers.** `{ version, ops, events, features }`:
  `ops` is every name the device may send (the dispatcher's allowlist plus the
  ops the session layer answers itself), `events` the forwarded-event whitelist,
  `features` named behaviours that are neither — none defined yet. It is the
  whole surface, not a delta.
- **`ops` is per device, not per host.** It is the host's surface narrowed to
  the calling device's pairing scopes (see "Scopes"), so the membership gate a
  client already uses to hide what a host lacks also hides what *this device*
  may not do. Nothing else in `protocol` varies by device.
- **A missing `protocol` means the v2 default set**: the ops and events this doc
  listed when the field was introduced (42 ops, 15 events). Only a host from
  before the field omits it, and that is exactly what those hosts answer.
- **Clients gate on membership in `ops`, `events` and `features`** — never on
  `host.appVersion` or on `protocol.version`. A feature whose op is absent is
  hidden or disabled with a reason; it is not attempted and it is not offered.
- **`"unknown op"` means unsupported, not broken.** It is an ordinary error
  response on a healthy connection: the client must treat it as "this host
  cannot do that", show it as such, and not reconnect, retry or report a
  transport fault.

## Operations (v1 allowlist)

Op name, argument keys and result type are identical to the Tauri command of
the same name (see `src/api/domains/*.ts` for the argument keys and
`src/api/types/*.ts` for the DTOs). The host dispatches through an explicit
allowlist; any op not listed returns `{ ok: false, error: "unknown op" }`.

| op | args | result |
|---|---|---|
| `get_workspace` | `{}` | `Workspace \| null` |
| `allocate_draft_name` | `{ drafts: string[] }` | `string` |
| `spawn_agent` | as command; host forces `view: "custom"` and ignores `skills` / `mcpServers` — it resolves `customAgentId` against its own library instead, stamping that row's brief, skills and MCP servers on the session (a dangling id spawns a plain agent, and the phone's `instructions` are used only when the row's brief is blank). `purpose` is honoured only when it is `"roadmap-pm"` (see "Planning chats from the phone") and dropped otherwise | `AgentRecord` |
| `send_user_message` | `{ agentId, turnId, text, attachments: string[] }` — paths from `attachment_end` (see "Attachments") | `boolean` |
| `answer_tool_use` | `{ agentId, requestId, updatedInput, behavior, message? }` | `null` |
| `answer_publish_approval` | `{ id, approved }` — `id` from the `publish:approval-requested` event; an id the host has already timed out is ignored | `null` |
| `stop_agent` | `{ agentId }` | `null` |
| `resume_agent` | `{ agentId }` | `null` |
| `archive_agent` | `{ agentId }` | `null` |
| `restore_agent` | `{ agentId }` | `null` |
| `discard_agent` | `{ agentId }` — destructive: record, checkout and transcript all go | `null` |
| `set_agent_model` | `{ agentId, model }` | `null` |
| `set_agent_effort` | `{ agentId, effort }` | `null` |
| `read_session_records` | `{ agentId }` | `SessionRecord[]` |
| `read_user_turns` | `{ agentId }` | `UserTurn[]` |
| `sync_session` | `{ agentId }` | `null` |
| `read_live_turn` | `{ agentId }` — the `event` payloads of the agent's current turn, oldest first, as they were forwarded on `agent:event`; `dropped` counts events cut from the head when the turn outgrew the host's buffer; `next_seq` is the `seq` the agent's next `agent:event` will carry, so a frame with `seq >= next_seq` is one the snapshot does not hold. Empty for a turn that ran under a previous host process or in the native view. No desktop command of this name yet | `{ events: object[], dropped: number, next_seq: number }` |
| `get_git_state` | `{ agentId }` | `GitState \| null` |
| `get_all_shortstats` | `{}` — uncommitted working-tree stats for every live agent; archived and still-cloning agents are omitted | `Record<agentId, ShortStats>` |
| `get_all_git_meta` | `{}` — advisory local-git metadata per checkout (base staleness, changed paths), keyed like the PR maps (`agentId` for the primary repo, `"{agentId}::{subdir}"` for secondaries); no network | `Record<gitKey, GitMeta>` |
| `list_checkout_tree` | as command | `CheckoutFile[]` |
| `read_checkout_file` | `{ agentId, path, baseMode? }` | `CheckoutFileContents` |
| `get_file_diff` | as command | `string` |
| `commit_agent` | as command | `null` |
| `push_agent` | as command | `string` |
| `pull_agent` | as command — `git pull` in the checkout, with the same GitHub credential `push_agent` spends | `null` |
| `rebase_agent` | as command — rebases the checkout onto its base's resolved tip, not onto the clone's stale local base ref | `null` |
| `stash_agent` | as command — stashes the working tree, untracked files included | `null` |
| `discard_agent_changes` | as command — destructive: every uncommitted change in the checkout goes. Not `discard_agent`, which takes the whole session | `null` |
| `abort_merge_agent` | as command — `git merge --abort` in the checkout | `null` |
| `create_pr` | as command | `PrState` |
| `merge_pr` | as command — merges the open PR on the targeted repo's branch | `null` |
| `get_pr_state` | as command | `PrState \| null` |
| `get_pr_checks` | as command | `PrChecks \| null` |
| `get_pr_live` | as command | `PrLive \| null` |
| `get_pr_threads` | as command — unresolved review threads; GraphQL, so polled well below the `get_pr_live` cadence | `PrComments \| null` |
| `list_repo_branches` | `{ repoPath }` | `string[]` |
| `repo_default_branch` | `{ repoPath }` | `string` |
| `discover_supported_models` | as command | `AgentModels[]` |
| `list_dir` | `{ path }` (tilde-expanded on the host) | `DirListing` — entries sorted directories-first then by name and capped at 1000, with `truncated` saying the cap bit; each entry carries `is_repo` (a directory holding a `.git`). Both fields were added within v2, so a client reading a host from before them sees no marks and no truncation flag |
| `add_workspace_repo` | `{ repoPath }` | `Workspace` |
| `clone_repo` | `{ spec, destParent }` | `Workspace` |
| `create_repo` | `{ name, destParent, private, description?, publish? }` — seeds the repo on the *host* (README + initial commit) and pins it; `publish: false` keeps it local-only, absent publishes | `Workspace` |
| `gh_status` | `{}` | `GhStatus` |
| `gh_repo_list` | `{}` | `GhRepoSummary[]` |
| `remove_workspace_repo` | `{ repoPath }` — unpins the repo; the folder on the host is untouched | `Workspace` |
| `attach_repo_to_project` | `{ projectId, repoPath }` — two-phase on the host (database, then `git init` for a folder that is not a repository yet), rolled back if the second phase fails | `Workspace` |
| `detach_repo_from_project` | `{ projectId, repoPath }` — refuses a project's last repo and any repo an agent checkout still references | `Workspace` |
| `set_repo_label` | `{ repoPath, label }` — blank clears back to the folder-basename fallback | `Workspace` |
| `rename_project` | `{ projectId, name }` — the display name only; no folder is renamed | `Workspace` |
| `project_has_running_agents` | `{ projectId }` — the read the Delete section polls | `boolean` |
| `delete_project` | `{ projectId }` — destructive: the project's agents, their checkouts and transcripts, and its workflow runs all go. Refused while any of its agents is running. The repository folders themselves are left alone | `ProjectDeleteResult` |
| `relocate_repo` | `{ oldPath, newPath }` — both paths are on the host; it validates the destination is a git repository there and moves nothing | `Workspace` |
| `dictation_status` | `{}` (remote-only, see "Dictation") | `{ available: boolean, reason: string \| null }` |
| `dictation_begin` | `{}` (remote-only) | `{ session: string, auto_stop: boolean }` |
| `dictation_audio` | `{ session, rate: number, pcm: string }` — base64 of 16-bit little-endian mono PCM at `rate` Hz (remote-only) | `null` |
| `dictation_end` | `{ session }` (remote-only) | `{ text: string }` |
| `dictation_cancel` | `{ session }` (remote-only) | `null` |
| `attachment_begin` | `{ name }` (remote-only, see "Attachments") | `{ upload: string }` |
| `attachment_chunk` | `{ upload, data: string }` — base64 of the file's next bytes (remote-only) | `null` |
| `attachment_end` | `{ upload }` (remote-only) | `{ path: string }` |
| `attachment_cancel` | `{ upload }` (remote-only) | `null` |
| `list_project_chats` | `{ projectId, purpose }` — `purpose` is `"roadmap-pm"` | `AgentRecord[]` |
| `list_custom_agents` | `{}` — the stored `custom_agents` rows, newest-edited first; `skill_ids` / `mcp_server_ids` are JSON text as stored | `CustomAgentRow[]` |
| `get_agent` | `{ agentId }` — one record by id, including the ones `get_workspace` hides (purpose-scoped chats, run-owned steps) | `AgentRecord \| null` |
| `wf_list_runs` | `{ projectId? }` — every run, newest-updated first; `null` lists them all | `WfRun[]` |
| `wf_get_run` | `{ runId }` | `WfRunDetail \| null` |
| `wf_events` | `{ runId, afterSeq, limit }` — a journal page, oldest first; `limit` is clamped host-side to 1000 | `WfEvent[]` |
| `wf_run_agents` | `{ runId }` — a run's step agents, live and archived; they are hidden from `get_workspace`, so the monitor reads them here | `AgentRecord[]` |
| `wf_launch` | as command — `attachments` are host paths from `attachment_end`, exactly as `send_user_message` takes them; the host resolves `projectId` from `repoPath` itself | `string` (the new run id) |
| `wf_cancel` | `{ runId }` | `null` |
| `wf_resume` | `{ runId, budgetPatch? }` — the patch additively raises the run-level caps | `null` |
| `wf_retry` | `{ runId }` | `null` |
| `wf_approve` | `{ runId }` | `null` |
| `wf_reject` | `{ runId, note }` | `null` |
| `wf_run_diff` | `{ runId, fromSha, toSha, path? }` — read-only, inside the run's own repo | `string` |
| `wf_resolve_conflict` | `{ runId, mode: "agent" \| "human" }` — `"human"` says the conflict was already resolved by hand in the host's integration worktree, so a client without a shell there sends `"agent"` | `null` |
| `wf_delete_run` | `{ runId }` — destructive: the run's step agents and their chats, its directory and its rows all go; refused while any run in the tree is active | `null` |
| `wf_answer` | `{ projectId, runId, messageId, body }` | `null` |
| `wf_def_save` | `{ spec, id?, hue? }` — omit `id` to create; an existing id edits in place | `Definition` |
| `wf_def_list` | `{}` | `Definition[]` |
| `wf_def_delete` | `{ id }` — in-flight runs keep their own launch snapshot | `null` |
| `wf_def_export_yaml` | `{ id }` — returns YAML *text*; picking a path and writing the file is the client's own business, on its own machine | `string` |
| `wf_def_import_yaml` | `{ yamlText }` — resolved against the *host's* library, so missing skills and unknown providers come back as warnings | `ImportReport` |
| `roadmap_list_items` | `{ projectId }` | `RoadmapItem[]` |
| `roadmap_get_item` | `{ itemId }` | `RoadmapItem \| null` |
| `roadmap_create_item` | `{ projectId, item }` — `code` is allocated host-side under the connection lock, so it is not part of the payload | `RoadmapItem` |
| `roadmap_update_item` | `{ id, patch, expectStatus?, queue? }` | `{ applied: boolean, item: RoadmapItem }` |
| `roadmap_set_rank` | `{ itemId, rank }` — bookkeeping, writes no history line | `RoadmapItem` |
| `roadmap_hand_off_item` | `{ itemId, agentId }` | `RoadmapItem` |
| `roadmap_item_review` | `{ itemId }` — one `in_review` item's live CI rollup and threads; read-only, each field degrades on its own | `RoadmapItemReview \| null` |
| `roadmap_merge_item_pr` | `{ itemId }` — the same host path and credential `merge_pr` uses; the merge sweep is still what writes `done` | `null` |
| `roadmap_note_review_feedback` | `{ itemId, threads }` | `RoadmapItemEvent` |
| `roadmap_hold_item` | `{ itemId, reason }` | `RoadmapItem` |
| `roadmap_release_item` | `{ itemId }` — the user's alone; the PM has an op to hold and none to release | `RoadmapItem` |
| `roadmap_hold_project` | `{ projectId, reason }` — stops the whole board; runs already in flight still settle | `RoadmapProjectHold` |
| `roadmap_release_project` | `{ projectId }` | `null` |
| `roadmap_get_project_hold` | `{ projectId }` | `RoadmapProjectHold \| null` |
| `roadmap_reclaim_item` | `{ itemId }` | `RoadmapItem` |
| `roadmap_reject_item` | `{ itemId, reason }` | `RoadmapItem` |
| `roadmap_reopen_item` | `{ itemId }` | `RoadmapItem` |
| `roadmap_delete_item` | `{ id }` — the board's own Remove: unconditional, and it takes the row's history with it | `null` |
| `roadmap_discard_proposal` | `{ id }` — deletes only while the item is still `proposed` | `{ applied: boolean, item: RoadmapItem \| null }` — `applied` with `item: null` when it was deleted; `applied: false` with the current `item` when it had already been accepted; `applied: false, item: null` when the id is gone |
| `roadmap_list_item_events` | `{ itemId }` — one item's durable history, newest first | `RoadmapItemEvent[]` |
| `roadmap_latest_events` | `{ projectId }` — the newest event of every item on the board | `RoadmapItemEvent[]` |
| `roadmap_list_proposals` | `{ projectId }` | `RoadmapProposal[]` |
| `roadmap_accept_proposal` | `{ proposalId }` — rejects with a message when the item raced past the gate | `null` |
| `roadmap_reject_proposal` | `{ proposalId }` | `null` |
| `roadmap_get_order_proposal` | `{ projectId }` | `RoadmapOrderProposal \| null` |
| `roadmap_accept_order_proposal` | `{ projectId }` — ranks the whole sequence in one transaction; rejects when the orderable set changed since the ask | `null` |
| `roadmap_reject_order_proposal` | `{ projectId }` | `null` |
| `roadmap_get_brief` | `{ projectId }` | `RoadmapBrief \| null` |
| `roadmap_get_brief_proposal` | `{ projectId }` | `RoadmapBriefProposal \| null` |
| `roadmap_accept_brief_proposal` | `{ projectId }` — the only thing that writes product memory | `RoadmapBrief` |
| `roadmap_reject_brief_proposal` | `{ projectId }` | `null` |
| `host_providers` | `{}` — which provider CLIs this host has and which of them are signed in, so a client never offers to spawn one the host cannot run (see "Which providers a host can run"). Read-only; no desktop command of this name | `{ id, label, installed, version: string \| null, auth: "signed_in" \| "signed_out" \| "unknown" \| null, loginCommand: string \| null }[]` |
| `register_push` | `{ token: string \| null, environment?: "sandbox" \| "production" }` — `environment` required with a token, ignored on clear (remote-only, see "Push notifications") | `null` |

Never exposed, by design: the generic `db_*` table bridge, every file mutation
(`write_checkout_file`, `rename_*`, `delete_*`, `create_*`, `copy_*`), shell
ops (`open_agent_shell`, `write_to_shell`, …), `write_to_agent` (raw PTY),
editor/log/telemetry ops, the provider install/login ops (`install_agent`,
`open_provider_login`) and the two desktop provider probes behind Settings ›
Providers (`probe_provider_versions`, `probe_provider_auth`, which answer with
resolved binary paths), and the `run_*` family (`run_start`, `run_stop`,
`run_verification` — this machine's own scripts). `host_providers` above is the
read-only half of the provider surface and the only part of it on the wire:
state and the command that would change it, never the change itself and never a
path. Adding an op means adding a row here and a match arm in the dispatcher.

Withheld on policy, not scope: `delete_branch_agent`. Every other Git-panel
action acts inside the agent's own checkout, which is the reach `commit_agent`
has always had; this one runs `git branch -D` in `TrackedRepo.repo_path` — the
user's real clone, outside every checkout — so a remote client cannot ask for
it. The branch it would delete is the agent's own recorded branch and never
comes from the request, but the write still lands in a repository the protocol
does not otherwise touch, which is the line `rpc/caps.rs` draws for agents:
force is confined to the branch the host itself materialized. Deleting a merged
branch is done on the host, or on GitHub.

Not yet exposed, but only for want of a reason to be: `fork_agent` and the two
mention sources the composers use (`list_repo_tree`, `list_repo_prs`, which stay
off with their read families). These are scope, not policy — a client gates each
one on its absence from `protocol.ops` and says so, and a later release may add
the row. `merge_pr` was in this group until it earned its row above: the
credential it spends is the one `push_agent` and `create_pr` already spend, and
the five working-tree ops (`pull_agent`, `rebase_agent`, `stash_agent`,
`discard_agent_changes`, `abort_merge_agent`) joined it for the same reason.
`delete_project` and `create_repo` left with the project-settings family below,
for the same kind of reason: they act on the project list and on the folders the
host already pins, and nothing outside them.

The spawn flow is the desktop's: `allocate_draft_name` → `spawn_agent` →
wait for `agent:status` to leave `spawning` → `send_user_message` with the
prompt as the first turn (`turnId` = client UUID). The phone does not send
the prompt through `instructions`.

### Which providers a host can run

An agent only runs if its vendor CLI is installed *and* signed in, on the
machine it spawns on — so with a remote environment active, the provider list
that matters is the host's, not the client's. `host_providers` answers it: one
row per provider the engine knows, with `installed`, the version, and `auth`
(`null` for a CLI that is not there — there is no login state to report about a
binary that is not present, and `unknown` is a probe that could not tell rather
than a claim, so a client must not block on it).

Clients fetch it once per connection, on the handshake snapshot, and disable a
provider that is missing or signed out rather than letting the spawn fail
later. The fix is the operator's and happens on the host — `loginCommand` is
the vendor's own command, carried so the client can quote
`fletch-host provider login <id>` beside the reason instead of offering a
button it must not have. A host too old to list the op reports nothing, and the
client offers every provider as before.

Settings › Providers stays this Mac's, whatever environment is active: it
installs binaries and opens login PTYs here, and those ops are never on the
wire. With a remote host active it says so in one line.

Adding a project from the phone reuses the desktop's commands unchanged. Two
flows: **open an existing folder** on the Mac (`list_dir` to browse, then
`add_workspace_repo`) and **clone from GitHub** (`gh_status` to know whether
`gh` is signed in, `gh_repo_list` to pick one of the user's repos or a typed
`owner/repo` / URL, `list_dir` to pick the destination parent, then
`clone_repo`). The result of either is the new `Workspace`, which the caller
applies itself; on success the host also emits `workspace:changed` (forwarded
to every phone like any whitelisted event), so the desktop window and the other
paired phones reload their project list at once. Applying the result and
receiving the event both replace the workspace wholesale, so the order they
land in does not matter. Note `DirListing.entries[].is_dir` is snake_case:
`DirEntry` is serialized as-is, the one non-camelCase payload on the allowlist.
Pinning a folder that is not yet a git repository runs `git init` plus an
initial commit in it, exactly as the desktop dialog does. The phone remembers
the last destination parent per host, as the desktop does. The third flow,
**create a new repo** (`list_dir` for the parent, then `create_repo`), seeds and
commits the repo on the host; it publishes to GitHub with the host's own `gh`
connection, and `publish: false` keeps it local there, exactly as the desktop
dialog does when no connection exists.

Project settings from a remote client are the same commands as well: a repo's
label (`set_repo_label`), the repos a project is made of
(`attach_repo_to_project`, `detach_repo_from_project`, `relocate_repo`,
`remove_workspace_repo`), the project's display name (`rename_project`), and
deleting it (`project_has_running_agents` to know whether the button is live,
then `delete_project`). Every path in them is a path on the *host*, so a client
picks one with `list_dir` rather than its own file dialog, and nothing here
moves, renames or deletes a folder on disk: attach may `git init` a folder that
is not a repository yet, and that is the only filesystem write in the group.
Each of these answers the new `Workspace` (or, for `delete_project`, a
`ProjectDeleteResult`) and the host also emits `workspace:changed`, as the
add-project ops do — the desktop commands emit nothing, because one window
applying the result is the whole audience there.

Planning chats from the phone are the desktop Roadmap tab's Project Manager
chat, reached through the same commands. `list_custom_agents` returns the
user's presets so the phone can find the Project Manager one; `spawn_agent`
then opens the chat with `purpose: "roadmap-pm"` and that preset's
`customAgentId` (the only `purpose` the host accepts — any other value is
dropped and the spawn lands as an ordinary sidebar agent). The phone names the
preset and nothing more: the host reads the row itself and stamps its brief,
its skills and its MCP servers on the session, so a planning chat keeps the
tools the desktop gives it while MCP commands, tokens and headers never cross
the wire.

A PM chat is absent from `get_workspace`, so `list_project_chats` with
`{ projectId, purpose: "roadmap-pm" }` is how the phone lists them, and
`get_agent` is how it resolves a single one — a cold open from a push
notification knows only the id it carried. The chat itself is
`send_user_message` and `agent:event` like any other. What the PM proposes
arrives as `roadmap:item` events and is read back with `roadmap_list_items`;
the user accepts a proposal with `roadmap_update_item` (`patch:
{ status: "open" }`, `expectStatus: "proposed"` so two clients cannot both
accept the same ghost row, plus `queue: true` to dispatch it straight away) and
discards one with `roadmap_discard_proposal`, which comes back as
`roadmap:item-deleted`. Discard is conditional for the same reason accept is: a
phone can hold a `proposed` card for minutes, so the host refuses it once the
row has been ruled on and answers `applied: false` with the row as it now
stands rather than deleting work in flight. `roadmap_delete_item` is the
unconditional twin, for the board's own Remove.

Workflows and the roadmap board are on the wire whole, so a paired desktop or
phone drives autopilot, workflows and planning against a headless host
(docs/multi-host-plan.md §5.3, item 2). All 18 `wf_*` and 30 `roadmap_*`
commands have a row above, plus `wf_run_agents` and the remote-only
`roadmap_discard_proposal`: none of them opens a dialog on the host, touches
desktop-only state, or is a host policy decision, so there was nothing to
withhold. The flows around them keep the exclusions of the families they borrow
from — the composers' `@file` / `#PR` mention sources (`list_repo_tree`,
`list_repo_prs`) and the desktop autopilot ladder's `run_verification` — and a
client gates them by name like any other absent op.

Two things a client should know before driving them. `wf_launch`'s
`attachments` are paths on the *host*, so upload with `attachment_*` first and
pass what `attachment_end` answered, exactly as for `send_user_message`. And
the ladder that walks one agent's checkout through commit → push → PR is the
desktop's own loop over its own engine, not an op: its opt-outs are rows in the
client's local `settings` / `project_settings`, and its verify rung runs local
scripts. The host's autonomous loop is the roadmap *queue*, which runs on the
host and is driven by the board ops above — holding a project or an item is how
a client stops it.

## Dictation

The phone has a mic button too, but no speech model: it captures, and the Mac
transcribes with the local whisper.cpp engine the desktop composer can opt into
(docs/dictation.md, "Local Whisper engine"). The five `dictation_*` ops are
remote-only — the desktop has its own microphone and its own commands — and
answer through the generic dispatcher, since none of them needs to know which
device is asking.

- **Hosts without a speech engine.** The five names are on the allowlist of
  *every* host, so a phone may always ask and the capability descriptor always
  advertises them. A host with no local engine behind it — a Linux
  `fletch-host`, or a non-macOS desktop build, where whisper.cpp is not compiled
  — answers `dictation_status` with `available: false` and a `reason` of
  "Dictation from a phone needs a Mac host: whisper.cpp is only built there.",
  and fails the four capture ops with that same message. Neither is `unknown
  op`: the op exists, the host just cannot serve it.
- **Preconditions.** The Mac's `dictation_engine` setting must be `whisper` and
  the selected model's weights must be on disk. `dictation_status` reports
  exactly that as `available`, with a `reason` the phone shows as-is when it is
  false ("Local dictation is off on your Mac. Turn on the Whisper engine in
  Settings › Dictation."). There is no fallback to Apple's recognizer on this
  path: it needs a microphone the Mac does not have.
- **Ending a session.** `dictation_begin` answers with `auto_stop`, the Mac's
  "Stop after a pause" setting (Settings › Dictation) as it stands at that
  moment. The pause is heard in frames that never cross the wire — the host
  only ever sees the chunks the phone chose to send — so the phone is the only
  side that can honour it: with `auto_stop: false` it never arms its silence
  monitor, and nothing but a tap ends the session, matching what the setting
  does to a desktop session (neither the pause nor the never-spoke deadline
  fires).

  It rides on `begin` rather than `dictation_status` because the phone probes
  status on mount and reconnect only; answering it per session means a setting
  flipped on the Mac takes hold on the phone's next session instead of waiting
  for it to reconnect, and costs no extra round trip.

  With `auto_stop` off, nothing on the host ends the session either. The idle
  sweep does not apply to a phone that is still streaming — every chunk
  refreshes it — so what stays bounded is the audio, not the session: an
  abandoned one holds the phone's mic until the user taps, leaves the screen,
  or the link drops. That is the same deal the desktop offers with the setting
  off.
- **Flow.** `dictation_begin` re-checks the preconditions (so the phone learns
  before its mic opens) and answers with a host-minted `session` id. While the
  user talks, the phone sends `dictation_audio` about once a second with the
  PCM captured since the last chunk. On stop it sends `dictation_end` and shows
  "transcribing" until the reply, whose `text` is the whole transcript (empty
  when the clip had no speech in it — the engine's silence gate, identical to
  the desktop). `dictation_cancel` throws the audio away; it is idempotent.
- **Audio.** `pcm` is base64 of 16-bit little-endian mono samples at `rate` Hz,
  whatever rate the phone's audio session runs at (typically 48 000). The rate
  is fixed by the first chunk of a session; a chunk that names another is
  refused. The host resamples to the model's 16 kHz itself with the same
  converter the desktop path uses. A decoded chunk over 2 MiB is refused.
- **Bounds.** Everything a session holds is bounded and lives in memory only.
  Audio past the desktop's capture cap (five minutes) is dropped, not refused,
  so the transcript is truncated rather than the session failing. At most 4
  sessions are open at once across every phone; a session that has received no
  chunk for 60 s is swept (a phone that lost its connection mid-sentence never
  sends `dictation_end`), after which its id is unknown. Nothing is written to
  disk, and no `dictation:*` event is forwarded — the transcript is the reply.
- **Wire cost.** Chunks are about 100 KB a second at 48 kHz mono before base64,
  well under the 4 MiB frame cap and the relay's 100-messages-per-10-s limit at
  one chunk a second. Two decodes never run at once (the engine serialises
  them), so a phone's transcription may wait behind a desktop session's.

## Attachments

The phone can attach photos and files to a message. They have no path on the
Mac, so — like a screenshot pasted into the desktop composer — their bytes are
written into the app-data staging area first and the message names the staged
path; `send_user_message` then moves each staged file into the agent's
workspace (`.fletch-attachments/`) and rewrites the path, the same adoption the
desktop's paste goes through. The four `attachment_*` ops are remote-only and
answer through the generic dispatcher.

- **Flow.** `attachment_begin` names the file and answers with a host-minted
  `upload` id. The phone sends `attachment_chunk` with consecutive slices of the
  file, in order, one in flight at a time; `attachment_end` closes the upload
  and answers with the staged `path`, which the phone holds until the user
  sends and then puts in `attachments`. `attachment_cancel` drops the upload
  and its partial file; it is idempotent. A phone that leaves the screen with
  uploads staged but unsent leaves them in staging, as an unsent desktop paste
  does.
- **Chunking.** A device WebSocket message is capped at 4 MiB and a phone
  screenshot base64-inflates past it, so the phone slices at 1 MiB (about
  1.4 MiB on the wire). A decoded chunk over 2 MiB is refused.
- **Names.** `name` is the display filename; the host keeps only its last
  component, so a path-shaped name can only ever name a file inside the
  upload's own staging dir. Empty or `..` becomes `attachment`.
- **Bounds.** Chunks are written to disk as they arrive, not buffered — a
  photo is tens of megabytes. An upload over 32 MiB is refused and removed
  (a truncated file is a corrupt one, not a shorter one, so unlike dictation
  nothing is silently dropped). The phone applies the same cap before it reads
  a picked file, so a video or archive chosen by mistake never enters the
  webview's memory or the wire. At most 8 uploads are open at once across
  every phone; one that has received no chunk for 120 s is swept with its
  partial file, after which its id is unknown.
- **Images.** The phone re-encodes large or HEIC images to JPEG before
  uploading, since most agents cannot read HEIC and a raw camera photo is a
  poor use of the relay; small PNG/JPEG files go through untouched.

## Events (v1 whitelist)

Forwarded verbatim with the desktop event name and payload (see
`src/api/events.ts` for payload types). The host subscribes to the engine's own
event stream — the same events the desktop webview gets — and forwards only
these names to every authenticated connection:

```
agent:event            agent:status           agent:task
agent:branch           agent:model            agent:effort
agent:repo_added       agent:git-action       session:records-appended
turn:sent              turn:started           workspace:changed
pr:state_changed       verify:report          publish:approval-requested
publish:approval-resolved
wf:event               wf:run                 wf:run-deleted
roadmap:item           roadmap:item-deleted   roadmap:item-event
roadmap:proposal       roadmap:proposal-deleted
roadmap:order-proposal roadmap:order-proposal-deleted
roadmap:project-hold   roadmap:project-hold-released
roadmap:brief          roadmap:brief-proposal roadmap:brief-proposal-deleted
roadmap:queue-note
```

`agent:event` is forwarded unfiltered, including the provider's
`control_request` records: that is the only way a held tool-use approval
reaches the phone, and `answer_tool_use` needs the `request_id` it carries.
`turn:sent` carries every accepted user message (`turn_id`, `text`,
`attachments`, `follow_up`) whichever client sent it, so a chat reads the same
on every device; a client skips the one whose `turn_id` matches its own
optimistic bubble.
On the `error` status transition, `agent:status` must carry the real
`last_error`, since both clients keep the previous error when it is null.
`publish:approval-requested` is a held prompt like a tool-use approval: the
agent's publish is blocked on the host until someone answers with
`answer_publish_approval` (or the host's wait lapses and refuses it). A client
that does not have that op on `protocol.ops` can show the prompt but not answer
it.

`publish:approval-resolved` `{ id, outcome: "approved" | "denied" | "expired" }`
closes one of those prompts. It fires once per `publish:approval-requested`,
whoever ended it: the device that answered, another device, `fletch-host
approve`, or nobody at all — `expired` covers both the lapsed
`publish_approval_wait` and a prompt that could not be delivered. A client drops
the card or dialog for `id` and needs no other reaction; the verdict itself is
already the answerer's, and the agent has been told. A host that does not list
it on `protocol.events` never emits it, and a client that has no handler for it
behaves as it did before the event existed — the prompt stays until answered,
which is what every client did until this event.

The whole `wf:*` and `roadmap:*` stream is forwarded, so a remote run monitor
and a remote board stay live instead of rendering once and going stale. Three
of them carry an id rather than a row and are named for what they address:
`wf:run-deleted` and `roadmap:item-deleted` fire the deleted row's id, while
`roadmap:order-proposal-deleted`, `roadmap:project-hold-released` and
`roadmap:brief-proposal-deleted` fire the *project* id — those three are facts
about a board, not about a row.

`wf:event` is an addressing envelope only — `{ run_id, seq, type, ts,
step_exec_id }` — so nothing transcript-derived is duplicated onto the stream;
a client that wants the payload pages it back with `wf_events` from the `seq` it
last saw. `wf:run` carries the full run row, so the sidebar and the monitor
update without a round-trip. `roadmap:queue-note` is transient and never
persisted: it explains why an item is not moving, and nothing reads it back.

Never forwarded: `agent:output`, `shell:output`, `run:output` (raw PTY bytes),
the rest of `run:*`, `dictation:*`, `docker:*` and `agent-install:*`.

Delivery is best effort, exactly like the desktop frontend: the phone must
refetch `get_workspace` on reconnect and on returning to the foreground, and
`read_session_records` when it opens an agent. An empty record list is not
proof of an empty conversation — the turn-end ingest can lag or miss — so the
phone then asks the host to `sync_session` and reads once more, and keeps
whatever log it already rendered from live events if that is still empty.

Records stop at the last *finished* turn: the running one is ingested only when
it ends. A phone that opens a busy agent therefore also asks for
`read_live_turn` and folds those events onto the rebuilt log exactly as it
folds the live `agent:event` stream — so a turn whose frames were lost to a
backgrounded app or a dropped socket is rendered whole, for every provider,
and the frames still to come continue from where the replay left off. A host
without the op (gate on `protocol.ops`) leaves the phone with the live log it
already has, as before.

## Errors

Host errors are strings (the `Display` of the Rust `Error`). Three are
reserved: `"unknown op"` for anything off the allowlist, `"forbidden"` for an op
this host has but this *device's* pairing scopes do not reach (see "Scopes"),
and `"too many in-flight requests"` for a connection over its concurrency cap.

`"forbidden"` is deliberately distinct from `"unknown op"`: the op exists here,
so a client should say "re-pair this device with more access" rather than "this
host cannot do that". Neither is a transport fault; neither warrants a retry or
a reconnect.

Auth failures are WebSocket close codes, not error responses: `4001` failed
handshake, text frame, or bad first frame; `4003` unauthenticated, unknown
device key or revoked; `4004` host has remote access disabled. `4003` also arrives unprompted when the host revokes the device this
connection is authenticated as, and `4004` when the host turns remote access
off — in both cases the credential is gone or dormant, so the client should
stop reconnecting until it is paired or the host is enabled again. `1012`
(service restart) arrives when the host moves its listener to another port; it
is retryable, and a relayed device reconnects without noticing, since the relay
routes on the host key. A LAN-only device still dials the old port until it
learns the new one.

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
| `remote_status` | `{ enabled, listening, port, name, hostId, addresses: string[], devices: RemoteDevice[], relay: RelayStatus, error: string \| null }` — `hostId` is the host public key, base64url; `name` is what a device sees this machine called (the same string `pair`/`hello` and the pairing URL carry), which is also how a desktop acting as a *client* names itself when it pairs with another host |
| `remote_set_enabled` | `{ enabled }` start/stop the listener and the relay link; persists setting `remote.enabled`; disabling closes live connections with `4004` |
| `remote_set_port` | `{ port }` persist setting `remote.port`; a running listener moves to it at once (its connections close with `1012`, the relay link stays up), an idle one records it for the next start; refused, with nothing stored, when the port cannot be bound; returns `RemoteStatus` |
| `remote_set_relay` | `{ url: string \| null }` persist setting `remote.relay_url` (null/empty clears it) and connect or drop the host link accordingly; returns `RemoteStatus` |
| `remote_begin_pairing` | `{ preset?: "full" \| "control" }` (absent is `full`) → `{ token, url, expiresAt, preset, scopes }`; refused for a preset the host does not define, and while the listener is down or `error` is set |
| `remote_revoke_device` | `{ deviceId }`; drops the credential and closes that device's live connections with `4003` |

`RemoteDevice = { deviceId, name, platform, createdAt, lastSeenAt, connected, pushEnabled, scopes }`,
where `connected` is derived from the live connections, not from `lastSeenAt`,
and `scopes` is what the device was paired with (see "Scopes").
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

QR scanning on the phone (manual entry of address and code, plus
`fletch://pair` deep-link parsing, for now), creating a brand-new repo from the
phone (`create_repo`), Run scripts, Keychain storage of the device key on the
phone (it lives in the app data dir). Attachment follow-ups: sweeping staged
uploads the phone never sent (today they linger like an unsent desktop paste),
and thumbnails on the phone's own bubbles. Push follow-ups:
feeding APNs `410 Unregistered` back to the host so stale tokens are dropped,
noticing a permission revoked in iOS Settings (the phone has no way to observe
it today, so the host keeps a token that can no longer alert), a "mute while
I'm at the Mac" setting, per-agent muting.
