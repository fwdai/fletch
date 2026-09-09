# Fletch Relay

The reference relay for the Fletch remote protocol: a Cloudflare Worker
fronting one Durable Object per host ID. It exists so a phone can reach its Mac
when the two are not on the same network.

It is a dumb pipe. The Noise channel between phone and Mac is end to end, so
everything the relay carries is already ciphertext — it routes by host ID,
verifies one Diffie-Hellman proof to decide who may claim a host ID, and
multiplexes device links onto the single host link. It stores nothing but a
connection-id counter. The contract is `../docs/remote-protocol.md`, section
"Relay"; that document is the source of truth and this code follows it.

## Endpoints

| route | who | behaviour |
|---|---|---|
| `GET /v1/host/<hostId>` | the Mac | challenge / proof / ready, then multiplexed frames |
| `GET /v1/device/<hostId>` | a phone | a plain WebSocket, exactly as the LAN host would serve |

Anything else is `404`, as is a `hostId` that is not 43 base64url characters
decoding to exactly 32 bytes. A valid route without `Upgrade: websocket` is
`426`.

Close codes: `4003` host authentication failed, `4404` host offline, `4409`
host link replaced, `4429` more than 8 device links, `1009` message over
4 MiB, `1008` more than 100 messages in 10 s on one device link.

The relay also sends Apple push notifications on the host's behalf; see
"Push notifications" below.

## Host link authentication

The host ID *is* the host's X25519 public key, so nothing has to be
registered in advance and nothing is stored. On upgrade the relay mints a
throwaway X25519 keypair and a 32-byte nonce and sends

```json
{ "type": "challenge", "nonce": "<base64url>", "relayKey": "<base64url>" }
```

The host answers within 10 s with

```json
{ "type": "proof", "proof": "<base64url SHA-256( shared || nonce || hostKey )>" }
```

where `shared = X25519(hostPrivate, relayKey)` and `hostKey` is the raw 32
bytes of the host ID. The relay recomputes it as
`X25519(relayPrivate, hostKey)`, compares in constant time, and replies
`{ "type": "ready" }` or closes `4003`. X25519 and SHA-256 both come from
WebCrypto (`crypto.subtle`); there is no third-party crypto dependency.

## Test vector

A known-answer vector for that proof, so the desktop's Rust implementation can
be checked against the same numbers. It was produced with Node's `crypto`
(`crypto.diffieHellman` for X25519) and is verified against this Worker's
WebCrypto code by `test/auth.test.ts`. It also lives in `test-vector.json`.
All values are unpadded base64url of raw bytes.

```
hostPrivate   OqU0-7cJg-RJvvrbEO7QdOhyDkXsLgpcep6iAuRBhyM
hostId        CByil-LnR3Z4wv604Br29p6iHZu2cmhqy35dlKs7Ojg   (= X25519 public of hostPrivate)
relayPrivate  X3dswLZDY6e6AswL_bcW6V13_XV_3NnBcXngrkpTn38
relayKey      9ULx3HxJf-CwYQMXrrUbub-mLokcC6O-EkH1Uv0bcR8   (= X25519 public of relayPrivate)
nonce         91SDD6qePWgyP4B77YoYcp5T7H2CVoihopcC_iVs13U
shared        3eRCj5nchCI2rpYP7m4yUk_eYadXHnLE8SIcsIEg82U
proof         hVOvHwh5XlYzm95Ts3hqD2J6OAVe6lIt1wRxGMN30CM
```

`proof = SHA-256( X25519(hostPrivate, relayKey) || nonce || hostId )` over the
raw 32-byte decodings. The private values are unclamped random bytes, which is
what both `crypto.diffieHellman` and `x25519-dalek`'s `StaticSecret::from`
accept.

## Multiplexing

Every device link becomes a numbered virtual connection on the one host link.
Binary messages on the host link are `type (1) || connId (u32 BE) || payload`:
`0x01` OPEN, `0x02` DATA, `0x03` CLOSE (`code (u16 BE) || reason`), `0x04`
TEXT and `0x05` NOTIFY. A device's binary message becomes DATA, its text
message becomes TEXT (verbatim, so the host applies its own `4001` rule), and
a device that goes away becomes CLOSE. Host DATA goes back out as a binary
message to that device; host CLOSE closes it with that code and reason. NOTIFY
names no connection and asks for a push (below). Frames naming an unknown
`connId` are dropped. `connId` is a u32 that is never reused for the life of a
Durable Object; the counter is the object's only stored value.

## Hibernation

