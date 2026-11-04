#!/bin/sh
# Build the box images: the base root filesystem and the layers. Runs as root
# on Debian trixie, arm64 or amd64, with CAP_SYS_ADMIN (a privileged container:
# the layer installs chroot and mount /proc): build-docker.sh runs it, on a Mac
# and in CI. The images are for the build host's architecture (`arch` in the
# manifest): a box never emulates.
#
#   build.sh OUT_DIR [VERSION]
#
# VERSION is semver (default: the VERSION file). Expects the guest agent
# cross-built first (crates/fletch-guest, <arch>-unknown-linux-musl);
# FLETCH_GUEST overrides its path.
#
# Writes to OUT_DIR:
#   base.erofs             the root every box boots, read-only
#   layer-<name>.erofs     one per packages/<name>.txt but base: the files that
#                          installing those packages adds to the base, overlaid
#                          by init on /usr, /etc and /var (rootfs/opt/fletch/init)
#   manifest.json          version, arch, and per image: file, size, sha256,
#                          dirs, tree_bytes, packages (what the app verifies
#                          downloads against; `fletch-box image chunk` adds the
#                          delta manifests next to the files)
#   sizes.md               tree and packed sizes, for the release notes
#
# Reproducible inputs: packages come from a snapshot.debian.org timestamp
# (DEBIAN_SNAPSHOT), Node from one nodejs.org tarball checked against its
# SHASUMS256 entry, Bun from one GitHub release zip checked against its
# SHASUMS256 entry, every inode gets SOURCE_DATE_EPOCH, each image has a fixed
# UUID. The same inputs and the same fletch-guest give the same images.
#
# Package managers: the base enables Node's corepack, so `pnpm` and `yarn` (and
# `pnpx`, `yarnpkg`) are shims that run the version a project's package.json
# `packageManager` names, fetched on first use through the box's egress proxy
# — what the Run panel's detected `pnpm dev` / `yarn dev` run. Corepack
# downloads with Node's own fetch, which takes the proxy from the env only
# when told to, and the shims ask on a tty before a download; the host's guest
# env sets both (NODE_USE_ENV_PROXY=1, COREPACK_ENABLE_DOWNLOAD_PROMPT=0,
# `boxes::guest_env`). Bun is a binary of its own, in the devtools layer.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:?usage: build.sh OUT_DIR [VERSION]}
VERSION=${2:-$(cat "$HERE/VERSION")}
WORK=${WORK:-/work}

ARCH=$(uname -m)
SUITE=trixie
# --- pins: packaging/box-image/bump.sh moves these ---
DEBIAN_SNAPSHOT=${DEBIAN_SNAPSHOT:-20261027T000000Z}
NODE_VERSION=22.23.4
NODE_SHA256_ARM64=6287aa46b6c375d816e63ae66d024fbbfd70404825aabdfdf898a970871d2f4b
NODE_SHA256_X64=1805bcd35e6815559bd2eb97db438d3fdb0eb3a5e6a977b7c1badca76337e9ce
BUN_VERSION=1.4.4
BUN_SHA256_ARM64=baa6bb36f51d6e58561e02bbea825038a9df454f673d6d776e8bbf86f8c7850e
BUN_SHA256_X64=e0446eb55995772eb9894bebae9a6be368d985a8b9271ee124486f9946eedd76
# --- end pins ---
# Debian's, Node's and Bun's names for $ARCH. Bun's x64 build is the baseline
# one: the plain build needs AVX2, which a VM is not guaranteed to show.
case "$ARCH" in
  aarch64) DEB_ARCH=arm64 NODE_ARCH=arm64 NODE_SHA256=$NODE_SHA256_ARM64 BUN_ARCH=aarch64 BUN_SHA256=$BUN_SHA256_ARM64 ;;
  x86_64) DEB_ARCH=amd64 NODE_ARCH=x64 NODE_SHA256=$NODE_SHA256_X64 BUN_ARCH=x64-baseline BUN_SHA256=$BUN_SHA256_X64 ;;
  *) echo "build.sh: images are built for aarch64 or x86_64, not $ARCH" >&2; exit 1 ;;
