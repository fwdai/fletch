#!/bin/sh
# Build the box base image. Runs as root on Debian arm64: a `debian:trixie`
# container (build-mac.sh) or CI.
#
#   build.sh OUT_DIR [VERSION]
#
# VERSION is semver (default 0.0.0-dev); releases are X.Y.Z. Expects the guest
# agent cross-built first (crates/fletch-guest, aarch64-unknown-linux-musl);
# FLETCH_GUEST overrides its path.
#
# Writes to OUT_DIR:
#   base.erofs      read-only root filesystem, lz4hc erofs
#   manifest.json   {version, arch, file, size, sha256, packages}: what fletch-box's
#                   ImageStore pins, plus the package list to rebuild from
#
# Reproducible inputs: packages come from a snapshot.debian.org timestamp
# (DEBIAN_SNAPSHOT), Node from one nodejs.org tarball checked against its
# SHASUMS256 entry, every inode gets SOURCE_DATE_EPOCH, the filesystem UUID is
# fixed. Rebuilding with the same inputs and the same fletch-guest binary gives
# the same base.erofs.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:?usage: build.sh OUT_DIR [VERSION]}
VERSION=${2:-0.0.0-dev}
GUEST=${FLETCH_GUEST:-$HERE/../../crates/fletch-guest/target/aarch64-unknown-linux-musl/release/fletch-guest}

ARCH=aarch64
SUITE=trixie
DEBIAN_SNAPSHOT=${DEBIAN_SNAPSHOT:-20261027T000000Z}
MIRROR=https://snapshot.debian.org/archive/debian/$DEBIAN_SNAPSHOT
SECURITY=https://snapshot.debian.org/archive/debian-security/$DEBIAN_SNAPSHOT
# Node 22 LTS as a tarball rather than the NodeSource apt repo: one URL and one
# hash pin it, no third-party signing key or repo metadata to keep in step.
NODE_VERSION=22.23.4
NODE_SHA256=6287aa46b6c375d816e63ae66d024fbbfd70404825aabdfdf898a970871d2f4b
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1767225600} # 2026-01-01
export SOURCE_DATE_EPOCH
BASE_UUID=3f2a9c1e-7b4d-4e8a-9d0c-5e6f7a8b9c0d

echo "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' ||
  { echo "build.sh: VERSION must be semver (X.Y.Z or X.Y.Z-tag), got '$VERSION'" >&2; exit 1; }
[ "$(id -u)" = 0 ] || { echo "build.sh: run as root" >&2; exit 1; }
[ "$(uname -m)" = "$ARCH" ] || { echo "build.sh: needs an $ARCH host" >&2; exit 1; }
[ -f "$GUEST" ] || {
  echo "build.sh: no fletch-guest at $GUEST; build crates/fletch-guest for aarch64-unknown-linux-musl first" >&2
  exit 1
}

echo "== installing build tools"
export DEBIAN_FRONTEND=noninteractive
apt-get -qq update
apt-get -qq install -y --no-install-recommends mmdebstrap erofs-utils jq xz-utils curl ca-certificates >/dev/null

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
ROOT="$WORK/rootfs"
mkdir -p "$OUT"

echo "== installing Debian $SUITE from snapshot $DEBIAN_SNAPSHOT"
PACKAGES=$(grep -v '^\s*#' "$HERE/packages.txt" | grep -v '^\s*$' | paste -sd, -)
# Snapshot Release files are past their Valid-Until by design. Docs, man pages
# and info are left out of the image; copyright files stay.
mmdebstrap --mode=root --variant=minbase --architectures=arm64 \
  --aptopt='Acquire::Check-Valid-Until "false"' --aptopt='Acquire::Retries "5"' \
  --dpkgopt='path-exclude=/usr/share/doc/*' --dpkgopt='path-include=/usr/share/doc/*/copyright' \
  --dpkgopt='path-exclude=/usr/share/man/*' --dpkgopt='path-exclude=/usr/share/info/*' \
  --include="$PACKAGES" \
  "$SUITE" "$ROOT" "deb $MIRROR $SUITE main" "deb $SECURITY $SUITE-security main"

