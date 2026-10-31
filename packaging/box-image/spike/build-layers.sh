#!/bin/sh
# Spike: build the layered box image. Runs as root on Debian trixie arm64 (the
# privileged container run.sh and the workflow set up).
#
#   build-layers.sh OUT_DIR [VERSION]
#
# Writes to OUT_DIR:
#   base.erofs                 the base, new mkfs flags
#   base-oldflags.erofs        the same tree with today's flags (-zlz4hc, 4 KiB clusters)
#   base-big.erofs             the same tree with 256 KiB clusters (AOSP's setting)
#   base-v2.erofs              the base tree with one new file and two edited files (delta test input)
#   layer-devtools.erofs       the devtools delta
#   layer-browser.erofs        the browser delta
#   manifest.json              version, arch, one entry per image: file, size, sha256, dirs, packages
#   sizes.md                   uncompressed tree sizes and every image's size
#   build.log                  what went where
#
# FLETCH_GUEST names a cross-built fletch-guest; without one the base boots
# nothing (chroot tests only). DEBIAN_SNAPSHOT and SOURCE_DATE_EPOCH as in build.sh.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
IMAGE_DIR=$(cd "$HERE/.." && pwd)
OUT=${1:?usage: build-layers.sh OUT_DIR [VERSION]}
VERSION=${2:-0.0.0-spike}
GUEST=${FLETCH_GUEST:-}
WORK=${WORK:-/work}

ARCH=aarch64
SUITE=trixie
DEBIAN_SNAPSHOT=${DEBIAN_SNAPSHOT:-20261027T000000Z}
MIRROR=https://snapshot.debian.org/archive/debian/$DEBIAN_SNAPSHOT
SECURITY=https://snapshot.debian.org/archive/debian-security/$DEBIAN_SNAPSHOT
NODE_VERSION=22.23.4
NODE_SHA256=6287aa46b6c375d816e63ae66d024fbbfd70404825aabdfdf898a970871d2f4b
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1767225600} # 2026-01-01
export SOURCE_DATE_EPOCH
LAYERS="devtools browser"
# Fixed filesystem UUIDs: one per image, so a rebuild is byte-identical.
UUID_BASE=3f2a9c1e-7b4d-4e8a-9d0c-5e6f7a8b9c0d
UUID_DEVTOOLS=3f2a9c1e-7b4d-4e8a-9d0c-5e6f7a8b9c11
UUID_BROWSER=3f2a9c1e-7b4d-4e8a-9d0c-5e6f7a8b9c12
UUID_V2=3f2a9c1e-7b4d-4e8a-9d0c-5e6f7a8b9c20
# Today's flags (build.sh) and the proposed ones. -x -1: no xattrs. -T/--all-time:
# every inode gets SOURCE_DATE_EPOCH.
OLD_FLAGS="-zlz4hc -x -1 -T $SOURCE_DATE_EPOCH --all-time"
NEW_FLAGS="-zlz4hc,12 -C65536 -Ededupe -Efragments -Eztailpacking -x -1 -T $SOURCE_DATE_EPOCH --all-time"
BIG_FLAGS="-zlz4hc,12 -C262144 -Ededupe -Efragments -Eztailpacking -x -1 -T $SOURCE_DATE_EPOCH --all-time"

[ "$(id -u)" = 0 ] || { echo "build-layers.sh: run as root" >&2; exit 1; }
[ "$(uname -m)" = "$ARCH" ] || { echo "build-layers.sh: needs an $ARCH host" >&2; exit 1; }
mkdir -p "$OUT" "$WORK"
OUT=$(cd "$OUT" && pwd)
LOG="$OUT/build.log"
: > "$LOG"
say() { echo "== $*"; echo "== $*" >> "$LOG"; }

say "installing build tools"
export DEBIAN_FRONTEND=noninteractive
apt-get -qq update
apt-get -qq install -y --no-install-recommends mmdebstrap erofs-utils jq xz-utils curl ca-certificates \
  rsync python3 >/dev/null
mkfs.erofs --version 2>&1 | head -1 | tee -a "$LOG"

