#!/bin/sh
# Build the box images the one way, on a Mac with Docker Desktop or on the
# arm64 CI runner: cross-builds fletch-guest, runs build.sh in a Debian arm64
# container (Docker Desktop runs it natively on Apple silicon; privileged,
# since the layer builds chroot and mount), optionally test.sh the same way,
# then writes the delta manifests and installs the result with the app's own
# installer. OUT_DIR is then exactly what publish.sh releases.
#
#   packaging/box-image/build-docker.sh [OUT_DIR] [VERSION]          # -> packaging/box-image/out
#   packaging/box-image/build-docker.sh --test [OUT_DIR] [VERSION]   # and run test.sh on it
#
# VERSION defaults to the VERSION file; the install check holds it to the
# app's pin. DEBIAN_SNAPSHOT and SOURCE_DATE_EPOCH pass through to build.sh;
# WITH_AGENTS to test.sh.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
TEST=0
if [ "${1:-}" = --test ]; then TEST=1; shift; fi
OUT=${1:-$HERE/out}
VERSION=${2:-$(cat "$HERE/VERSION")}
CRATE="$REPO/crates/fletch-guest"
GUEST="${CARGO_TARGET_DIR:-$CRATE/target}/aarch64-unknown-linux-musl/release/fletch-guest"
FLETCH_BOX="${CARGO_TARGET_DIR:-$REPO/crates/fletch-box/target}/release/fletch-box"

echo "== building fletch-guest and the fletch-box CLI"
rustup target add aarch64-unknown-linux-musl >/dev/null
(cd "$CRATE" && cargo build --quiet --release --target aarch64-unknown-linux-musl)
(cd "$REPO/crates/fletch-box" && cargo build --quiet --release --bin fletch-box)

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

echo "== delta manifests"
"$FLETCH_BOX" image chunk "$OUT"/*.erofs

echo "== installing the images with the app's installer"
# Every layer the app offers, from OUT as a local release, into a scratch
# store: the manifest passes the app's checks and names its pin, and every
# file matches it. A build the app could not install fails here.
store=$(mktemp -d)
FLETCH_BOX_RUN_ROOT=$store "$FLETCH_BOX" image install --url "file://$OUT" --layers all > "$store/log" ||
  { rm -rf "$store"; exit 1; }
tail -n 1 "$store/log"
rm -rf "$store"
echo "== done: $OUT"