echo "== installing Node $NODE_VERSION"
tarball="$WORK/node.tar.xz"
curl -fsSL -o "$tarball" "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-arm64.tar.xz"
echo "$NODE_SHA256  $tarball" | sha256sum -c - >/dev/null
tar -xJf "$tarball" -C "$ROOT/usr/local" --strip-components=1
rm -f "$ROOT/usr/local/CHANGELOG.md" "$ROOT/usr/local/README.md" "$ROOT/usr/local/LICENSE"

echo "== adding Fletch files"
cp -R "$HERE/rootfs/." "$ROOT/"
chmod 755 "$ROOT/opt/fletch/init"
chmod 644 "$ROOT/etc/ssh/sshd_config.fletch"
install -D -m 755 "$GUEST" "$ROOT/opt/fletch/bin/fletch-guest"
# `ssh box fletch-guest exec ...` runs through the login shell's default PATH.
ln -s /opt/fletch/bin/fletch-guest "$ROOT/usr/local/bin/fletch-guest"
# Mountpoints init needs: the data disk, the tools disk, and /Users, the first
# component of every identity path from a macOS host (init shadows it with a
# tmpfs; /home and /root already exist for Linux hosts). /Volumes is the other
# root a folder the user shares with a box can sit under (an external disk).
mkdir -p "$ROOT/data" "$ROOT/opt/fletch/tools" "$ROOT/Users" "$ROOT/Volumes"
# The agent runs as `agent`: uid 1000, home on the data disk, no password but
# not locked (`*`, not `!`), or sshd would refuse the account.
chroot "$ROOT" useradd --uid 1000 --user-group --no-create-home \
  --home-dir /data/home --shell /bin/bash --password '*' agent
echo fletch-box > "$ROOT/etc/hostname"
printf '127.0.0.1\tlocalhost fletch-box\n::1\tlocalhost\n' > "$ROOT/etc/hosts"
# No DNS inside the VM: the egress proxy resolves names on the host.
: > "$ROOT/etc/resolv.conf"
# sshd uses the per-workspace key on /data; the postinst-generated ones are
# unused and would make the image differ per build.
rm -f "$ROOT"/etc/ssh/ssh_host_*
: > "$ROOT/etc/machine-id"
echo "fletch-box-image $VERSION ($ARCH, debian $SUITE @ $DEBIAN_SNAPSHOT, node $NODE_VERSION)" > "$ROOT/etc/fletch-image"

echo "== cleaning"
rm -rf "$ROOT/var/cache/apt" "$ROOT/var/lib/apt/lists" "$ROOT/var/log"/* "$ROOT/tmp"/* "$ROOT/root/.bash_history"
mkdir -p "$ROOT/var/cache/apt" "$ROOT/var/lib/apt/lists"
# Reproducible: fontconfig caches record their directories' mtimes. Pin every
# mtime to the one the image carries (mkfs.erofs -T) and rebuild them, so they
# match and Chromium never rebuilds them into the agent's home.
find "$ROOT" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
chroot "$ROOT" /usr/bin/fc-cache -s -f
{
  chroot "$ROOT" dpkg-query -W -f '${binary:Package}=${Version}\n'
  echo "node=$NODE_VERSION"
} | sort > "$WORK/packages.lst"

echo "== writing base.erofs"
rm -f "$OUT/base.erofs"
# -x -1: no xattrs, nothing in the image needs them. -T: every inode gets SOURCE_DATE_EPOCH.
mkfs.erofs --quiet -zlz4hc -x -1 -T "$SOURCE_DATE_EPOCH" --all-time -U "$BASE_UUID" \
  "$OUT/base.erofs" "$ROOT"

echo "== writing manifest.json"
jq -n --arg version "$VERSION" --arg arch "$ARCH" \
  --argjson size "$(stat -c %s "$OUT/base.erofs")" \
  --arg sha256 "$(sha256sum "$OUT/base.erofs" | cut -d' ' -f1)" \
  --rawfile packages "$WORK/packages.lst" \
  '{version: $version, arch: $arch, file: "base.erofs", size: $size, sha256: $sha256,
    packages: ($packages | split("\n") | map(select(length > 0)))}' > "$OUT/manifest.json"

chmod 644 "$OUT/base.erofs" "$OUT/manifest.json"
jq -r '"   base.erofs \(.size) bytes, sha256 \(.sha256)"' "$OUT/manifest.json"
echo "== done: $OUT"
