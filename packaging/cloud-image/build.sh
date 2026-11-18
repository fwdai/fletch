#!/bin/sh
# Build the cloud image's root filesystem from a built box image: the base
# with every layer on it, flattened, plus what a machine reached over the
# network needs instead of the VM plumbing. Runs as root on Debian trixie (a
# container: build-docker.sh runs it, on a Mac and in CI); executes nothing
# from the images, so any architecture builds any architecture's root.
#
#   build.sh BOX_OUT OUT_DIR
#
# BOX_OUT holds a box image as packaging/box-image/build.sh writes it (or its
# release): manifest.json, base.erofs, layer-<name>.erofs. Writes to OUT_DIR:
#   rootfs.tar   the root, numeric owners, for `docker import`
#   tag          <box image version>-<fingerprint of this directory's inputs>
#                (inputs.sh)
#   arch         the manifest's arch (aarch64, x86_64)
#
# The box image is the one source of what a box contains; nothing here
# installs a package. The flattened root is what init's overlays show a box
# (/opt/fletch/init, "mounting the system"): each layer's directories over the
# base, in the manifest's order, later on top. A layer only adds or replaces
# files (packaging/box-image/build.sh makes it with rsync --compare-dest and
# mkfs.erofs without xattrs, so it can carry neither an overlay whiteout nor
# an opaque directory), so copying it over the base is the overlay.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BOX=${1:?usage: build.sh BOX_OUT OUT_DIR}
OUT=${2:?usage: build.sh BOX_OUT OUT_DIR}
INPUTS=$("$HERE/inputs.sh" "$HERE")
WORK=${WORK:-/work/cloud-image}
ROOT=$WORK/root
M=$BOX/manifest.json

fail() { echo "build.sh: $*" >&2; exit 1; }

[ "$(id -u)" = 0 ] || fail "run as root"
echo "$INPUTS" | grep -Eq '^[0-9a-f]{12}$' || fail "inputs.sh gave '$INPUTS', not a fingerprint"
[ -f "$M" ] || fail "no $M (packaging/box-image/build-docker.sh first, or a release's files)"

echo "== installing build tools"
export DEBIAN_FRONTEND=noninteractive
apt-get -qq update
apt-get -qq install -y --no-install-recommends erofs-utils jq >/dev/null

VERSION=$(jq -r .version "$M")
ARCH=$(jq -r .arch "$M")
[ "$(jq -r '.images[0].name' "$M")" = base ] || fail "$M does not list the base first"
echo "== box image $VERSION ($ARCH), cloud inputs $INPUTS"
jq -r '.images[] | "\(.sha256)  \(.file)"' "$M" | (cd "$BOX" && sha256sum --quiet -c -) ||
  fail "the files in $BOX do not match $M"

rm -rf "$WORK"
mkdir -p "$WORK" "$OUT"
echo "== extracting base.erofs"
fsck.erofs --extract="$ROOT" "$BOX/base.erofs"
for layer in $(jq -r '.images[1:][] | .name' "$M"); do
  L=$WORK/layer-$layer
  echo "== extracting layer-$layer.erofs and copying it over the root"
  fsck.erofs --extract="$L" "$BOX/layer-$layer.erofs"
  [ -f "$L/.fletch-layer" ] || fail "layer-$layer.erofs has no /.fletch-layer marker"
  # A character device is how an overlay deletes a lower file; a layer never
  # holds one, and this copy could not honour it.
  [ -z "$(find "$L" -type c -print -quit)" ] || fail "layer-$layer.erofs holds a device node"
  dirs=$(jq -r '.dirs[]' "$L/.fletch-layer")
  for d in $dirs; do
    case "$d" in usr|etc|var) ;; *) fail "layer-$layer.erofs carries /$d, which init does not overlay";; esac
  done
  # tar replaces a file by unlinking it first, so a file hard-linked in the
  # base keeps its other names, as under the overlay, and gives a directory
  # the layer's owner and mode, as the overlay shows the upper one's.
  # shellcheck disable=SC2086
  tar -C "$L" -cf - $dirs | tar -C "$ROOT" -xpf - --numeric-owner
  rm -rf "$L"
done

echo "== the cloud changes"
# The guest contract over TCP (README.md); the entrypoint starts sshd.
install -m 755 "$HERE/entrypoint" "$ROOT/opt/fletch/cloud-entrypoint"
# apt and dockerd reach the network themselves: nothing listens on the
# egress loopback (127.0.0.1:8899), nor serves the containers' sentinel DNS
# on the bridge. dockerd is not started (nothing listens on its socket), but
# one started by hand finds a config that works: the data root, the nftables
# backend and the bridge stay, the dns and proxies go.
rm "$ROOT/etc/apt/apt.conf.d/90fletch-proxy"
daemon=$ROOT/etc/docker/daemon.json
jq 'del(.dns, .proxies)' "$daemon" > "$daemon.new"
chmod 644 "$daemon.new"
mv "$daemon.new" "$daemon"
# /opt/fletch/tools stays the image's empty directory: the entrypoint binds
# /data/tools on it.
[ -d "$ROOT/opt/fletch/tools" ] || fail "no /opt/fletch/tools directory in the base"
echo "fletch-cloud-image $VERSION-$INPUTS ($ARCH, box image $VERSION, cloud inputs $INPUTS)" > "$ROOT/etc/fletch-cloud-image"

echo "== writing rootfs.tar"
tar -C "$ROOT" --numeric-owner -cf "$OUT/rootfs.tar" .
echo "$VERSION-$INPUTS" > "$OUT/tag"
echo "$ARCH" > "$OUT/arch"
rm -rf "$WORK"
echo "== done: $OUT/rootfs.tar ($(du -m "$OUT/rootfs.tar" | cut -f1) MB), tag $VERSION-$INPUTS"