pkgs() { grep -v '^\s*#' "$HERE/packages/$1.txt" | grep -v '^\s*$' | paste -sd, -; }
# The dpkg rules as --dpkgopt arguments. They contain globs: no word splitting
# may expand them, hence set -f around the use.
DPKGOPTS=$(grep -v '^\s*#' "$HERE/dpkg-excludes.cfg" | grep -v '^\s*$' | sed 's/^/--dpkgopt=/' | paste -sd' ' -)
pin_times() { find "$1" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +; }
tree_size() { du -sxb "$1" | cut -f1; }
mib() { awk "BEGIN { printf \"%.0f\", $1 / 1048576 }"; }
mkimage() { # <uuid> <flags> <out> <tree>
  rm -f "$3"
  set -f
  # shellcheck disable=SC2086
  mkfs.erofs --quiet $2 -U "$1" "$3" "$4"
  set +f
  chmod 644 "$3"
}

# ---------------------------------------------------------------- base
ROOT="$WORK/base"
rm -rf "$ROOT"
say "installing Debian $SUITE from snapshot $DEBIAN_SNAPSHOT (base)"
set -f
# shellcheck disable=SC2086
mmdebstrap --mode=root --variant=minbase --architectures=arm64 \
  --aptopt='Acquire::Check-Valid-Until "false"' --aptopt='Acquire::Retries "5"' \
  $DPKGOPTS \
  --include="$(pkgs base)" \
  "$SUITE" "$ROOT" "deb $MIRROR $SUITE main" "deb $SECURITY $SUITE-security main"
set +f
# The same rules for everything installed into a copy of this tree later.
install -D -m 644 "$HERE/dpkg-excludes.cfg" "$ROOT/etc/dpkg/dpkg.cfg.d/fletch-excludes"

say "installing Node $NODE_VERSION (headers go to the devtools layer)"
tarball="$WORK/node.tar.xz"
[ -f "$tarball" ] || curl -fsSL -o "$tarball" "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-arm64.tar.xz"
echo "$NODE_SHA256  $tarball" | sha256sum -c - >/dev/null
tar -xJf "$tarball" -C "$ROOT/usr/local" --strip-components=1
rm -f "$ROOT/usr/local/CHANGELOG.md" "$ROOT/usr/local/README.md" "$ROOT/usr/local/LICENSE"
rm -rf "$ROOT/usr/local/share/doc" "$ROOT/usr/local/share/man" "$ROOT/usr/local/lib/node_modules/npm/docs"
rm -rf "$WORK/node-include"
mv "$ROOT/usr/local/include" "$WORK/node-include"

say "adding Fletch files"
cp -R "$IMAGE_DIR/rootfs/." "$ROOT/"
install -m 755 "$HERE/init" "$ROOT/opt/fletch/init"
chmod 644 "$ROOT/etc/ssh/sshd_config.fletch"
if [ -n "$GUEST" ] && [ -f "$GUEST" ]; then
  install -D -m 755 "$GUEST" "$ROOT/opt/fletch/bin/fletch-guest"
  ln -s /opt/fletch/bin/fletch-guest "$ROOT/usr/local/bin/fletch-guest"
else
  say "no fletch-guest given: the base will not boot, chroot tests only"
fi
mkdir -p "$ROOT/data" "$ROOT/opt/fletch/tools" "$ROOT/Users" "$ROOT/Volumes"
chroot "$ROOT" useradd --uid 1000 --user-group --no-create-home \
  --home-dir /data/home --shell /bin/bash --password '*' agent
echo fletch-box > "$ROOT/etc/hostname"
printf '127.0.0.1\tlocalhost fletch-box\n::1\tlocalhost\n' > "$ROOT/etc/hosts"
: > "$ROOT/etc/resolv.conf"
rm -f "$ROOT"/etc/ssh/ssh_host_*
: > "$ROOT/etc/machine-id"
echo "fletch-box-image $VERSION ($ARCH, debian $SUITE @ $DEBIAN_SNAPSHOT, node $NODE_VERSION, layered spike)" > "$ROOT/etc/fletch-image"

