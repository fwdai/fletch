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
  reconnect. Client reconnects with exponential backoff (1 s, 2 s, 4 s … 30 s);
  a phone returning to the foreground with a retry pending dials at once
  rather than wait it out.
- WebSocket messages larger than 4 MiB are rejected (close code 1009). A
  protocol frame is not bound by that cap: on a connection whose two ends
  negotiated fragmentation (see "Secure channel"), a frame over 256 KiB
  travels as a run of messages of 256 KiB each, up to 64 MiB in total. The
  host holds itself to the matching cap in the other direction — 4 MiB toward
  a peer that did not negotiate fragmentation, since the relay closes the
  whole *host link* with `1009` for a message over it and every relayed device
  goes with it, and 64 MiB toward one that did: a response that would exceed
  it is answered with `{ ok: false, error: "response too large" }` instead,
  and an event that would is dropped for that connection (delivery is best
  effort; the turn-end refetch carries what it would have).
- Everything the host holds for a connection is bounded, and ends with it. The
  outbound queue holds 64 frames: a client that stops reading while the host
  still has frames for it has its socket dropped (no close frame — the queue is
  what one would travel through), and reconnects. At most 8 requests may be in
  flight per connection; further requests are answered immediately with
  `{ ok: false, error: "too many in-flight requests" }` and never dispatched.
  Requests still running when the socket goes away are abandoned.
- The client keeps to the same cap: beyond 8 outstanding requests it queues
  the rest in order and sends each as an earlier one is answered, so the burst
  of reads that follows every handshake is never refused. It counts every
  request, `pair`/`hello` and `register_push` included, although the host
  answers those inline outside its count. A dropped socket rejects queued
  requests along with in-flight ones.
- A request has no timeout unless its caller sets one, and only a read should:
  an op that changes something on the Mac is still running there after the
  client gives up on it, and reporting it failed invites a retry of work that
  is happening. The phone's reads ask for 30 s. A request that times out is
  rejected and the socket is left alone, but its in-flight slot stays taken
  until the host's answer arrives (and is then ignored) or the socket goes —
  the host is still counting it against the 8, and handing the slot on early
  would only get the next request refused. The clock starts when the request
  is sent, not while it waits in the queue. `pair`/`hello` have the
  handshake's own 15 s bound (see "Relay" → "Phone side").

## Secure channel

Every connection starts with a Noise handshake and carries only encrypted
frames afterwards. The WebSocket, and any relay between the two ends, sees
ciphertext.

- Pattern `Noise_XX_25519_ChaChaPoly_BLAKE2s`, prologue the ASCII bytes
  `fletch-remote-v2`. The phone is the initiator and the host the responder.
  The three handshake messages travel as three WebSocket **binary** messages:
  `-> e`, `<- e, ee, s, es`, `-> s, se`.
- **Handshake payloads carry capabilities.** Message 1 has an empty payload
  (nothing is encrypted yet). Messages 2 (the host's) and 3 (the client's)
  each carry one capability byte, a bit set: `0x01` = "I reassemble
  fragmented frames" (see below); every other bit is reserved and ignored. A
  reader looks at the first payload byte only and ignores any bytes after it;
  an empty payload means no capabilities. Both payloads are encrypted and
  authenticated by the handshake, so nobody between the two ends can strip
  them. Ends that predate capabilities send empty payloads and discard the
  payloads they receive, which is why adding the byte changed nothing for
  them. A capability is in force on a connection only when **both** ends
  advertised it; each end computes that the same way once the handshake is
  done.
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
- **Fragmentation.** On a connection where both ends advertised `0x01`, a
  frame over 262 144 bytes (256 KiB) of plaintext may be sent as a run of
  *fragment messages* instead, one per 256 KiB of the frame. The host always
  fragments such a frame: its messages to every relayed device share one host
  link, and a small message for one phone should not wait behind megabytes for
  another. A client fragments only a frame that would not fit one message
  under the 4 MiB cap, because the relay's rate limit counts the messages a
  device sends (see "Relay"); every frame a client sends today fits, so its
  traffic is unchanged. A fragment
  message is `0x00 0x00` followed by the usual chunks; the zero-length marker
  cannot open an ordinary message (a chunk shorter than its tag is malformed),
  so the two forms never collide. The chunks' plaintext is one flag byte and
  then the frame's next *piece*: `0x01` means more fragments of this frame
  follow, and its piece is exactly 262 144 bytes; `0x00` that this is the last
  one, and its piece is 1 to 262 144 bytes. Any other flag, and a piece of any
  other size, is malformed. That is the only shape an encoder ever produces,
  and it means a run of at most 64 MiB is at most 256 messages, with no
  counter or timer: a run cannot be kept open with empty or tiny pieces. The
  flag is inside the ciphertext, so a run cannot be cut short or extended
  undetected. A frame at or under 256 KiB, a larger frame a
  client sends whole, and every frame on a connection without the capability
  is one ordinary message exactly as above; a receiver accepts either form
  whatever the frame's size. The fragments of one frame are consecutive messages on the
  connection — no other frame between them — which is also what keeps the
  Noise nonces in step, since every fragment is an encryption on the
  connection's one channel. A WebSocket is ordered and reliable, so there are
  no fragment ids, no reordering and no timers. A reassembled frame is capped
  at 64 MiB.
- **Reassembly before authentication.** The host reassembles at most 64 KiB
  of fragment run until the connection's first frame has authenticated the
  peer, and the full 64 MiB from the moment `pair` or `hello` succeeds. Both
  frames are a few hundred bytes, so no legitimate client fragments before
  then, and a stranger who completed the handshake cannot make the host hold
  megabytes for it. The client takes the full 64 MiB from the start: its peer
  is the pinned host. An ordinary message needs no such rule, since the 4 MiB
  message cap already bounds it.
