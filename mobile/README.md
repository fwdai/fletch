# Fletch Mobile

The iOS companion for the Fletch desktop app: a thin Tauri client that drives a
paired Mac over the remote protocol. Nothing runs locally — every agent lives on
the host.

## Web dev loop

```sh
bun install
bun run dev            # http://localhost:1420 — add ?mock=1 for the mock host
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
