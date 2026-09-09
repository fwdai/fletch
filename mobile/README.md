# Fletch Mobile

The iOS companion for the Fletch desktop app: a thin Tauri client that drives a
paired Mac over the remote protocol. Nothing runs locally — every agent lives on
the host.

## Secure channel

Every connection to the host is a Noise `XX` handshake (`snow`, prologue
`fletch-remote-v2`) followed by encrypted binary frames, so the WebSocket — and
any relay that later carries it — only sees ciphertext. Both identities are
static X25519 keys: the phone's is generated on first launch and kept at
`<app data dir>/device_key`, and the host's public key is the `host=` value in
the pairing link, which the phone either checks the handshake against or pins on
first contact. That all lives in `src-tauri/src/remote/` (commands
`remote_connect` / `remote_send` / `remote_close` /
`remote_device_public_key`), and the webview talks plain JSON to it. Which is
also why the browser dev loop can only reach the mock host: outside Tauri there
is no Rust layer to hold the device key, so `?mock=1` is the only way to run
`bun run dev` in a desktop browser.

## LAN first, relay second

A connection attempt walks two candidates in order: the host's LAN address over
`ws://` with a 3 s open timeout, then `wss://<relay>/v1/device/<hostId>` if that
did not answer. The relay base URL comes from `relay=` in the pairing link (the
desktop puts it there when it has a relay configured) and is persisted with the
host; it can also be typed into the Host sheet later, which is the only way a
hand-typed pairing gets one. The relay needs the host's public key, since that
key is its route, so a target without one is LAN-only. Both paths carry the same
Noise handshake and the same frames, so nothing above the transport can tell
them apart or needs to — the path is reported in the Host sheet and that is all
it is used for. The open timeout is enforced in Rust, inside `remote_connect`,
so a candidate the client has moved past is really closed.

## Push notifications

A turn finishing and an agent waiting on an approval reach the phone as APNs
alerts (docs/remote-protocol.md, "Push notifications"). The Mac raises them, the
relay signs and sends them, and all the phone does is hand the host a device
token and act on a tap.

Tauri ships no push plugin, so `src-tauri/plugins/push/` is a small in-repo one:
`request_permission`, `register`, `unregister`, and two events —
`push://token` (`{ token, environment }`) and `push://opened` (the alert's
`fletch` object). The webview wrapper is `src/remote/push.ts` and the flow that
drives it is `src/store/push.ts`: ask iOS once per paired host (the answer is
persisted, so no host is ever asked twice and a refusal is final), then send
`register_push` after every handshake and after every new token. Off iOS every
command answers "push is iOS-only", which is why `cargo check` and the browser
dev loop are unaffected.

Two things in it are worth knowing about:

- **The app delegate is patched at runtime.** The APNs device token only ever
  arrives at `application(_:didRegisterForRemoteNotificationsWithDeviceToken:)`,
  Tauri's iOS `Plugin` base class forwards no app-delegate callbacks, and
  `gen/apple` is generated and gitignored — so there is no AppDelegate to edit.
  The plugin adds that callback (and the failure one) to Tao's delegate class
  with `class_addMethod` when it loads. Tao implements neither, so nothing is
  overridden.
- **`aps-environment` is written by the build script.** Tauri's `bundle.iOS`
  config has no entitlements key, so the plugin's `build.rs` inserts the
  entitlement into the generated Xcode project through
  `tauri_plugin::mobile::update_entitlements`, the same hook
  tauri-plugin-deep-link uses for associated domains. It writes `development`,
  which is what Xcode's own capability writes; Xcode substitutes `production`
  when it signs with a distribution profile. Either way the phone reports the
  environment the *embedded profile* names, so the relay always picks the right
  APNs host.

### Manual Apple steps

These cannot be committed and have to be done once, per team:

1. In the Apple Developer portal, enable the **Push Notifications** capability
   on the `sh.fletch.mobile` App ID, then regenerate the provisioning profile
   (Xcode's automatic signing does this for you once the App ID has it). Without
   it, signing fails with a missing-entitlement error.
2. Create an APNs auth key (`.p8`) and give the relay `APNS_TEAM_ID`,
   `APNS_KEY_ID`, `APNS_PRIVATE_KEY` and `APNS_BUNDLE_ID=sh.fletch.mobile` —
   `apns-topic` is the bundle id, so the two have to match.
3. Run on a real device. The simulator has no embedded profile and cannot
   receive a remote push; the plugin reports `sandbox` there.

## Dictation

The composer's mic button records in the webview and has the Mac transcribe
(docs/remote-protocol.md, "Dictation"; docs/dictation.md, "Dictating from the
phone"). There is no speech model on the phone: `src/dictation/capture.ts` opens
`getUserMedia` into an `AudioWorklet` tap and batches a second of 16-bit PCM at
a time, `src/dictation/session.ts` streams those chunks to the host in order and
asks for the transcript on stop, and `useDictation` turns that into the button's
phases. The Mac has to have its local Whisper engine on with a model downloaded;
`dictation_status` says so, and a slashed mic that explains itself on tap is
what the phone shows otherwise.

Two things are worth knowing:

- **The mic prompt string is `src-tauri/Info.ios.plist`.** The Tauri CLI merges
  it into the generated project's `Info.plist` during `ios dev` / `ios build`
  (it is not applied by `ios init`). Without `NSMicrophoneUsageDescription` iOS
  kills the app on the first `getUserMedia`.
- **The webview's permission prompt is answered by wry.** Its `WKUIDelegate`
  grants `requestMediaCapturePermission` outright, so the only prompt the user
  sees is the system one for the microphone.

`mobile/spike/` holds a standalone capture diagnostic for when something about
the webview's audio needs checking on a device.

## Web dev loop

```sh
bun install
bun run dev            # http://localhost:1420 — needs ?mock=1 (mock host only)
bun run check          # tsc --noEmit
bun run lint           # biome
bun run test           # vitest
```

## iOS setup

`src-tauri/gen/` (the generated Xcode project) is gitignored and never in the
repo. Each machine creates its own on the first init, which needs Xcode and an
Apple team ID.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
bun install
bun run ios:init       # tauri ios init — writes src-tauri/gen/
bun run tauri ios dev
```

There is no shared Xcode project to commit: `tauri ios init` regenerates
`gen/apple` from `tauri.conf.json`, its `project.pbxproj` hardcodes whichever
`DEVELOPMENT_TEAM` ran the init, and the entitlements are written at build time
by the plugins (see above). Re-run `ios:init` instead of sharing the output.
