#!/bin/sh
# Build the box image on a Mac: cross-builds fletch-guest with cargo, then runs
# build.sh inside a Debian arm64 container (Docker Desktop runs it natively on
# Apple silicon).
#
#   packaging/box-image/build-mac.sh [OUT_DIR] [VERSION]
#
# Defaults: OUT_DIR packaging/box-image/out (gitignored), VERSION 0.0.0-dev.
# DEBIAN_SNAPSHOT and SOURCE_DATE_EPOCH pass through to build.sh.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
OUT=${1:-$HERE/out}
VERSION=${2:-0.0.0-dev}
CRATE="$REPO/crates/fletch-guest"
GUEST="${CARGO_TARGET_DIR:-$CRATE/target}/aarch64-unknown-linux-musl/release/fletch-guest"

echo "== building fletch-guest"
rustup target add aarch64-unknown-linux-musl >/dev/null
(cd "$CRATE" && cargo build --quiet --release --target aarch64-unknown-linux-musl)

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
echo "== building the image in a debian:trixie container"
# Only the pipeline directory and the one binary go in; build.sh stages the
# root tree inside the container, where the filesystem is case-sensitive.
# The binary's directory is mounted, not the file: Docker Desktop binds a single
# file by inode and kept serving the previous build after cargo replaced it,
# so an image built that way carried a stale fletch-guest.
docker run --rm --platform linux/arm64 \
  -v "$HERE:/src:ro" -v "$(dirname "$GUEST"):/guest:ro" -v "$OUT:/out" \
  -e FLETCH_GUEST=/guest/fletch-guest \
  -e "DEBIAN_SNAPSHOT=${DEBIAN_SNAPSHOT:-}" -e "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-}" \
  debian:trixie /src/build.sh /out "$VERSION"