The object uses the WebSocket Hibernation API, so it is evicted while its
sockets are idle and rebuilt on the next message — which means no socket
bookkeeping can live in instance fields. `ctx.getWebSockets(tag)` is the only
source of truth for who is attached, and each socket's own attachment
(`serializeAttachment` / `deserializeAttachment`) carries its role, connection
id, authentication stage, challenge keypair and rate-limit window. The 10 s
proof deadline is a storage alarm rather than a `setTimeout`, for the same
reason.

## Push notifications

The phone cannot hold a socket open in the background, so the Mac asks the
relay to send an Apple push instead: a NOTIFY frame (`0x05`, `connId` 0) whose
payload is UTF-8 JSON naming up to 8 device tokens plus a title, body, kind
and agent id. The relay signs Apple's provider JWT — the host never sees the
APNs key — and POSTs one alert per token to `api.push.apple.com` or
`api.sandbox.push.apple.com`, per token. It is fire and forget: no result
frame goes back to the host, Apple's errors are logged and dropped, and
nothing about a token is stored. At most 30 NOTIFY frames per minute per host;
the rest are dropped, and neither that nor a malformed payload ever closes the
host link. The contract is `../docs/remote-protocol.md`, "Push notifications".

Four secrets turn it on. Without all four the relay ignores NOTIFY, which is a
perfectly valid way to run one.

| secret | where it comes from |
|---|---|
| `APNS_TEAM_ID` | Apple Developer → Membership, 10 characters |
| `APNS_KEY_ID` | the Key ID of the APNs auth key (`.p8`), 10 characters |
| `APNS_PRIVATE_KEY` | the PEM *contents* of that `.p8` (PKCS#8 EC P-256) |
| `APNS_BUNDLE_ID` | the iOS app's bundle id, sent as `apns-topic` |

```sh
bunx wrangler secret put APNS_TEAM_ID       # prompts, paste, enter
bunx wrangler secret put APNS_KEY_ID
bunx wrangler secret put APNS_BUNDLE_ID
# The key is multi-line, so pipe the file instead of pasting it:
bunx wrangler secret put APNS_PRIVATE_KEY < AuthKey_XXXXXXXXXX.p8
```

The `.p8` never belongs in the repo or in `wrangler.toml`; it lives in your
password manager and in Cloudflare's secret store, nowhere else. One APNs key
serves both environments, so `sandbox` (a development build) and `production`
(TestFlight or the App Store) need no extra configuration — the phone reports
which one its token belongs to.

**Verify the first deploy with one real send.** APNs requires HTTP/2, which
Cloudflare negotiates for outbound `fetch`, but nothing in the test suite can
prove it — the tests inject their own transport. So after the first deploy,
trigger one notification from a paired phone's Mac (finish a turn while the
Mac's window is not focused) and read `wrangler tail`: silence means it
worked, and a failure shows up as an `apns …` line with Apple's status and
`reason` (`BadDeviceToken`, `TopicDisallowed`, `ExpiredProviderToken`, …) or
as a transport error, which is the case to escalate.

## Develop

```sh
bun install
bun run dev            # wrangler dev
bun run check          # tsc --noEmit
bun run lint           # biome
bun run test           # vitest in workerd, via @cloudflare/vitest-pool-workers
```

The tests run the real Durable Object inside workerd, so hibernation, WebCrypto
and close-code handling behave as they do in production. Keep
`compatibility_date` in `wrangler.toml` at or below the newest date the bundled
workerd supports, or the test runtime refuses to start.

## Deploy

```sh
bunx wrangler login
bunx wrangler deploy
```

That publishes to `https://fletch-relay.<subdomain>.workers.dev`. For a custom
domain, add the zone to the same Cloudflare account and give the Worker a
route — either in the dashboard (Workers & Pages → the Worker → Settings →
Domains & Routes → Add custom domain) or in `wrangler.toml`:

```toml
[[routes]]
pattern = "relay.example.com"
custom_domain = true
```

Cloudflare provisions the certificate; WebSockets need no extra configuration.

## Pointing the apps at it

The relay is opt-in and per host. On the Mac, Settings → Remote control sets
`remote.relay_url` (Tauri command `remote_set_relay`) to the base URL, e.g.
`wss://relay.example.com` — no path, the app appends `/v1/host/<hostId>`. Empty
or absent means no relay, and the Mac only dials LAN. With it set, the desktop
keeps a host link up whenever remote access is enabled, reconnecting with
backoff, and reports it in `remote_status.relay`.

The phone learns the URL from the pairing link, which carries
`relay=<url-encoded>` when the host has one, and persists it with the host.
A phone then tries the LAN address first and the relay second. A hand-typed
pairing has no `relay=`, so it stays LAN-only until the URL is entered by hand.

Anyone can run their own relay: it holds no keys and no accounts, and both ends
only need to agree on the base URL.