- A handshake that fails, a text frame at any point, or a frame that does not
  decrypt (truncated header or body, a chunk shorter than a tag, a bad tag)
  closes the connection with `4001`. So does a fragment that breaks the rules:
  a fragment message on a connection without the capability, an ordinary
  message in the middle of a run, a bad flag, a piece of the wrong size, or a
  run that passes 64 MiB, or 64 KiB before authentication (which, with the
  piece sizes, is what ends a run that never ends). The secure channel failing
  is the same class of failure as never having established it.
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
  messages in 10 s. That rate limit counts what the *device* sends, so the
  host's fragments of a large answer cost it nothing. It is also why a client
  sends a frame whole whenever it fits one message: fragmenting a 1.4 MiB
  attachment chunk would spend six messages of the budget instead of one. A
  frame too big for one message would spend one per 256 KiB (a 25 MiB frame is
  the whole budget); no client sends one today, since attachments travel in
  chunked ops (see "Attachments").
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
  inside a DATA frame. DATA carries WebSocket messages, not protocol frames: a
  fragmented frame is several DATA frames, and the relay neither knows nor
  needs to. A larger DATA frame for a live connId costs only that
  device: the relay drops the frame, closes the device link with `1009` and
  sends the host the matching CLOSE. Only an oversized message that names no
  device (unknown connId, NOTIFY, too short to decode, any frame before
  `ready`) closes the host link itself with `1009`, which drops every device
  with `4404` as any host departure does. The host serves each virtual connection through the
  same code path as a LAN socket; WebSocket ping/pong is per hop (host↔relay
  and relay↔device), never forwarded, and the host answers its own liveness
  pings to a virtual connection locally. The relay originates no pings of its
  own (they would keep the Durable Object awake); the host pings the relay,
  and the runtime answers device pings without waking the object.
- **Host side.** The Mac keeps the host link up whenever remote access is
  enabled and a relay URL is set, reconnecting with backoff (1 s … 15 s) when
  it drops, `4409` included: two Macs sharing one host key is a
  misconfiguration, and the alternating link surfaces it in `relay.error`
  rather than silently picking a winner. `remote_status.relay` reports the
  link (see host commands). The host hashes the nonce it is given whatever its
  length (the relay always sends 32 bytes), ignores frames for a connId it
  does not know, and closes a single virtual connection with `1008` if that
  device outruns the host's inbound queue for it. The host keeps the link fair:
  each virtual connection may have at most 4 messages waiting in the link's
  outbound queue, so a device's fragmented answer goes out a few fragments at a
  time and another device's frame waits behind those, not behind the whole
  run. Disabling
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

The phone cannot keep a socket open in the background, so the out-of-app
signals the desktop already raises — a turn finishing, an agent waiting on a
tool-use approval — and the ship loop's — checks settling, a review comment, a
PR merging or closing, autopilot giving up — reach it as APNs alerts. Content stays minimal: a fixed
title and the agent's name, at most joined by a check name, a reviewer's login
or a PR number. Transcript text never leaves the Mac.

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
  turn ends. Both mirror `signalAway` in `src/store/eventListeners.ts`.
  The ship-loop kinds read the host-side PR watcher's events (see "Events",
  `pr:checks_changed` / `pr:threads_changed`, and `pr:set_entry_changed` for
  the checkout's other PRs), so they fire with the Mac's window shut. `checks_settled`: an open PR's CI rollup lands on `passing`
  or `failing` from anything else — `pending`, `none`, unseen, or the other of
  the two; a `failing → failing` with a different set of failing names is not
  a second alert. Title `Checks passed` or `Checks failed`; body the agent's
  name, and for a failure ` · ` plus the first failing check's name when there
  is one. `review_comment`: the focused PR's unresolved review threads gained one the
  watcher had not seen (first observation of a PR seeds and never alerts, so a
  host restart does not re-raise old threads). Title `New review comment`;
  body the agent's name, ` · ` plus the new thread's author when known.
  `pr_merged` / `pr_closed`: a `pr:state_changed` (or `pr:set_entry_changed`)
  whose state is `merged` / `closed` for a PR (of a primary or secondary repo) this host process had
  seen `open` — the event reports a state, not a transition, so a
  first event of `merged` after a cold start is a stale snapshot and alerts
  nobody, and becoming `open` never alerts. Title `PR merged` / `PR closed`;
  body the agent's name ` · #<number>`. Two repos of one agent merging are two
  alerts under the one agent `collapseId`, so the later replaces the earlier. `autopilot_gave_up`: the host's autopilot gave
  up on a rung (an `autopilot:event` with outcome `give-up`, see "Autopilot").
  Title `Autopilot gave up · <rung>` (`fix-checks`, `resolve`, `update-branch`,
  `resolve-comments`); body the agent's name ` · ` the reason (`budget spent`,
  `no progress`, `no evidence`).
  The host skips every trigger while its own main window has focus (the user
  is at the Mac); otherwise it sends to every device with a token, and iOS
  itself hides the banner when the app is in the foreground. Every alert's
  `collapseId` is the agent id, so a later alert for the same agent replaces
  the banner rather than stacking.
  Opt-outs are host settings, both default on, only `"false"` disables:
  `notify_turn_complete` (`turn_complete`) and `notify_pr_activity` (the four
  ship-loop kinds and `autopilot_gave_up` under one switch); `needs_input` is
  always sent. The desktop
  writes them with the Tauri commands `set_notify_turn_complete` and
  `set_notify_pr_activity`, both `{ enabled }`.
- **NOTIFY frame (host → relay).** Mux type `0x05`, `connId` 0, payload UTF-8 JSON:

  ```json
  { "tokens": [{ "token": "<hex>", "environment": "sandbox" }], "title": "Turn complete", "body": "Fix login crash", "kind": "turn_complete", "agentId": "…", "collapseId": "<agentId>" }
  ```

  `kind` is one of `turn_complete`, `needs_input`, `checks_settled`,
  `review_comment`, `pr_merged`, `pr_closed`, `autopilot_gave_up`; the same
  payload shape for all seven, e.g. `{ …, "title": "Checks failed", "body": "Fix login crash · unit", "kind": "checks_settled", … }`.
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
  key, the value the phone stores as `hostKey`), otherwise Home — on the Ship
  tab for the four ship-loop kinds and `autopilot_gave_up`, the chat for the
  rest; the plugin
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
public key, so it cannot be guessed or enumerated. A handshake that succeeds,
here or on the LAN, buys a stranger little more: until `pair` or `hello` authenticates it, the host
holds at most 64 KiB of fragment run for it, not 64 MiB. A hostile relay
operator can deny service and nothing more.

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
| `observe` | every read: the workspace, transcripts, diffs, PR state, the workflow and roadmap boards, `gh_status`, `list_dir`, `dictation_status`, `host_providers`, `scan_usage_transcripts`, `approvals_list`, `get_settings`, `get_project_settings` |
| `agents` | spawn, message, answer a tool-use prompt, stop/resume/archive/restore/discard, set model and effort, dictation capture, attachment upload, and the working-tree moves that never leave the machine (`commit_agent`, `pull_agent`, `rebase_agent`, `stash_agent`, `discard_agent_changes`, `abort_merge_agent`, `clear_checkout_config`), and choosing which of a checkout's PRs is focused (`set_focused_pr`) |
| `projects` | add, clone, create, rename, relocate, label, attach/detach and delete projects and their repos; every project setting (`set_project_setting`); and the host's own configuration — alerts, the idle sweep, code indexing, the sandbox engine and its launch knobs, provider binary overrides |
| `workflows` | launch, cancel, resume, retry, approve, reject and delete runs; save, delete and import stored definitions |
| `roadmap` | create, edit, rank, hand off, hold, release, reject, reopen and delete items; accept or reject the PM's proposals |
| `publish` | the ops that leave this machine under the user's name: `push_agent`, `create_pr`, `merge_pr`, `roadmap_merge_item_pr`, `answer_publish_approval`, `delegate_git` — a delegation's own pushes and PRs are approved by the host without a prompt (see "Delegations"), so starting one is as much a publish as answering the prompt would be — and `autopilot_set`, for the same reason: an enrolled checkout's pushes skip the prompt (see "Autopilot"), so switching autopilot on is a standing publish grant; plus the five settings that decide how a publish happens: `set_publish_confirmation`, `set_publish_approval_wait`, `set_branch_prefix`, `set_draft_prs`, `set_agent_attribution_removed` |

