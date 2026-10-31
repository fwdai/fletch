#!/bin/sh
# Container entrypoint for the spike: build the layered image, then test it.
#   run.sh OUT_DIR [VERSION]
# Needs a privileged container (loop mounts, overlayfs, chroot). WITH_AGENTS=0
# skips the agent CLI installs (about 1 GB of downloads).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:?usage: run.sh OUT_DIR [VERSION]}
VERSION=${2:-0.0.0-spike}
"$HERE/build-layers.sh" "$OUT" "$VERSION"
exec "$HERE/test-layers.sh" "$OUT"
