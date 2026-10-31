#!/bin/sh
# Build the spike images on a Mac, as ../build-mac.sh builds the released
# image: cross-build fletch-guest with cargo, then run build-layers.sh in a
# Debian arm64 container (privileged: the layer installs chroot and mount).
# About 10-15 minutes; downloads ~400 MB of packages from snapshot.debian.org.
#
#   packaging/box-image/spike/build-mac.sh [OUT_DIR]      # default ./vm
#
# Then: packaging/box-image/spike/boot-test.sh OUT_DIR
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${1:-$PWD/vm}
CRATE="$REPO/crates/fletch-guest"
GUEST="${CARGO_TARGET_DIR:-$CRATE/target}/aarch64-unknown-linux-musl/release/fletch-guest"

echo "== building fletch-guest"
rustup target add aarch64-unknown-linux-musl >/dev/null
(cd "$CRATE" && cargo build --quiet --release --target aarch64-unknown-linux-musl)

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
echo "== building the layered image in a debian:trixie container -> $OUT"
# The binary's directory is mounted, not the file (see ../build-mac.sh).
docker run --rm --privileged --platform linux/arm64 \
  -v "$HERE/..:/src:ro" -v "$(dirname "$GUEST"):/guest:ro" -v "$OUT:/out" \
  -e FLETCH_GUEST=/guest/fletch-guest \
  -e "DEBIAN_SNAPSHOT=${DEBIAN_SNAPSHOT:-}" -e "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-}" \
  debian:trixie /src/spike/build-layers.sh /out 0.0.0-spike
echo "== done; boot it with: packaging/box-image/spike/boot-test.sh $OUT"