Every op in the table below has exactly one scope. `register_push` is outside
the scheme and always allowed: it writes the calling device's own APNs token
and grants it nothing over the host.

Two presets are offered at pairing:

- **`full`** — every scope. Today's surface, and the default when no preset is
  named.
- **`control`** — every scope except `publish`. Watch and steer agents, approve
  tool use, add projects, drive workflows and the roadmap; but no push, no PR
  opened or merged, no publish approved, and no change to the publishing
  settings — a device that may not publish may not switch off the approval
  gate either.

Because `protocol.ops` is already narrowed to the device's scopes, a client
needs no scope-specific gating: the actions it hides for "this host does not
have that op" are the same actions it hides for "this device may not". The
`forbidden` error is the backstop, for a client that asks anyway.

**Changing a device's scopes is a revoke and a re-pair.** There is no in-place
edit. A re-pair on a key already on file replaces that record's scopes with the
new code's, and if that *changed* the set the host closes that device's live
connections with `1012` — the device is still paired, so the client reconnects
on its normal backoff and `hello` hands it the new `protocol.ops`. A re-pair
that grants the same scopes closes nothing.

**The stored record is the only authority.** Every request is authorized
against `devices.json` as it stands at that moment, not against a set captured
when the connection authenticated, so a device narrowed mid-connection cannot
go on using the access it no longer has. `protocol.ops` is a copy handed out at
`pair`/`hello`; the close above is what keeps a client from holding a stale one.

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
- **Transport capabilities ride in the handshake, not in `protocol`.** They
  change how bytes are framed, so both ends have to know before the first
  frame — `protocol` arrives inside one. An end from before capabilities sends
  empty payloads and ignores the ones it receives, so it never gets a fragment
  and is never expected to read one (see "Secure channel").
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
| `approvals_list` | `{}` — the publish approvals still waiting, oldest first, for a client that connected after the event fired | `PendingApproval[]` (the `publish:approval-requested` payload plus `requested_at`) |
| `stop_agent` | `{ agentId }` | `null` |
| `resume_agent` | `{ agentId }` | `null` |
| `archive_agent` | `{ agentId }` | `null` |
| `restore_agent` | `{ agentId }` | `null` |
| `discard_agent` | `{ agentId }` — destructive: record, checkout and transcript all go | `null` |
| `set_agent_model` | `{ agentId, model }` | `null` |
| `set_agent_effort` | `{ agentId, effort }` | `null` |
| `read_session_records` | `{ agentId }` — the agent's display history: what its session inherits through lineage (a fork's parent conversation, `inherited: true`), then its own records | `SessionRecord[]` |
| `read_session_page` | `{ agentId, before?: string \| null, limit?: number }` — the same history one page at a time, newest page first: the last `limit` records before the cursor `before` (from the end when absent), crossing into inherited history exactly as `read_session_records` does. `limit` defaults to 200 and is clamped to 1–500. `records` are in display order (oldest first within the page); `older` is the cursor for the page before this one, `null` once nothing older is left. The cursor is opaque (`"<chainIndex>:<seq>"`, parsed strictly — a malformed or out-of-range one is an error) and belongs to the history it was read from: after a turn ends, start again from the newest page. Lets a phone open a long session without shipping every tool result it ever read. No desktop command of this name | `{ records: SessionRecord[], older: string \| null }` |
| `read_user_turns` | `{ agentId }` — the user turns of the same history, in the same order | `UserTurn[]` |
| `sync_session` | `{ agentId }` | `null` |
| `read_live_turn` | `{ agentId }` — the `event` payloads of the agent's current turn, oldest first, as they were forwarded on `agent:event`; `dropped` counts events cut from the head when the turn outgrew the host's buffer; `next_seq` is the `seq` the agent's next `agent:event` will carry, so a frame with `seq >= next_seq` is one the snapshot does not hold. Empty for a turn that ran under a previous host process or in the native view. No desktop command of this name yet | `{ events: object[], dropped: number, next_seq: number }` |
| `get_git_state` | `{ agentId }` — a checkout whose config Fletch refuses to run git over comes back as a zero-state with the keys in `blocked_config` | `GitState \| null` |
| `get_all_shortstats` | `{}` — uncommitted working-tree stats for every live agent; archived and still-cloning agents are omitted | `Record<agentId, ShortStats>` |
| `get_all_git_meta` | `{}` — advisory local-git metadata per checkout (base staleness, changed paths), keyed like the PR maps (`agentId` for the primary repo, `"{agentId}::{subdir}"` for secondaries); no network. Staleness is measured against the source repo's `origin/<base>`, which the host itself fetches every five minutes when it has a GitHub credential — no client asks for that fetch | `Record<gitKey, GitMeta>` |
| `get_all_pr_status` | `{ reverifyClosed?: boolean }` — every live agent-repo's PRs, keyed like `get_all_git_meta`. `AgentPrStatus` is `{ state: PrState, checks: PrChecks \| null, prs: { state, checks }[] }`: `state`/`checks` are the checkout's *focused* PR (the one it is bound to, which is all a single-PR client reads) and `prs` is the checkout's whole PR set, the focused PR included, newest number first (absent from a host that predates PR sets — read it as `[{ state, checks }]`). `checks: null` means "nothing to say this round" so a client keeps its last value; merged PRs are served from the snapshot, closed ones re-verified live only when `reverifyClosed`. Every open PR of every checkout is read in the same batched query. A seed, not a poll: read it after a handshake (and on returning to the foreground), then follow `pr:state_changed` / `pr:checks_changed` (the focused PR) and `pr:set_entry_changed` (the rest of `prs`), which the host's PR watcher emits on every change it sees in its once-a-minute sweep of the same resolver | `Record<gitKey, AgentPrStatus>` |
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
| `clear_checkout_config` | as command — unsets the config keys a `GitState.blocked_config` names, in the checkout's own `.git/config`; fails naming any key it could not reach | `null` |
| `create_pr` | as command | `PrState` |
| `merge_pr` | as command — merges the open PR on the targeted repo's branch | `null` |
| `set_focused_pr` | `{ agentId, subdir?, number }` — makes `number`, which must already be one of the checkout's PRs (`prs` above), its focused PR: the one `state`/`checks`, the badge and the focused reads follow. Local only, no GitHub call; emits `pr:state_changed` for it (the event is the focused PR's, so a client reads it as the focus moving) | `PrState` |
| `get_pr_state` | as command | `PrState \| null` |
| `get_pr_checks` | as command | `PrChecks \| null` |
| `get_pr_live` | as command — read once when a screen showing the PR opens with nothing cached for it; `pr:state_changed` / `pr:checks_changed` keep it current after that | `PrLive \| null` |
| `get_pr_threads` | as command — unresolved review threads (GraphQL). Read once like `get_pr_live`; `pr:threads_changed` keeps it current | `PrComments \| null` |
| `delegate_git` | `{ agentId, subdir?, action, params? }` — hands a git playbook to the agent (see "Delegations"). `action` is a playbook name (`commit`, `commit-push`, `commit-pr`, `open-pr`, `push`, `resolve-conflicts`, `update-branch`, `fix-checks`, `resolve-comments`); anything else is refused. `params` is a `{ [key]: string }` of the playbook's dynamic context (`base`, `failing`); empty values are dropped, `repo` is the host's to set from `subdir`, and so is `branch` — the focused PR's head branch while it is open, added for the playbooks that work on that PR (`update-branch`, `fix-checks`, `resolve-comments`) so an agent on another branch switches first. The host composes the `[app-action]` trigger, records the delegation and either sends it now as a user turn (`turn:sent` fires as for any send) or, while the agent is `running`, holds it until the agent goes idle | `Delegation` |
| `get_delegations` | `{}` — every delegation the host is tracking, for a client that connected mid-flight | `Delegation[]` |
| `autopilot_state` | `{ agentId? }` — autopilot as the host runs it (see "Autopilot"): one row per checkout of `agentId`, or of every live agent when it is absent, plus the two opt-out lists | `AutopilotSnapshot` |
| `autopilot_set` | `{ projectId, enabled }` or `{ agentId, enabled }` — exactly one of the two ids. On a project it is the project's switch (default on); on an agent `enabled: false` pauses that agent's checkouts and `true` resumes them. Persists the host setting, takes effect before the call returns, and emits `autopilot:switches` (always, first) then `autopilot:state` for every checkout it changed | `AutopilotSnapshot` (the whole host's, as `autopilot_state {}` would answer) |
| `autopilot_log` | `{ agentId?, subdir? }` — what autopilot did, newest first: one agent's checkouts (only the one named when `subdir` is given; the primary's own subdir names the primary), or every agent's when `agentId` is absent. At most the newest 50 rows per checkout are kept | `AutopilotLogEntry[]` |
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
| `scan_usage_transcripts` | `{ sinceMs, untilMs }` — token counts read off *this host's* Claude Code and Codex transcripts over the half-open window, bucketed by local hour, provider and model, plus a span per contributing session. Uncached and slow (seconds over 90 days): ask for the widest window once and slice shorter ranges out of the answer. Read-only; the hours are the host's local hours and the ids are unique within the host only, so a client aggregating several hosts must tag each answer with the host it came from | `UsageScan` — `{ buckets: { hourStartMs, provider, model, tokens: { input, output, cacheRead, cacheWrite }, requests }[], sessions: { provider, id, firstMs, lastMs }[], scannedFiles, filesRead, bytesRead, sinceMs, untilMs }` |
| `get_settings` | `{}` — the host-owned global settings, and only those (see "Settings"): absent keys are unset and read as their default. No secret is ever in the answer | `Record<key, string>` |
| `set_notify_turn_complete` | `{ enabled: boolean }` — `notify_turn_complete`: a finished turn alerts at all (chime, banner, phone push) | `null` |
| `set_notify_pr_activity` | `{ enabled: boolean }` — `notify_pr_activity`: the ship-loop alerts (checks settled, review comment, PR merged or closed) | `null` |
| `set_auto_archive_idle_days` | `{ days: number }` — `auto_archive_idle_days`; `0` turns the idle sweep off | `null` |
| `set_code_indexing_enabled` | `{ enabled: boolean }` — `code_indexing_enabled`; turning it on also installs codegraph and warms the index for every pinned repo, in the background on the host | `null` |
| `set_context_layer_enabled` | `{ enabled: boolean }` — `context_layer_enabled`: the project context layer's developer gate; off, the layer's ops, instruction block, ingesters and extractor are inert and the project page shows no Context tab | `null` |
| `set_sandbox_engine` | `{ engine: "sandbox-exec" \| "docker" \| "podman" }` — `sandbox_engine` for *new* agents; a container engine is probed live on the host first and refused when its runtime is unreachable or cannot launch | `null` |
| `set_docker_launch_settings` | `{ image: string \| null, memory: string \| null, cpus: string \| null }` — `docker_image` / `docker_memory` / `docker_cpus`, written together; blank or `null` clears one back to the launch default | `null` |
| `set_podman_launch_settings` | `{ image, memory, cpus }` — the podman twin over `podman_image` / `podman_memory` / `podman_cpus` | `null` |
| `set_agent_bin_override` | `{ id, path: string \| null }` — `agent_bin_path_<id>`, a path *on the host*; blank or `null` clears it. Live agents on that provider are respawned so they exec the new binary | `null` |
| `set_branch_prefix` | `{ prefix }` — `git_branch_prefix`; validated and trimmed on the host, empty clears it | `string` (the prefix as stored) |
| `set_draft_prs` | `{ enabled: boolean }` — `github_draft_prs` | `null` |
| `set_publish_confirmation` | `{ enabled: boolean }` — `publish_confirmation`: an agent's own push or PR waits for `answer_publish_approval` | `null` |
| `set_publish_approval_wait` | `{ secs: number }` — `publish_approval_wait`; `0` waits until answered | `null` |
| `set_agent_attribution_removed` | `{ removed: boolean }` — `agent_attribution_removed`: strip agents' co-author trailers and "Generated with" lines from the next spawn or resume on | `null` |
| `get_project_settings` | `{ projectId }` — the project's client-writable settings, and only those (see "Settings"); absent keys read as their default | `Record<key, string>` |
| `set_project_setting` | `{ projectId, key, value: string \| null }` — writes one key, `null` deletes the row (back to the default). A key outside the project allowlist is refused | `null` |
| `context_overview` | `{ projectId }` — the project's whole context in one read: the two toggles as the host reads them, the graph (entities, assertions, relations), the *pending* proposals and the stats (see "Project context") | `ContextOverview` |
| `context_preview` | `{ projectId, query: CompileQuery }` — what an agent would be served for `query`, rendered as markdown; the roadmap brief stands in for a vision nobody has recorded | `string` |
| `context_record_entity` | `{ projectId, input: EntityInput }` — creates, or records a revision when `input.id` is set. Stamped user / UI | `string` (the entity id) |
| `context_record_assertion` | `{ projectId, input: AssertionInput }` — always lands `confirmed` (the user saying it is the confirmation); `input.supersedes` with reasoning is how a decision is changed | `string` (the assertion id) |
| `context_retract` | `{ projectId, assertionId, reason }` — hides an assertion that was never right; a blank reason is refused | `null` |
| `context_archive_entity` | `{ projectId, entityId }` | `null` |
| `context_merge_entities` | `{ projectId, from, into }` — `from`'s edges move to `into`; `from` stays behind as `merged` | `null` |
| `context_link` | `{ projectId, change: { from, to, rel, add } }` — `add: false` unlinks | `null` |
| `context_rule_proposal` | `{ projectId, proposalId, verdict: "accept" \| "dismiss", dismissReason?: "wrong" \| "trivial" \| "duplicate" \| "already_known" }` — accepting lands the proposal as events; dismissing needs a reason | `string \| null` (the recorded id on accept) |
| `context_resolve_contradiction` | `{ projectId, a, b, reasoning }` — closes the `contradicts` edge between assertions `a` and `b` with a ruling; neither side changes (retract or supersede one for that). Blank reasoning is refused | `null` |
| `context_bootstrap` | `{ projectId }` — records a `module` entity (path-anchored, `part_of` its parent) for every package and code directory of the project's primary repo at `HEAD` whose slug the project has never had; a second run writes nothing. Stamped ingester / `repo` with the commit | `ContextBootstrap` (`{ commit, modules, created, mapping_task }`) |
| `register_push` | `{ token: string \| null, environment?: "sandbox" \| "production" }` — `environment` required with a token, ignored on clear (remote-only, see "Push notifications") | `null` |