esac
GUEST=${FLETCH_GUEST:-$HERE/../../crates/fletch-guest/target/$ARCH-unknown-linux-musl/release/fletch-guest}
MIRROR=https://snapshot.debian.org/archive/debian/$DEBIAN_SNAPSHOT
SECURITY=https://snapshot.debian.org/archive/debian-security/$DEBIAN_SNAPSHOT
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1767225600} # 2026-01-01
export SOURCE_DATE_EPOCH
# The layers, in the order init stacks them (later on top; they are disjoint).
LAYERS="devtools browser"
# Fixed filesystem UUIDs, one per image.
UUID_BASE=3f2a9c1e-7b4d-4e8a-9d0c-5e6f7a8b9c0d
UUID_DEVTOOLS=3f2a9c1e-7b4d-4e8a-9d0c-5e6f7a8b9c11
UUID_BROWSER=3f2a9c1e-7b4d-4e8a-9d0c-5e6f7a8b9c12
# lz4hc is the one codec the guest kernel (libkrunfw) decodes. 64 KiB clusters,
# dedupe, fragments and tail packing: 24% smaller than the 4 KiB default on the
# same tree. -x -1: no xattrs. -T/--all-time: every inode gets SOURCE_DATE_EPOCH.
MKFS_FLAGS="-zlz4hc,12 -C65536 -Ededupe,fragments,ztailpacking -x -1 -T $SOURCE_DATE_EPOCH --all-time"

echo "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' ||
  { echo "build.sh: VERSION must be semver (X.Y.Z or X.Y.Z-tag), got '$VERSION'" >&2; exit 1; }
[ "$(id -u)" = 0 ] || { echo "build.sh: run as root" >&2; exit 1; }
[ -f "$GUEST" ] || {
  echo "build.sh: no fletch-guest at $GUEST; build crates/fletch-guest for $ARCH-unknown-linux-musl first" >&2
  exit 1
}
mkdir -p "$OUT" "$WORK"
OUT=$(cd "$OUT" && pwd)

echo "== installing build tools"
export DEBIAN_FRONTEND=noninteractive
apt-get -qq update
apt-get -qq install -y --no-install-recommends mmdebstrap erofs-utils jq xz-utils unzip curl ca-certificates rsync >/dev/null

