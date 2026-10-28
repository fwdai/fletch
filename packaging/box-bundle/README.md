# Bundling the box runtime into Fletch.app

What the macOS app needs to run boxes, and how it gets into the bundle. Port
of Calina's `sandbox/bundle/`. **Not wired into `src-tauri/tauri.conf.json`
yet**: the box engine ships behind a developer flag (`docs/boxes-spec.md` §4)
and the bundle change lands with the engine in a later phase, so release
builds today are unchanged.

| File | What |
| --- | --- |
| `fletch-vm.entitlements` | `com.apple.security.hypervisor`, the one entitlement the helper needs. Nothing else in the app gets it (`docs/boxes-spec.md` §12, nuance 15). |
| `prepare.sh` | Builds `fletch-vm`, copies it and the two libkrun dylibs into `out/`, signs them (ad hoc, or with `APPLE_SIGNING_IDENTITY` plus the hardened runtime and a timestamp for notarization). |
| `check.sh` | Verifies a built app: only `Helpers/fletch-vm` is entitled, and the helper boots a guest with libkrun loaded from the bundle's `Frameworks`. |

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

## Wiring it in (later phase)

In `src-tauri/tauri.conf.json`, or in a platform override merged for release
builds:

```json
{
  "build": {
    "beforeBundleCommand": "../packaging/box-bundle/prepare.sh"
  },
  "bundle": {
    "macOS": {
      "files": {
        "Helpers/fletch-vm": "../packaging/box-bundle/out/fletch-vm",
        "Frameworks/libkrun.1.dylib": "../packaging/box-bundle/out/libkrun.1.dylib",
        "Frameworks/libkrunfw.5.dylib": "../packaging/box-bundle/out/libkrunfw.5.dylib"
      }
    }
  }
}
```

Tauri signs its own binaries and the app around these files but not the files
themselves, so `prepare.sh`'s signatures are what ship. The release workflow
needs `packaging/libkrun/ensure.sh`'s prerequisites (Xcode, not only the
Command Line Tools) or a prebuilt `FLETCH_LIBKRUN_DIR`. `libkrunfw` bundles a
GPL-2.0 kernel: the About screen's licence notice must offer its source and
`NOTICE` lists libkrun (Apache-2.0) when this ships (nuance 19).

After a build:

```sh
packaging/box-bundle/check.sh src-tauri/target/release/bundle/macos/Fletch.app <data dir>/box/images/<version>/base.erofs
```