Never exposed, by design: the generic `db_*` table bridge, every file mutation
(`write_checkout_file`, `rename_*`, `delete_*`, `create_*`, `copy_*`), shell
ops (`open_agent_shell`, `write_to_shell`, …), `write_to_agent` (raw PTY),
editor/log/telemetry ops, the provider install/login ops (`install_agent`,
`open_provider_login`) and the two desktop provider probes behind Settings ›
Providers (`probe_provider_versions`, `probe_provider_auth`, which answer with
resolved binary paths), and the `run_*` family (`run_start`, `run_stop`,
`run_verification` — this machine's own scripts). Autopilot's `fix-checks`
verdict needs no `run_verification` on the wire: the ladder runs on the host,
which calls its own verifier and reports it as `verify:report`. `host_providers` above is the
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

Not yet exposed, but only for want of a reason to be: `fork_agent`, the rewind
ops (`rewind_agent`, `preview_rewind_code`, `undo_code_restore`,
`discard_code_undo`, `has_code_undo`), and the two
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
`list_repo_prs`) and `run_verification` — and a client gates them by name like
any other absent op.

Two things a client should know before driving them. `wf_launch`'s
`attachments` are paths on the *host*, so upload with `attachment_*` first and
pass what `attachment_end` answered, exactly as for `send_user_message`. And
the host has two autonomous loops: the roadmap *queue*, driven by the board ops
above — holding a project or an item is how a client stops it — and autopilot
(see "Autopilot"), which a client steers with `autopilot_set`.