pkgs() { grep -v '^\s*#' "$HERE/packages/$1.txt" | grep -v '^\s*$' | paste -sd, -; }
# The dpkg rules as --dpkgopt arguments. They hold globs, so `set -f` guards
# their expansion.
DPKGOPTS=$(grep -v '^\s*#' "$HERE/dpkg-excludes.cfg" | grep -v '^\s*$' | sed 's/^/--dpkgopt=/' | paste -sd' ' -)
pin_times() { find "$1" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +; }
tree_size() { du -sxb "$1" | cut -f1; }
mib() { awk "BEGIN { printf \"%.0f\", $1 / 1048576 }"; }
mkimage() { # <uuid> <out> <tree>
  rm -f "$2"
  set -f
  # shellcheck disable=SC2086
  mkfs.erofs --quiet $MKFS_FLAGS -U "$1" "$2" "$3"
  set +f
  chmod 644 "$2"
}
clean_tree() {
  rm -rf "$1/var/cache/apt" "$1/var/lib/apt/lists" "$1/var/log"/* "$1/tmp"/* "$1/root/.bash_history"
  mkdir -p "$1/var/cache/apt" "$1/var/lib/apt/lists"
}

# ---------------------------------------------------------------- base
ROOT="$WORK/base"
rm -rf "$ROOT"
echo "== installing Debian $SUITE from snapshot $DEBIAN_SNAPSHOT (base)"
set -f
# shellcheck disable=SC2086
mmdebstrap --mode=root --variant=minbase --architectures="$DEB_ARCH" \
  --aptopt='Acquire::Check-Valid-Until "false"' --aptopt='Acquire::Retries "5"' \
  $DPKGOPTS \
  --include="$(pkgs base)" \
  "$SUITE" "$ROOT" "deb $MIRROR $SUITE main" "deb $SECURITY $SUITE-security main"
set +f
# The same rules for everything installed into a copy of this tree (the layers).
install -D -m 644 "$HERE/dpkg-excludes.cfg" "$ROOT/etc/dpkg/dpkg.cfg.d/fletch-excludes"

echo "== installing Node $NODE_VERSION (headers go to the devtools layer)"
tarball="$WORK/node-$NODE_VERSION-$NODE_ARCH.tar.xz"
[ -f "$tarball" ] || curl -fsSL -o "$tarball" "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-$NODE_ARCH.tar.xz"
echo "$NODE_SHA256  $tarball" | sha256sum -c - >/dev/null
tar -xJf "$tarball" -C "$ROOT/usr/local" --strip-components=1
rm -f "$ROOT/usr/local/CHANGELOG.md" "$ROOT/usr/local/README.md" "$ROOT/usr/local/LICENSE"
rm -rf "$ROOT/usr/local/share/doc" "$ROOT/usr/local/share/man" "$ROOT/usr/local/lib/node_modules/npm/docs"
rm -rf "$WORK/node-include"
mv "$ROOT/usr/local/include" "$WORK/node-include"
# pnpm and yarn as corepack's shims beside node: `corepack enable` links them
# into the bin dir it runs from, relative, so the links hold in the image.
echo "== enabling corepack (pnpm, yarn)"
chroot "$ROOT" /usr/local/bin/corepack enable
for shim in pnpm pnpx yarn yarnpkg; do
  [ -L "$ROOT/usr/local/bin/$shim" ] || { echo "build.sh: corepack enable left no $shim shim" >&2; exit 1; }
done

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
echo "fletch-box-image $VERSION ($ARCH, debian $SUITE @ $DEBIAN_SNAPSHOT, node $NODE_VERSION, bun $BUN_VERSION in devtools)" > "$ROOT/etc/fletch-image"

echo "== cleaning the base"
clean_tree "$ROOT"
# Reproducible: fontconfig caches record their directories' mtimes. Pin every
# mtime to the one the image carries (mkfs.erofs -T) and build the caches after,
# so they match and a browser never rebuilds them into the agent's home.
pin_times "$ROOT"
chroot "$ROOT" /usr/bin/fc-cache -s -f
{
  chroot "$ROOT" dpkg-query -W -f '${binary:Package}=${Version}\n'
  echo "node=$NODE_VERSION"
} | LC_ALL=C sort > "$WORK/base.pkgs"
BASE_TREE_BYTES=$(tree_size "$ROOT")
echo "== writing base.erofs"
mkimage "$UUID_BASE" "$OUT/base.erofs" "$ROOT"

# ---------------------------------------------------------------- layers
for layer in $LAYERS; do
  LR="$WORK/$layer-root"
  D="$WORK/$layer-delta"
  rm -rf "$LR" "$D"
  echo "== building the $layer layer"
  cp -a "$ROOT" "$LR"
  mount -t proc proc "$LR/proc"
  # The image's resolv.conf is empty (a box has no DNS), so apt in the chroot
  # borrows the build host's resolver for the install.
  cp /etc/resolv.conf "$LR/etc/resolv.conf"
  chroot "$LR" sh -c "apt-get -qq -o Acquire::Check-Valid-Until=false -o Acquire::Retries=5 update \
    && apt-get -qq -o Acquire::Check-Valid-Until=false install -y --no-install-recommends $(pkgs "$layer" | tr , ' ')" \
    </dev/null
  umount "$LR/proc"
  : > "$LR/etc/resolv.conf"
  if [ "$layer" = devtools ]; then
    cp -a "$WORK/node-include" "$LR/usr/local/include"
    echo "== installing Bun $BUN_VERSION (devtools layer)"
    zip="$WORK/bun-$BUN_VERSION-$BUN_ARCH.zip"
    [ -f "$zip" ] || curl -fsSL -o "$zip" "https://github.com/oven-sh/bun/releases/download/bun-v$BUN_VERSION/bun-linux-$BUN_ARCH.zip"
    echo "$BUN_SHA256  $zip" | sha256sum -c - >/dev/null
    rm -rf "$WORK/bun"
    unzip -q "$zip" -d "$WORK/bun"
    install -m 755 "$WORK/bun/bun-linux-$BUN_ARCH/bun" "$LR/usr/local/bin/bun"
    ln -s bun "$LR/usr/local/bin/bunx"
  fi
  clean_tree "$LR"
  pin_times "$LR"
  chroot "$LR" /usr/bin/fc-cache -s -f
  chroot "$LR" dpkg-query -W -f '${binary:Package}=${Version}\n' | LC_ALL=C sort > "$WORK/$layer.allpkgs"
  LC_ALL=C comm -13 "$WORK/base.pkgs" "$WORK/$layer.allpkgs" > "$WORK/$layer.pkgs"
  [ "$layer" = devtools ] && echo "bun=$BUN_VERSION" >> "$WORK/$layer.pkgs"

  # The delta: what the layer tree has that the base has not, byte for byte
  # (-c). The package databases, apt state and logs stay with the base; the
  # loader cache too, since the loader's default search path covers the
  # multiarch directories and two layers could not share one cache.
  mkdir "$D"
  rsync -a -c --compare-dest="$ROOT/" \
    --exclude=/proc --exclude=/dev --exclude=/sys --exclude=/run --exclude=/tmp \
    --exclude=/etc/ld.so.cache \
    --exclude=/var/lib/dpkg --exclude=/var/lib/apt --exclude=/var/cache/apt \
    --exclude=/var/cache/debconf --exclude=/var/log --exclude=/var/backups \
    "$LR/" "$D/"
  find "$D" -depth -type d -empty -delete
  dirs=$(ls "$D" | tr '\n' ' ')
  echo "   $layer carries: $dirs"
  for d in $dirs; do
    case "$d" in usr|etc|var) ;; *) echo "build.sh: the $layer layer touches /$d, which init does not overlay" >&2; exit 1;; esac
  done
  jq -n --arg name "$layer" --arg version "$VERSION" \
    --argjson dirs "$(printf '%s\n' $dirs | jq -R . | jq -s .)" \
    '{name: $name, version: $version, dirs: $dirs}' > "$D/.fletch-layer"
  pin_times "$D"
  var=$(echo "$layer" | tr - _)
  eval "uuid=\$UUID_$(echo "$layer" | tr a-z- A-Z_)"
  echo "== writing layer-$layer.erofs"
  mkimage "$uuid" "$OUT/layer-$layer.erofs" "$D"
  eval "${var}_tree_bytes=$(tree_size "$D")"
  rm -rf "$LR"
done

# ---------------------------------------------------------------- manifest and sizes
echo "== writing manifest.json and sizes.md"
entry() { # <name> <file> <dirs json> <packages file> <tree bytes>
  jq -n --arg name "$1" --arg file "$2" --argjson dirs "$3" \
    --argjson size "$(stat -c %s "$OUT/$2")" \
    --arg sha256 "$(sha256sum "$OUT/$2" | cut -d' ' -f1)" \
    --argjson tree_bytes "$5" \
    --rawfile packages "$4" \
    '{name: $name, file: $file, dirs: $dirs, size: $size, sha256: $sha256, tree_bytes: $tree_bytes,
      packages: ($packages | split("\n") | map(select(length > 0)))}'
}
{
  entry base base.erofs '["/"]' "$WORK/base.pkgs" "$BASE_TREE_BYTES"
  for layer in $LAYERS; do
    var=$(echo "$layer" | tr - _)
    eval "tb=\$${var}_tree_bytes"
    entry "$layer" "layer-$layer.erofs" "$(jq -c .dirs "$WORK/$layer-delta/.fletch-layer")" "$WORK/$layer.pkgs" "$tb"
  done
} | jq -s --arg version "$VERSION" --arg arch "$ARCH" '{version: $version, arch: $arch, images: .}' > "$OUT/manifest.json"
chmod 644 "$OUT/manifest.json"

{
  echo "| Image | Tree | Packed | Packages |"
  echo "| --- | ---: | ---: | ---: |"
  jq -r '.images[] | "| \(.name) (`\(.file)`) | \(.tree_bytes / 1048576 | floor) MB | \(.size / 1048576 | floor) MB | \(.packages | length) |"' "$OUT/manifest.json"
  echo
  def=$(jq '[.images[] | select(.name == "base" or .name == "devtools") | .size] | add' "$OUT/manifest.json")
  all=$(jq '[.images[] | .size] | add' "$OUT/manifest.json")
  echo "Default download (base + devtools): $(mib "$def") MB; every layer: $(mib "$all") MB."
} > "$OUT/sizes.md"
cat "$OUT/sizes.md"
echo "== done: $OUT"
