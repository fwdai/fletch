#!/bin/sh
# Build the cloud image the one way, on a Mac with Docker Desktop or on the
# x86-64 CI runner, from a built box image: build.sh in a debian:trixie
# container writes the root, `docker import` makes it the image
# ghcr.io/fwdai/fletch-cloud:<box image version>-<REVISION> with the
# entrypoint, and with --test, test.sh runs it.
#
#   packaging/cloud-image/build-docker.sh [--test] [BOX_OUT] [OUT_DIR]
#
# BOX_OUT defaults to packaging/box-image/out (its build-docker.sh, or a
# release's manifest.json and .erofs files), OUT_DIR to packaging/cloud-image/out.
# The image is for the box image's architecture (its manifest's arch); the
# build runs natively whatever that is, and only test.sh needs it to be this
# machine's. OUT_DIR/image then names the reference built, which publish.sh
# pushes; test.sh writes report.md and test.log there. FLETCH_CLOUD_REPOSITORY
# replaces ghcr.io/fwdai/fletch-cloud.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
TEST=0
if [ "${1:-}" = --test ]; then TEST=1; shift; fi
BOX=${1:-$REPO/packaging/box-image/out}
OUT=${2:-$HERE/out}
REPOSITORY=${FLETCH_CLOUD_REPOSITORY:-ghcr.io/fwdai/fletch-cloud}
[ -f "$BOX/manifest.json" ] || { echo "build-docker.sh: no box image in $BOX" >&2; exit 1; }
mkdir -p "$OUT"
BOX=$(cd "$BOX" && pwd)
OUT=$(cd "$OUT" && pwd)

echo "== building the root in a debian:trixie container"
docker run --rm -v "$HERE:/src:ro" -v "$BOX:/box:ro" -v "$OUT:/out" debian:trixie /src/build.sh /box /out
case $(cat "$OUT/arch") in
  aarch64) PLATFORM=linux/arm64 ;;
  x86_64) PLATFORM=linux/amd64 ;;
  *) echo "build-docker.sh: no cloud image for $(cat "$OUT/arch")" >&2; exit 1 ;;
esac
IMAGE=$REPOSITORY:$(cat "$OUT/tag")

echo "== importing $IMAGE ($PLATFORM)"
# One layer, the tar as it is: owners, modes (setuid bits included) and hard
# links come through unchanged. The box image has no extended attributes
# (mkfs.erofs -x -1), so there are none to carry.
docker import --platform "$PLATFORM" \
  --change 'ENTRYPOINT ["/opt/fletch/cloud-entrypoint"]' \
  --change 'ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin' \
  --change 'LABEL org.opencontainers.image.source=https://github.com/fwdai/fletch' \
  --message "$(cat "$OUT/tag")" \
  "$OUT/rootfs.tar" "$IMAGE"
rm "$OUT/rootfs.tar"
echo "$IMAGE" > "$OUT/image"

if [ "$TEST" = 1 ]; then
  echo "== testing $IMAGE"
  "$HERE/test.sh" "$IMAGE" "$OUT"
fi
echo "== done: $IMAGE"