## Settings

A setting the *host* reads — its loops, its spawn path, its alerts — is the
host's, and a client changes it through an op, never through a table. The
generic `db_*` bridge stays off the wire, and a client never writes one of these
keys into its own database to mean "the host's".

**Global settings.** `get_settings` answers the host-owned keys and nothing
else, as raw strings (an absent key is unset; the client applies the same
default the host does):

| key | written by | scope |
|---|---|---|
| `notify_turn_complete` | `set_notify_turn_complete` | `projects` |
| `notify_pr_activity` | `set_notify_pr_activity` | `projects` |
| `auto_archive_idle_days` | `set_auto_archive_idle_days` | `projects` |
| `code_indexing_enabled` | `set_code_indexing_enabled` | `projects` |
| `context_layer_enabled` | `set_context_layer_enabled` | `projects` |
| `sandbox_engine` | `set_sandbox_engine` | `projects` |
| `docker_image`, `docker_memory`, `docker_cpus` | `set_docker_launch_settings` | `projects` |
| `podman_image`, `podman_memory`, `podman_cpus` | `set_podman_launch_settings` | `projects` |
| `agent_bin_path_<id>` | `set_agent_bin_override` | `projects` |
| `git_branch_prefix` | `set_branch_prefix` | `publish` |
| `github_draft_prs` | `set_draft_prs` | `publish` |
| `publish_confirmation` | `set_publish_confirmation` | `publish` |
| `publish_approval_wait` | `set_publish_approval_wait` | `publish` |
| `agent_attribution_removed` | `set_agent_attribution_removed` | `publish` |

Each setter is the desktop command of the same name, so it does what the
desktop's Settings pane does: it persists the key *and* updates the in-memory
mirror the host's spawn and publish paths read, so a change applies without a
restart. Nothing secret is on the list and nothing secret is ever answered —
`github_token`, `linear_token`, `claude_container_token`, the `remote.*` keys,
`telemetry_*` and every client-side preference (theme, pane widths, shortcuts,
dictation) stay where they are, off the wire.

