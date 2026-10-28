#!/bin/sh
# Builds the box runtime binaries for development, signs the helper ad hoc with
# the hypervisor entitlement, and prints the environment the CLI and the VM
# tests read. Use it as: eval "$(scripts/box-dev.sh)"
#
#   FLETCH_BOX_LIB_DIR defaults to packaging/libkrun/dist when built, else
#   Calina's dist (~/Code/calina/sandbox/libkrun/dist), the dev artifact until
#   packaging/libkrun/ensure.sh has run here.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CRATE="$ROOT/crates/fletch-box"
TARGET="$CRATE/target/release"
ENTITLEMENTS="$ROOT/packaging/box-bundle/fletch-vm.entitlements"

[ "$(uname -sm)" = "Darwin arm64" ] || { echo "box-dev.sh: boxes run on macOS on Apple silicon" >&2; exit 1; }

echo "== building fletch-vm and fletch-box (release)" >&2
cargo build --release --manifest-path "$CRATE/Cargo.toml" --bins >&2
codesign --force --sign - --entitlements "$ENTITLEMENTS" "$TARGET/fletch-vm" >&2

LIB_DIR=${FLETCH_BOX_LIB_DIR:-}
if [ -z "$LIB_DIR" ]; then
  for candidate in "$ROOT/packaging/libkrun/dist" "$HOME/Code/calina/sandbox/libkrun/dist"; do
    if [ -f "$candidate/libkrun.1.dylib" ] && [ -f "$candidate/libkrunfw.5.dylib" ]; then
      LIB_DIR=$candidate
      break
    fi
  done
fi
[ -n "$LIB_DIR" ] || { echo "box-dev.sh: no libkrun dist found; run packaging/libkrun/ensure.sh" >&2; exit 1; }

echo "export FLETCH_BOX_HELPER=\"$TARGET/fletch-vm\""
echo "export FLETCH_BOX_LIB_DIR=\"$LIB_DIR\""
echo "export PATH=\"$TARGET:\$PATH\""
