#!/bin/sh
# Build the box images on a Mac: cross-builds fletch-guest with cargo, then
# runs build.sh in a Debian arm64 container (Docker Desktop runs it natively
# on Apple silicon; privileged, since the layer builds chroot and mount).
#
#   packaging/box-image/build-mac.sh [OUT_DIR] [VERSION]     # -> packaging/box-image/out
#   packaging/box-image/build-mac.sh --test [OUT_DIR] [VERSION]   # then run test.sh on it
#
# VERSION defaults to the VERSION file. DEBIAN_SNAPSHOT and SOURCE_DATE_EPOCH
# pass through to build.sh; WITH_AGENTS to test.sh. The delta manifests the app
# uses for small updates are written afterwards with
# `cargo run -p fletch-box --bin fletch-box -- image chunk OUT_DIR/*.erofs`.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
TEST=0
if [ "${1:-}" = --test ]; then TEST=1; shift; fi
OUT=${1:-$HERE/out}
VERSION=${2:-$(cat "$HERE/VERSION")}
CRATE="$REPO/crates/fletch-guest"
GUEST="${CARGO_TARGET_DIR:-$CRATE/target}/aarch64-unknown-linux-musl/release/fletch-guest"

echo "== building fletch-guest"
rustup target add aarch64-unknown-linux-musl >/dev/null
(cd "$CRATE" && cargo build --quiet --release --target aarch64-unknown-linux-musl)

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
echo "== building the images in a debian:trixie container"
# Only this directory and the one binary go in; build.sh stages the trees
# inside the container, where the filesystem is case-sensitive. The binary's
# directory is mounted, not the file: Docker Desktop binds a single file by
# inode and kept serving the previous build after cargo replaced it.
run_in_container() {
  docker run --rm --privileged --platform linux/arm64 \
    -v "$HERE:/src:ro" -v "$(dirname "$GUEST"):/guest:ro" -v "$OUT:/out" \
    -e FLETCH_GUEST=/guest/fletch-guest -e "WITH_AGENTS=${WITH_AGENTS:-1}" \
    -e "DEBIAN_SNAPSHOT=${DEBIAN_SNAPSHOT:-}" -e "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-}" \
    debian:trixie "$@"
}
run_in_container /src/build.sh /out "$VERSION"
if [ "$TEST" = 1 ]; then
  echo "== testing the images"
  run_in_container sh -c "apt-get -qq update && apt-get -qq install -y --no-install-recommends jq >/dev/null && /src/test.sh /out"
fi