The scope split follows the line `publish` already draws. The five settings
that decide whether and how an agent's work leaves the machine — the approval
gate and its wait, the branch prefix, draft PRs, attribution — are `publish`,
so a `control` device cannot switch the approval gate off on its way to a
publish it is not allowed to approve. Everything else is the host's own
configuration and sits under `projects`, the scope that already rewrites what
the host holds; note that it includes the sandbox engine and the provider
binary paths, so a device paired without `projects` cannot change how the host
runs agents either.

**Project settings.** `get_project_settings` / `set_project_setting` address
one project's rows through an allowlist the host enforces on both sides — the
read answers only these keys, the write refuses any other with an error naming
it:

| key | read by | value |
|---|---|---|
| `verify.on_turn_end` | the turn-end verifier | `"1"` on; absent off |
| `run.<row>` (`run.install`, `run.test`, `run.lint`, `run.dev`, …) | the Run panel, the verifier, workflow checks | the command |
| `run.agent.<agentId>.<row>` | the same, for one agent, over the project's | the command |
| `run_env` | the Run panel's env membrane | the `RunEnvDoc` JSON; values never live here |
| `workflow.default`, `composer.mode` | the roadmap queue, the composer | a definition id; `"agent"` \| `"workflow"` |
| `roadmap.autoqueue` | the roadmap queue | `"1"` on; absent off |
| `roadmap.max_concurrent` | the roadmap queue | an integer; absent is one |
| `roadmap.settle_review`, `roadmap.midrun_awareness` | the PM's review | `"0"` off; absent on |
| `roadmap.declined_issues` | the issue funnel | JSON array of issue URLs |
| `linear.team_id`, `linear.team_name` | the issue inbox and picker | a Linear team id; its display name |
| `context.enabled`, `context.extract` | the context layer (its ops, instruction block, ingesters) and its background extractor | `"false"` off; absent on |

Every `run.` key is allowed, which is what lets the per-agent overrides through.
The roadmap's code allocator (`roadmap.code_prefix`, `roadmap.code_seq`) is the
host's alone and is neither answered nor writable; autopilot's switch is the
desktop's own loop and has no row here.

**`null` deletes.** `set_project_setting` with `value: null` removes the row,
and an absent row is how every key above spells its default — so "set back to
the default" and "never set" are the same state, and a later change of default
reaches every project that never chose otherwise. An empty string is stored as
given.

**Live.** Every write emits one event per key it wrote, forwarded like any
other: `settings:changed { key, value }` and
`project_settings:changed { project_id, key, value }`, where `value` is the
stored string or `null` for a deleted row. A client applies it over what it read
and needs no refetch; a second desktop's Settings pane and project page follow
the first's edits this way. A client still reads the whole set again on every
handshake, since an event only reaches the clients connected when it fired.

## Project context

The context layer (`crates/fletch-core/src/context`) is what a project is made
of and what has been decided about it, served to agents so they do not
re-explore the project to reconstruct it. The `context_*` ops are the human
side of it — the project page's Context tab — and every op addresses the
project by its host-local `projects.id`; the host resolves the project's
context id itself. The reads are `observe`; every write is `projects`, since
it rewrites what the host knows.

