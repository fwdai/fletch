# Bundling the box runtime into Fletch.app

What the macOS app needs to run boxes, and how it gets into the bundle and
into a dev build. Port of Calina's `sandbox/bundle/`.

| File | What |
| --- | --- |
| `fletch-vm.entitlements` | `com.apple.security.hypervisor`, the one entitlement the helper needs. Nothing else in the app gets it (`docs/boxes-spec.md` §12, nuance 15). |
| `prepare.sh` | Builds `fletch-vm`, copies it and the two libkrun dylibs into `out/`, signs them (ad hoc, or with `APPLE_SIGNING_IDENTITY` plus the hardened runtime and a timestamp for notarization). Tauri's `beforeBundleCommand`. |
| `ensure-dev.sh` | The dev hook: `prepare.sh`, best effort. Prints one line; without Xcode it prints one warning and exits 0, and the Sandbox page's probe says boxes are not built. `bun run tauri dev` runs it (`scripts/tauri.sh`). |
| `check.sh` | Verifies a built app: only `Helpers/fletch-vm` is entitled, the runtime's signatures verify, and (given an image) the helper boots a guest with libkrun loaded from the bundle's `Frameworks`. |

Calina also stages an "agent harness" external binary (`harness.sh`); Fletch
has no equivalent, the guest agent lives in the image, so it is not ported.

## Layout in the app

```
Fletch.app/Contents/
  MacOS/fletch                  the app; no hypervisor entitlement
  Helpers/fletch-vm             RuntimePaths::bundled finds it here
  Frameworks/libkrun.1.dylib    what fletch-vm dlopens when no lib dir is given
  Frameworks/libkrunfw.5.dylib  (@executable_path/../Frameworks)
```

## Where the app looks (`fletch_core::sandbox::boxes::runtime_paths`)

1. `FLETCH_BOX_HELPER` and `FLETCH_BOX_LIB_DIR` (`scripts/box-dev.sh` prints
   them, for the CLI and the VM tests).
2. The bundle: `Contents/Helpers/fletch-vm`, dylibs in `Contents/Frameworks`.
3. A dev checkout: `packaging/box-bundle/out/` of the repository the running
   binary is inside (`RuntimePaths::dev`; `src-tauri/target/debug/fletch` under
   `tauri dev`). It resolves whether or not `out/` exists, so the probe can say
   "box runtime not built; run packaging/box-bundle/ensure-dev.sh (needs Xcode)".

## Dev build

`bun run tauri dev` runs `ensure-dev.sh` before Vite: `packaging/libkrun/ensure.sh`
(builds `dist/` once, about a minute; a no-op after), then `prepare.sh` with
an ad hoc signature into `out/`. Needs Xcode, not only the Command Line Tools
(bindgen wants libclang). Inside a Fletch sandbox `~/.cargo/registry` is
read-only; the hook then sets `CARGO_HOME` under `$TMPDIR` by itself. Running
it by hand is the same command: `packaging/box-bundle/ensure-dev.sh`.

## Release build

`src-tauri/tauri.macos.conf.json` (merged into `tauri.conf.json` on macOS
only, so Linux and Windows builds never see it) wires `prepare.sh` as
`beforeBundleCommand` and places the three files with `bundle.macOS.files`,
whose keys are paths under `Contents/`. Checked against the Tauri 2.11 CLI and
bundler sources:

- the hook runs with the frontend directory (the repo root) as cwd, hence the
  path `packaging/box-bundle/prepare.sh` with no `../`; `files` values are
  resolved from `src-tauri/`, hence `../packaging/...`;
- the bundler copies `files` into the bundle and does not add them to what it
  signs, and it signs without `--deep`, so the app's own `Entitlements.plist`
  never reaches them and `prepare.sh`'s signatures are the ones that ship. The
  app seal covers them, so they must be signed before the app is, which is the
  order the hook gives.

`prepare.sh` in a release build (`CI` set, or a real `APPLE_SIGNING_IDENTITY`)
does not build libkrun itself: `packaging/libkrun/dist` must exist, or the
bundle fails with the step to run. `release.yml` builds it in a cached step and
imports the signing certificate into a keychain before the Tauri build, because
the Tauri CLI only sets up its own keychain after the hook has run. A local
`bun run tauri build` on a Mac needs Xcode (or `FLETCH_LIBKRUN_DIR` naming a
built dist) for the same reason the dev hook does; unlike the dev hook, it fails
rather than ships an app without the runtime.

`libkrunfw` bundles a GPL-2.0 kernel: `NOTICE` lists it and libkrun
(Apache-2.0) with the source location, and the Settings screen's version footer
shows the same line on macOS (nuance 19).

After a build:

```sh
packaging/box-bundle/check.sh src-tauri/target/universal-apple-darwin/release/bundle/macos/Fletch.app   # signatures only (CI)
packaging/box-bundle/check.sh <that app> ~/Library/Application\ Support/com.fletch.desktop/box/images/<version>/base.erofs  # also boots
```