clean_tree() {
  rm -rf "$1/var/cache/apt" "$1/var/lib/apt/lists" "$1/var/log"/* "$1/tmp"/* "$1/root/.bash_history"
  mkdir -p "$1/var/cache/apt" "$1/var/lib/apt/lists"
}
say "cleaning the base"
clean_tree "$ROOT"
pin_times "$ROOT"
chroot "$ROOT" /usr/bin/fc-cache -s -f
chroot "$ROOT" dpkg-query -W -f '${binary:Package}=${Version}\n' | sort > "$WORK/base.pkgs"
echo "node=$NODE_VERSION" >> "$WORK/base.pkgs"
BASE_TREE_BYTES=$(tree_size "$ROOT")

say "writing base images (new, old and big-cluster flags)"
mkimage "$UUID_BASE" "$NEW_FLAGS" "$OUT/base.erofs" "$ROOT"
mkimage "$UUID_BASE" "$NEW_FLAGS" "$WORK/base-again.erofs" "$ROOT"
mkimage "$UUID_BASE" "$OLD_FLAGS" "$OUT/base-oldflags.erofs" "$ROOT"
mkimage "$UUID_BASE" "$BIG_FLAGS" "$OUT/base-big.erofs" "$ROOT"
if cmp -s "$OUT/base.erofs" "$WORK/base-again.erofs"; then
  echo "repro=identical" > "$OUT/repro.txt"
else
  echo "repro=DIFFERENT" > "$OUT/repro.txt"
fi
rm -f "$WORK/base-again.erofs"

# ---------------------------------------------------------------- layers
layer_entries=""
for layer in $LAYERS; do
  LR="$WORK/$layer-root"
  D="$WORK/$layer-delta"
  rm -rf "$LR" "$D"
  say "building the $layer layer"
  cp -a "$ROOT" "$LR"
  mount -t proc proc "$LR/proc"
  # The snapshot's Release files are past Valid-Until, as for mmdebstrap above.
  chroot "$LR" sh -c "apt-get -qq -o Acquire::Check-Valid-Until=false -o Acquire::Retries=5 update \
    && apt-get -qq -o Acquire::Check-Valid-Until=false install -y --no-install-recommends $(pkgs "$layer" | tr , ' ')" \
    </dev/null
  umount "$LR/proc"
  if [ "$layer" = devtools ]; then
    cp -a "$WORK/node-include" "$LR/usr/local/include"
  fi
  clean_tree "$LR"
  chroot "$LR" /usr/bin/fc-cache -s -f
  chroot "$LR" dpkg-query -W -f '${binary:Package}=${Version}\n' | sort > "$WORK/$layer.allpkgs"
  comm -13 "$WORK/base.pkgs" "$WORK/$layer.allpkgs" > "$WORK/$layer.pkgs"

  # The delta: what the layer tree has that the base has not, byte for byte
  # (-c). The package databases, apt state and logs stay with the base; the
  # loader cache too, since the base's default search path covers the
  # multiarch directories (A7) and two layers could not share one cache.
  mkdir "$D"
  rsync -a -c --compare-dest="$ROOT/" \
    --exclude=/proc --exclude=/dev --exclude=/sys --exclude=/run --exclude=/tmp \
    --exclude=/etc/ld.so.cache \
    --exclude=/var/lib/dpkg --exclude=/var/lib/apt --exclude=/var/cache/apt \
    --exclude=/var/cache/debconf --exclude=/var/log --exclude=/var/backups \
    "$LR/" "$D/"
  find "$D" -depth -type d -empty -delete
  dirs=$(ls "$D" | tr '\n' ' ')
  say "$layer carries: $dirs"
  for d in $dirs; do
    case "$d" in usr|etc|var) ;; *) echo "build-layers.sh: the $layer layer touches /$d, which init does not overlay" >&2; exit 1;; esac
  done
  # What a layer puts under /var and /etc is worth a look in the log: only
  # caches and configuration should be there.
  { echo "$layer /var and /etc content:"; find "$D/var" "$D/etc" -maxdepth 3 2>/dev/null; } >> "$LOG"
  jq -n --arg name "$layer" --arg version "$VERSION" \
    --argjson dirs "$(printf '%s\n' $dirs | jq -R . | jq -s .)" \
    '{name: $name, version: $version, dirs: $dirs}' > "$D/.fletch-layer"
  pin_times "$D"
  eval "uuid=\$UUID_$(echo "$layer" | tr a-z A-Z)"
  mkimage "$uuid" "$NEW_FLAGS" "$OUT/layer-$layer.erofs" "$D"
  eval "${layer}_tree_bytes=$(tree_size "$D")"
  eval "${layer}_full_bytes=$(tree_size "$LR")"
  layer_entries="$layer_entries $layer"
done

# ---------------------------------------------------------------- delta variant
say "writing base-v2 (one new file, two edited) for the chunk-delta test"
V2="$WORK/base-v2"
rm -rf "$V2"
cp -a "$ROOT" "$V2"
mkdir -p "$V2/usr/share/fletch-spike"
python3 -c 'import random,sys; r=random.Random(7); sys.stdout.buffer.write(bytes(r.getrandbits(8) for _ in range(1<<20)))' \
  > "$V2/usr/share/fletch-spike/blob"
echo "fletch-box-image $VERSION+1 (delta variant)" > "$V2/etc/fletch-image"
# An edit in the middle of a large text file, as a package update would make.
sed -i '1000s/^/# spike edit\n/' "$V2/usr/share/zoneinfo/tzdata.zi" 2>/dev/null || echo "spike" >> "$V2/usr/share/zoneinfo/tzdata.zi"
pin_times "$V2"
mkimage "$UUID_V2" "$NEW_FLAGS" "$OUT/base-v2.erofs" "$V2"
rm -rf "$V2"

# ---------------------------------------------------------------- manifest and sizes
say "writing manifest.json and sizes.md"
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
  for layer in $layer_entries; do
    eval "tb=\$${layer}_tree_bytes"
    entry "$layer" "layer-$layer.erofs" "$(jq -c .dirs "$WORK/$layer-delta/.fletch-layer")" "$WORK/$layer.pkgs" "$tb"
  done
} | jq -s --arg version "$VERSION" --arg arch "$ARCH" '{version: $version, arch: $arch, images: .}' > "$OUT/manifest.json"

size_of() { stat -c %s "$OUT/$1"; }
{
  echo "## Sizes"
  echo
  echo "| Image | Tree (uncompressed) | Packed | Ratio |"
  echo "| --- | ---: | ---: | ---: |"
  row() { # <label> <file> <tree bytes>
    s=$(size_of "$2")
    printf '| %s | %s MB | %s MB | %s |\n' "$1" "$(mib "$3")" "$(mib "$s")" "$(awk "BEGIN { printf \"%.2f\", $s / $3 }")"
  }
  row "base, today's flags (lz4hc, 4 KiB)" base-oldflags.erofs "$BASE_TREE_BYTES"
  row "base, new flags (lz4hc,12 64 KiB dedupe fragments ztailpacking)" base.erofs "$BASE_TREE_BYTES"
  row "base, 256 KiB clusters" base-big.erofs "$BASE_TREE_BYTES"
  for layer in $layer_entries; do
    eval "tb=\$${layer}_tree_bytes"; eval "fb=\$${layer}_full_bytes"
    row "layer $layer (delta; full tree would be $(mib "$fb") MB)" "layer-$layer.erofs" "$tb"
  done
  echo
  echo "Reference: image 0.2.0 is 810 MB packed from a 1498 MB tree (ratio 0.54)."
  echo
  echo "Default download (base + devtools): $(mib $(( $(size_of base.erofs) + $(size_of layer-devtools.erofs) )) ) MB;"
  echo "everything: $(mib $(( $(size_of base.erofs) + $(size_of layer-devtools.erofs) + $(size_of layer-browser.erofs) )) ) MB."
  echo
  echo "Reproducible build (mkfs twice on the base tree): $(cat "$OUT/repro.txt")"
  echo
  echo "### Packages per image"
  echo
  for img in base $layer_entries; do
    f="$WORK/$img.pkgs"
    echo "- $img: $(wc -l < "$f") packages"
  done
  echo
  echo "<details><summary>devtools and browser package lists</summary>"
  echo
  for layer in $layer_entries; do
    echo "**$layer**: $(paste -sd' ' "$WORK/$layer.pkgs")"
    echo
  done
  echo "</details>"
} > "$OUT/sizes.md"
cat "$OUT/sizes.md" | tee -a "$LOG"
say "done: $OUT"