`context_overview` is the one read the tab needs: `{ enabled, extract, graph:
{ project_id, entities, assertions, relations }, proposals, stats }`, where
`proposals` is the pending review queue only. Every type is the serde form of
the Rust model (`context::model`, snake_case enums). Writes are stamped user /
UI (except `context_bootstrap`, which records what the repository says as the
ingester); an assertion recorded here is always `confirmed`, and changing one is a
new assertion with `supersedes: { id, reasoning }` rather than an edit —
assertions are immutable. After every write the host emits
`context:changed { project_id }` and the tab reloads the overview.

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
agent:spawn-progress
agent:title            agent:branch           agent:model
agent:effort
agent:repo_added       agent:git-action       session:records-appended
turn:sent              turn:started           workspace:changed
workspace:auto-archived
pr:state_changed       pr:checks_changed      pr:threads_changed
pr:set_entry_changed
verify:report          publish:approval-requested
publish:approval-resolved
delegation:changed
autopilot:state        autopilot:event        autopilot:switches
wf:event               wf:run                 wf:run-deleted
roadmap:item           roadmap:item-deleted   roadmap:item-event
roadmap:proposal       roadmap:proposal-deleted
roadmap:order-proposal roadmap:order-proposal-deleted
roadmap:project-hold   roadmap:project-hold-released
roadmap:brief          roadmap:brief-proposal roadmap:brief-proposal-deleted
roadmap:queue-note
settings:changed       project_settings:changed
context:changed
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
`agent:spawn-progress` (`agent_id`, `stage`, `detail`) names the step a fresh
spawn is on — `preparing`, `cloning`, `indexing`, `carrying`,
`attaching_repos`, `starting` — so the client can label the wait behind
`spawning` instead of showing a bare spinner. Progress only, and not
snapshotted anywhere: the authoritative state is `agent:status`, a client that
connects mid-spawn sees `spawning` with no stage until the next one fires, and
one that does not know a `stage` value ignores it. The host emits no stage once
the spawn has reached its terminal status — a spawn that times out stops at its
next stage boundary rather than narrating work the `error` status already
ended, and a stage always belongs to the attempt that started it, so resuming
the agent afterwards never revives the abandoned attempt's stages — but a
client must still tolerate a stage that was in transit when the
terminal status overtook it: treat `agent:status` as authoritative and let the
stage label go stale, never the other way round.
`publish:approval-requested` is a held prompt like a tool-use approval: the
agent's publish is blocked on the host until someone answers with
`answer_publish_approval` (or the host's wait lapses and refuses it). A client
that does not have that op on `protocol.ops` can show the prompt but not answer
it. An event only reaches the clients that were connected when it fired, so a
client asks `approvals_list` after every handshake and replaces its pending set
with the answer — that is how a phone opened after the agent asked still sees
the prompt, and how one whose card was answered elsewhere stops showing it.

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

`pr:state_changed` `{ agent_id, subdir?: string | null, state: PrState | null }`
is one checkout's focused PR as found now — a state, not a transition; `null`
is "no bound PR". It fires after a push or a turn end (primary repo), from
`set_focused_pr`, and from the host-side PR watcher below, for the primary and
every secondary repo. `subdir` is `null` (or absent, from a host that predates
it) for the agent's primary repo and the repo's subdir for a secondary, so a
client keys the write by checkout as it does `get_all_pr_status`; a client that
keeps only the primary's PR ignores an event with `subdir` set rather than
writing it to the primary. A checkout holds a set of PRs (`prs` in
`get_all_pr_status`), but this event only ever describes its *focused* one —
the PR a client from before PR sets holds as the checkout's only PR — so a
client never has to tell PRs apart to apply it. One whose `state.number`
differs from the focused PR a client holds is the focus moving (a
`set_focused_pr`, or the host binding a PR just opened). The same state may be
reported again — the watcher re-reports every open PR on its first look after a
host restart — so a client derives "opened" / "merged" from the record it
replaces and treats a same-state event as a refresh.

`pr:checks_changed` `{ agent_id, subdir: string | null, number, checks: PrChecks }` and
`pr:threads_changed` `{ agent_id, subdir, comments: PrComments, new_thread_ids: string[] }`
come from the host-side PR watcher, which runs the sidebar's batched sweep
(`get_all_pr_status`'s resolver — state and CI for every open PR of every
checkout, one GraphQL query per 50 PRs) once a minute, folds each checkout's
focused open PR's review threads into that same query every other tick (only
those aliases select them), and emits only on a change: the
first fires when the normalized `rollup` (`none | pending | passing |
failing`), the set of failing check names or the merge gate (`merge_state`,
ignoring GitHub's transient `unknown`) moves, the second whenever the
unresolved thread set changes, naming in `new_thread_ids` the ids the watcher
had not seen beside the whole current set — an empty `new_thread_ids` is a
thread resolved, or the first threads read of a PR that just became focused,
which a client applies and does not announce. `subdir` is `null` for the
agent's primary repo and the repo's subdir for a secondary; `number` is the
PR. Like `pr:state_changed`, both describe the checkout's focused PR only.
The watcher keeps its memory per PR, so each of a checkout's PRs is watched on
its own. Its first read of an open PR seeds its memory and emits only its
state — so a PR reopened or opened outside Fletch reaches every client — and no
checks or threads, so a host restart announces no old thread; a PR that leaves
`open` gets one final state event and is then forgotten. A client treats both
as the freshest copy of the PR's checks and comments and may append an
activity line for them. They are what keeps a client current: clients read
`get_all_pr_status` after a handshake and `get_pr_live` / `get_pr_threads` once
for a PR on screen with nothing cached, and do not poll them — so GitHub is
read by the watcher alone.

`pr:set_entry_changed` `{ agent_id, subdir: string | null, entry: { state: PrState, checks: PrChecks | null } }`
is the same watcher's report on a PR of the checkout's set that is *not* its
focused one: `entry` is that PR's `prs` element (see `get_all_pr_status`),
emitted under the same rules — its first open read, a state change, or its
checks moving. A state change carries `checks: null` ("nothing to say this
round": keep the last); a checks change carries the PR's state beside its new
checks. Its threads are never read, so nothing reports them. The three events
above stay the focused PR's alone, with the payloads they had before PR sets,
because a client that predates PR sets reads each of them as its checkout's
only PR; this event is a name such a client does not know, and an unknown
event name is ignored by design ("Compatibility"), which is what makes it
additive. A client that keeps the whole set upserts `entry` into it and never
writes it over the focused PR; one that keeps a single PR per checkout ignores
it. A host lists it in `protocol.events`, so a client can tell a host that
reports the rest of the set from one that predates PR sets (whose
`pr:state_changed` is the checkout's whole set). These four are the only
`pr:*` events.

`workspace:auto-archived` `{ agent_ids: string[], names: string[] }` is the
idle sweep's notice: one per pass that archived anything, naming the agents by
id and display name (nothing from their transcripts), so a client can say what
moved to History. `workspace:changed` fires alongside it and already reloads
the list; this one only explains the change, and a client may ignore it.

`delegation:changed` `{ agent_id, subdir: string | null, kind, phase, started_at, notice? }`
is one step of a delegation's life (see "Delegations"). `kind` is the
delegation kind (`commit`, `commit-push`, `commit-pr`, `open-pr`, `push`,
`resolve`, `update-branch`, `fix-checks`, `resolve-comments` — the playbook name,
except `resolve-conflicts`, whose kind is `resolve`). `phase` is `queued` (held
behind the agent's running turn), `started` (the trigger was delivered),
`running` (the delegated turn is under way), `done` or `abandoned`. The last two
end the delegation and carry a one-line `notice` for the client to show
("Committed & pushed", "Agent finished — review the chat for details"); an
`abandoned` with no `notice` is a delegation dropped because its agent went away.
`started_at` is epoch milliseconds, reset when a held trigger is delivered. A
client keeps the live ones keyed by `(agent_id, subdir)`, drops the entry on
`done`/`abandoned`, and replaces its set with `get_delegations` after every
handshake — an event reaches only the clients connected when it fired.

`autopilot:state` is one `AutopilotCheckout` (see "Autopilot") whenever a
checkout's enrollment, pause, project switch or cycle changes; a client replaces
its row for `(agent_id, subdir)` with it. `autopilot:switches` is `{
disabled_projects: string[], paused_agents: string[] }`, the whole of both
opt-out lists, fired by every successful `autopilot_set` — including a project
switch on a project with no agents, which changes no row — before that call's
`autopilot:state` rows; a client replaces both lists with it (no merge, so a
repeat or a late one is harmless). `autopilot:event` is one
`AutopilotLogEntry`, the row `autopilot_log` will answer from then on; a client
prepends it to that checkout's history. All three are host facts: a client seeds
them with `autopilot_state {}` and `autopilot_log {}` after every handshake and
treats the events as the deltas.

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

`settings:changed` `{ key, value: string | null }` and
`project_settings:changed` `{ project_id, key, value: string | null }` follow a
write to a host-owned setting, one event per key written, and only ever for a
key on the allowlists in "Settings" — so a secret cannot ride one. `value` is
the stored string, `null` for a deleted row.

`context:changed { project_id }` follows every `context_*` write (and any other
write to the project's context on the host). It carries no row: a client
re-reads `context_overview`, which is small by design.

Never forwarded: `agent:output`, `shell:output`, `run:output` (raw PTY bytes),
the rest of `run:*`, `dictation:*`, `docker:*` and `agent-install:*`.

Delivery is best effort, exactly like the desktop frontend: the phone must
refetch `get_workspace` on reconnect and on returning to the foreground, and
`read_session_records` when it opens an agent — or, on a host that lists it,
the newest `read_session_page`, reading older pages only when the user scrolls
back for them. A list with no records of the agent's own (empty, or only
`inherited` ones) is not proof of an empty conversation — the turn-end ingest
can lag or miss — so the phone then asks the host to `sync_session` and reads
once more, and keeps whatever log it already rendered from live events if that
is still empty. The newest page settles the question as well as the whole list
would: an agent's own records are the newest in its history, so a newest page
with none of them means the session has none.

Records stop at the last *finished* turn: the running one is ingested only when
it ends. A phone that opens a busy agent therefore also asks for
`read_live_turn` and folds those events onto the rebuilt log exactly as it
folds the live `agent:event` stream — so a turn whose frames were lost to a
backgrounded app or a dropped socket is rendered whole, for every provider,
and the frames still to come continue from where the replay left off. A host
without the op (gate on `protocol.ops`) leaves the phone with the live log it
already has, as before.

## Delegations

A delegation hands one git playbook to the coding agent (`delegate_git`): the
agent writes the judgment part (commit message, PR description, conflict
edits, the test fix) and runs the credentialed steps through its own RPC. It is
a host fact, so it runs to its end with every window closed and is the same on
every client. The host:

- **holds the trigger while the agent is mid-turn.** A message written to a
  running turn folds into it instead of running as its own, so a delegation sent
  to a `running` agent is recorded `queued` and delivered when the agent goes
  idle — at most one per agent at a time, so two queued on one agent's two
  checkouts never coalesce either.
- **decides when it is over.** Done when the agent ran an op from the
  delegation's own playbook during the delegated turn (`agent:git-action`) *and*
  that checkout reached the target (clean tree, nothing unpushed, PR open, no
  conflicts, branch mergeable). `fix-checks` and `resolve-comments` cannot be
  observed that way (CI takes minutes, GitHub reports threads on its own
  schedule), so the agent settling is their normal ending and reports `done`.
  Any other kind whose agent settles without reaching its target — after its turn
  ran, or 15 s after delivery if no turn was ever seen — is `abandoned`.
- **pre-approves its own publishes.** While a delivered delegation is live on a
  checkout, a gated `git_push` / `open_pr` there that the playbook performs is
  approved without a `publish:approval-requested` prompt: the user asked for it
  when they started the delegation. An op outside the playbook (a `commit`
  delegation pushing) still prompts.

`Delegation` is `{ agent_id, subdir: string | null, kind, phase: "queued" |
"started" | "running", started_at }` — the `delegation:changed` payload without
a `notice`. The table is in memory: a host restart forgets its delegations,
which then read to a client as simply having ended.

## Autopilot

Autopilot nurses an open PR to mergeable without being asked: failing checks,
a branch behind or conflicting with its base, unaddressed review threads. It
runs on the host, so it keeps going with every window closed and acts once
however many clients are connected; clients render its state and flip its
switches.

- **What it touches.** Every checkout of every live (non-archived) agent whose
  project's switch is on and that is not paused. **On by default**: a project
  is on until switched off, and an agent until paused. Nothing happens on a
  checkout until it has an open PR, and autopilot never commits, pushes or
  opens a PR for work that was not already proposed — those stay the user's.
- **What it does.** Every 10 s the host reads each such checkout (git state;
  PR state and CI; review threads, which are GraphQL and read at most once a
  minute per checkout unless a cycle is waiting on them) and runs the ladder
  the Git panel shows. A rung it drives (`fix-checks`, `resolve`,
  `update-branch`, `resolve-comments`) opens a **cycle**: the host delegates it
  exactly as `delegate_git` would (a `delegation:changed` and a `turn:sent`
  follow), waits for the agent's turn to end, then judges it — `fix-checks` by
  running the project's own verify commands on the host (reported as
  `verify:report`), the others by the PR state alone. Never while the agent is
  mid-turn or another delegation is live on the checkout, and never two
  dispatches to one agent in one pass.
- **When it stops.** Each rung gets a budget per situation (`fix-checks` 3,
  the others 2); a cycle that changes nothing at all, a spent budget, or no CI
  verdict within 15 minutes ends in a `give-up`, after which autopilot waits
  until the situation changes. A give-up is pushed to the phone
  (`autopilot_gave_up`, see "Push notifications").
- **Publishing.** While a checkout is enrolled, its `git_push` is approved
  without a `publish:approval-requested` prompt — nobody is watching to answer
  it. `open_pr` always prompts: every rung works on a PR that already exists.

The switches are host settings, written only through `autopilot_set` (never the
desktop's generic table bridge): `project_settings` key `autopilot.enabled` per
project (`"0"` off, anything else or no row on) and `settings` key
`autopilotPausedAgents` (a JSON array of agent ids). A pause or a switched-off
project takes effect at once: the checkout's cycle is dropped, its pushes
prompt again, and its turn already running is left to finish. Every write is
announced as `autopilot:switches` (both lists, whole) so every client's switches
agree, then as `autopilot:state` for each checkout whose row it changed.

`AutopilotSnapshot` is `{ checkouts: AutopilotCheckout[], disabled_projects:
string[], paused_agents: string[] }`. `AutopilotCheckout` is `{ agent_id,
subdir: string | null, project_id, enrolled, paused, project_enabled, cycle }`,
where `subdir` is `null` for the agent's primary repo, `enrolled` is
`project_enabled && !paused`, and `cycle` is `null` or `{ rung, attempt, phase:
"working" | "awaiting-evidence", since }` — `rung` a delegation kind, `attempt`
1-based, `since` epoch milliseconds the phase began. `AutopilotLogEntry` is
`{ id, agent_id, subdir, at, outcome: "dispatch" | "settle" | "retry" |
"give-up", rung, attempt, reason? }`, `reason` being `budget-spent`,
`no-progress` or `no-evidence` and present only on a `give-up`. Cycles are in
memory, so a host restart drops them (the next tick starts over from the live
PR); the log is durable.

## Errors

Host errors are strings (the `Display` of the Rust `Error`). Four are
reserved: `"unknown op"` for anything off the allowlist, `"forbidden"` for an op
this host has but this *device's* pairing scopes do not reach (see "Scopes"),
`"too many in-flight requests"` for a connection over its concurrency cap, and
`"response too large"` for an answer over the frame cap — 64 MiB on a
connection that negotiated fragmentation, 4 MiB on one that did not (see
"Transport"; the op ran and the connection is fine; the client needs a smaller
read).

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
(service restart) arrives when the host moves its listener to another port, and
when this device re-paired into a different scope set (see "Scopes") — in both
cases the credential is still good, so it is retryable and the client comes
straight back for a fresh `protocol`. A relayed device reconnects without
noticing, since the relay routes on the host key; a LAN-only device still dials
the old port until it learns the new one.

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
