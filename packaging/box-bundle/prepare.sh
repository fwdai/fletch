#!/bin/sh
# Stages and signs what the macOS app needs to run boxes. Tauri runs it after
# compiling, before bundling (beforeBundleCommand; see README.md), and
# bundle.macOS.files then copies these into the app as they are:
#
#   out/fletch-vm           -> Contents/Helpers/     the fletch-vm cargo just built, signed with the
#                                                    hypervisor entitlement and nothing else
#   out/libkrun.1.dylib     -> Contents/Frameworks/  from FLETCH_LIBKRUN_DIR, else packaging/libkrun/dist
#   out/libkrunfw.5.dylib   -> Contents/Frameworks/  (built by packaging/libkrun/ensure.sh when needed)
#
# Signs with APPLE_SIGNING_IDENTITY, the variable Tauri signs the app with;
# unset means ad hoc. Port of Calina's sandbox/bundle/prepare.sh.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT="$ROOT/packaging/box-bundle/out"
LIB_DIR=${FLETCH_LIBKRUN_DIR:-$ROOT/packaging/libkrun/dist}
IDENTITY=${APPLE_SIGNING_IDENTITY:--}
# fletch-vm is built by crates/fletch-box, which has its own target dir unless
# CARGO_TARGET_DIR says otherwise.
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/crates/fletch-box/target}
PROFILE=release
[ "${TAURI_ENV_DEBUG:-false}" = true ] && PROFILE=debug

[ "$(uname -s)" = Darwin ] || { echo "prepare.sh: the VM runtime is bundled on macOS only" >&2; exit 1; }
[ -n "${FLETCH_LIBKRUN_DIR:-}" ] || "$ROOT/packaging/libkrun/ensure.sh"
for lib in libkrun.1.dylib libkrunfw.5.dylib; do
  [ -f "$LIB_DIR/$lib" ] || { echo "prepare.sh: no $LIB_DIR/$lib (FLETCH_LIBKRUN_DIR)" >&2; exit 1; }
done

echo "== building fletch-vm ($PROFILE)"
flags=
[ "$PROFILE" = release ] && flags=--release
# shellcheck disable=SC2086
cargo build $flags --manifest-path "$ROOT/crates/fletch-box/Cargo.toml" --bin fletch-vm
# With --target, cargo builds into target/<triple>/; take whichever build is newest.
VM=$(ls -t "$TARGET_DIR/${TAURI_ENV_TARGET_TRIPLE:-none}/$PROFILE/fletch-vm" \
  "$TARGET_DIR/$PROFILE/fletch-vm" 2>/dev/null | head -1) || true
[ -n "$VM" ] || { echo "prepare.sh: no $PROFILE fletch-vm in $TARGET_DIR" >&2; exit 1; }

rm -rf "$OUT" && mkdir -p "$OUT"
cp "$VM" "$LIB_DIR/libkrun.1.dylib" "$LIB_DIR/libkrunfw.5.dylib" "$OUT/"

# Tauri signs its own binaries and the app around these, but not these files
# themselves, so the signatures made here are the ones that ship. Notarization
# requires a secure timestamp and the hardened runtime. The hardened runtime
# only loads libraries signed by the same team, which ad hoc signatures don't
# have, so an ad hoc (local, never notarized) build goes without it.
set -- --force --sign "$IDENTITY"
vm_runtime="--options runtime"
if [ "$IDENTITY" = - ]; then vm_runtime=""; else set -- "$@" --timestamp; fi
echo "== signing fletch-vm and libkrun ($IDENTITY)"
codesign "$@" "$OUT/libkrun.1.dylib" "$OUT/libkrunfw.5.dylib"
# shellcheck disable=SC2086
codesign "$@" $vm_runtime --entitlements "$ROOT/packaging/box-bundle/fletch-vm.entitlements" "$OUT/fletch-vm"
