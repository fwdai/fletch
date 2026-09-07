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

## Web dev loop

```sh
bun install
bun run dev            # http://localhost:1420 — needs ?mock=1 (mock host only)
bun run check          # tsc --noEmit
bun run lint           # biome
bun run test           # vitest
```

## iOS setup

`src-tauri/gen/` (the generated Xcode project) is **not** in the repo; it is
created on the first init, which needs Xcode and an Apple team ID.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
bun install
bun run ios:init       # tauri ios init — writes src-tauri/gen/
bun run tauri ios dev
```

Commit `src-tauri/gen/` after that first init if the team wants a shared Xcode
project; until then it is a local artifact.
