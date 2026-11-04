#!/bin/sh
# Move the image's dependency pins to the current upstream state and bump the
# image version: what the monthly "Box image bump" workflow runs before it
# builds, tests, publishes and opens its pull request. Safe to run by hand.
#
#   packaging/box-image/bump.sh            # edit build.sh, VERSION and the app's pin
#   packaging/box-image/bump.sh --check    # print what would change, change nothing
#
# Pins moved:
#   DEBIAN_SNAPSHOT   snapshot.debian.org timestamp: yesterday 00:00 UTC (the
#                     latest snapshot is always available for that time)
#   NODE_VERSION      the newest Node 22 (LTS) release on nodejs.org
#   NODE_SHA256_ARM64 its linux-arm64 and linux-x64 tarballs' entries in
#   NODE_SHA256_X64   SHASUMS256.txt
#   BUN_VERSION       the newest Bun release on GitHub
#   BUN_SHA256_ARM64  its linux-aarch64 and linux-x64-baseline zips' entries
#   BUN_SHA256_X64    in the release's SHASUMS256.txt
#   VERSION           patch bump when a pin moved (X.Y.Z -> X.Y.Z+1)
#   PinnedImage::VERSION in crates/fletch-box/src/image_store/mod.rs -> VERSION
#
# Prints `changed=yes|no` last; exits 0 either way.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
BUILD="$HERE/build.sh"
PIN_RS="$REPO/crates/fletch-box/src/image_store/mod.rs"
CHECK=0
[ "${1:-}" = --check ] && CHECK=1

current() { sed -n "s/^$1=\${$1:-\(.*\)}$/\1/p; s/^$1=\([^$].*\)$/\1/p" "$BUILD" | head -1; }
cur_snapshot=$(current DEBIAN_SNAPSHOT)
cur_node=$(current NODE_VERSION)
cur_sha_arm64=$(current NODE_SHA256_ARM64)
cur_sha_x64=$(current NODE_SHA256_X64)
cur_bun=$(current BUN_VERSION)
cur_bun_sha_arm64=$(current BUN_SHA256_ARM64)
cur_bun_sha_x64=$(current BUN_SHA256_X64)
[ -n "$cur_snapshot" ] && [ -n "$cur_node" ] && [ -n "$cur_sha_arm64" ] && [ -n "$cur_sha_x64" ] &&
  [ -n "$cur_bun" ] && [ -n "$cur_bun_sha_arm64" ] && [ -n "$cur_bun_sha_x64" ] ||
  { echo "bump.sh: cannot read the pins in $BUILD" >&2; exit 1; }

new_snapshot=$(date -u -d yesterday +%Y%m%dT000000Z 2>/dev/null || date -u -v-1d +%Y%m%dT000000Z)
index=$(curl -fsSL https://nodejs.org/dist/index.json)
new_node=$(echo "$index" | jq -r '[.[] | select(.version | startswith("v22.")) | select(.lts != false)] | .[0].version | ltrimstr("v")')
[ -n "$new_node" ] && [ "$new_node" != null ] || { echo "bump.sh: no Node 22 LTS release in nodejs.org/dist/index.json" >&2; exit 1; }
sums=$(curl -fsSL "https://nodejs.org/dist/v$new_node/SHASUMS256.txt")
node_sha() { # <node arch>
  sha=$(echo "$sums" | awk -v f="node-v$new_node-linux-$1.tar.xz" '$2 == f { print $1 }')
  [ -n "$sha" ] || { echo "bump.sh: no sha256 for node-v$new_node-linux-$1.tar.xz" >&2; exit 1; }
  echo "$sha"
}
new_sha_arm64=$(node_sha arm64)
new_sha_x64=$(node_sha x64)

# Bun: the newest release's tag is `bun-vX.Y.Z`; its SHASUMS256.txt names the
# zips build.sh installs (the x64 one the baseline build, see build.sh).
new_bun=$(curl -fsSL https://api.github.com/repos/oven-sh/bun/releases/latest | jq -r '.tag_name | ltrimstr("bun-v")')
[ -n "$new_bun" ] && [ "$new_bun" != null ] || { echo "bump.sh: no latest Bun release on GitHub" >&2; exit 1; }
bun_sums=$(curl -fsSL "https://github.com/oven-sh/bun/releases/download/bun-v$new_bun/SHASUMS256.txt")
bun_sha() { # <bun arch>
  sha=$(echo "$bun_sums" | awk -v f="bun-linux-$1.zip" '$2 == f { print $1 }')
  [ -n "$sha" ] || { echo "bump.sh: no sha256 for bun-linux-$1.zip in Bun $new_bun" >&2; exit 1; }
  echo "$sha"
}
new_bun_sha_arm64=$(bun_sha aarch64)
new_bun_sha_x64=$(bun_sha x64-baseline)

echo "DEBIAN_SNAPSHOT $cur_snapshot -> $new_snapshot"
echo "NODE_VERSION    $cur_node -> $new_node"
echo "BUN_VERSION     $cur_bun -> $new_bun"
changed=no
[ "$cur_snapshot" != "$new_snapshot" ] && changed=yes
[ "$cur_node" != "$new_node" ] && changed=yes
[ "$cur_bun" != "$new_bun" ] && changed=yes
if [ "$changed" = yes ] && [ "$CHECK" = 0 ]; then
  sed -i.bak \
    -e "s/^DEBIAN_SNAPSHOT=\${DEBIAN_SNAPSHOT:-$cur_snapshot}/DEBIAN_SNAPSHOT=\${DEBIAN_SNAPSHOT:-$new_snapshot}/" \
    -e "s/^NODE_VERSION=$cur_node/NODE_VERSION=$new_node/" \
    -e "s/^NODE_SHA256_ARM64=$cur_sha_arm64/NODE_SHA256_ARM64=$new_sha_arm64/" \
    -e "s/^NODE_SHA256_X64=$cur_sha_x64/NODE_SHA256_X64=$new_sha_x64/" \
    -e "s/^BUN_VERSION=$cur_bun/BUN_VERSION=$new_bun/" \
    -e "s/^BUN_SHA256_ARM64=$cur_bun_sha_arm64/BUN_SHA256_ARM64=$new_bun_sha_arm64/" \
    -e "s/^BUN_SHA256_X64=$cur_bun_sha_x64/BUN_SHA256_X64=$new_bun_sha_x64/" "$BUILD"
  rm -f "$BUILD.bak"
  old_version=$(cat "$HERE/VERSION")
  new_version=$(echo "$old_version" | awk -F. '{ printf "%d.%d.%d", $1, $2, $3 + 1 }')
  echo "$new_version" > "$HERE/VERSION"
  sed -i.bak "s/pub const VERSION: &'static str = \"$old_version\";/pub const VERSION: \&'static str = \"$new_version\";/" "$PIN_RS"
  rm -f "$PIN_RS.bak"
  grep -q "\"$new_version\"" "$PIN_RS" || { echo "bump.sh: PinnedImage::VERSION was not $old_version; fix $PIN_RS by hand" >&2; exit 1; }
  echo "VERSION         $old_version -> $new_version (and PinnedImage::VERSION)"
fi
echo "changed=$changed"
