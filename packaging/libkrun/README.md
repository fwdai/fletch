# libkrun build

The VM runtime the box engine ships: libkrun 1.19.6 with four patches, built
without GPU support and with disk support (`blk`), and libkrunfw 5.6.2, the
Linux 6.12 guest kernel. The build script and patches are vendored from
Calina's `sandbox/libkrun/` (the upstream of this runtime) unchanged except for
paths.

| File in `dist/`     | What                                                     |
| ------------------- | -------------------------------------------------------- |
| `libkrun.1.dylib`   | libkrun with `patches/*.patch`, `@rpath/libkrun.1.dylib` |
| `libkrunfw.5.dylib` | Kernel bundle, `@rpath/libkrunfw.5.dylib`                |
| `manifest.txt`      | Versions, source sha256s, patches and dylib sha256s      |

Both dylibs are ad hoc signed; the app bundle re-signs them
(`packaging/box-bundle/prepare.sh`). `build/`, `dist/` and `dist-unpatched/`
are build outputs and must stay out of git.

## Building

```sh
packaging/libkrun/ensure.sh            # builds dist/ when missing or when build.sh / a patch changed
packaging/libkrun/build.sh             # force a rebuild -> dist/
packaging/libkrun/build.sh --unpatched # -> dist-unpatched/, the same without patches
```

Runs on macOS on Apple silicon with Xcode (not only the Command Line Tools:
bindgen needs libclang), rustup and cargo. Sources are pinned by sha256 and
downloaded into `build/`. The first run takes about a minute, later ones about
15 s. Two runs in the same checkout give byte-identical dylibs.

Nothing calls `build.sh` by hand in the normal flow: `packaging/box-bundle/prepare.sh`
runs `ensure.sh` for a dev build (`ensure-dev.sh`, which `bun run tauri dev`
runs), and the release workflow runs it in its own cached step
(`.github/workflows/release.yml`). Until the first run `dist/` does not exist
and the runtime probe reports boxes as not built.

Build environment notes:

- **Linux init.** libkrun embeds a static Linux `init`, cross-compiled with
  `/usr/bin/clang` against a Debian bookworm sysroot that libkrun's Makefile
  downloads. Linking needs `ld.lld`; if it isn't installed, the script uses
  rustup's `rust-lld` under that name.
- **libclang.** The script points `LIBCLANG_PATH` and
  `DYLD_FALLBACK_LIBRARY_PATH` at Xcode's toolchain and runs `cargo` directly:
  `/usr/bin/make` is SIP-protected and drops `DYLD_*` variables.
- **`CARGO_HOME`** is respected. Inside a Fletch sandbox `~/.cargo/registry`
  is read-only, so use one outside the repo, e.g.
  `CARGO_HOME="${TMPDIR}fletch-cargo-home" packaging/libkrun/build.sh`.

## Patches

Carried here; they apply to a pinned, checksummed libkrun source at build
time. Moving to a newer libkrun means rebasing them. Each is in `git am`
format and could be sent upstream as is.

1. **init: apply generic mount options as mount flags.**
   `krun_set_root_disk_remount` passes its options to `mount(2)` as filesystem
   data with no flags, so `ro,nodev,nosuid` fail with EINVAL and a read-only
   disk cannot be the root. The patch turns generic options into `MS_*` flags
   and, like `mount(8)`, retries read-only on EACCES or EROFS. The runtime
   relies on it: the base image is attached read-only and remounted with `ro`.
2. **balloon: give freed guest memory back to macOS.** Free page reporting
   called `madvise(MADV_FREE)`, which leaves the VM's footprint as it was. The
   patch unmaps the reported range from the guest, marks it
   `MADV_FREE_REUSABLE` and maps it back. Measured: 1776 MB falling to 465 MB
   within ten seconds of the guest dropping its cache.
3. **block: never change the length of the image file.** imago, libkrun's disk
   backend, discards a range that reaches the end of a file by truncating it,
   so a discard at the end of a raw disk shrank the image for good. The patch
   zeroes the last sector with a plain write instead. Init formats the data
   disk 4 MiB short of the device as a belt to this brace.
4. **init: report the exit code when the root is a disk.** init skipped the
   exit-code ioctl after switching to a disk root, so the VM always exited 0.
   With the patch `fletch-vm`'s exit status is the guest's: a clean `poweroff`
   is 0, which the runtime and the CLI read as a clean stop.

Upstream's patch test boots its own image with its own runner, so it is not
vendored. The crate's VM tests (`crates/fletch-box/tests/vm.rs`) exercise 1 and 4
on every run (read-only erofs root, exit status after a control stop).
