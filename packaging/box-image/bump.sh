#!/bin/sh
# Move the image's dependency pins to the current upstream state and bump the
# image version: what the weekly "Box image bump" workflow runs before it
# builds, tests, publishes and opens its pull request. Safe to run by hand.
#
#   packaging/box-image/bump.sh            # edit build.sh, VERSION and the app's pin
#   packaging/box-image/bump.sh --check    # print what would change, change nothing
#
# Pins moved:
#   DEBIAN_SNAPSHOT   snapshot.debian.org timestamp: yesterday 00:00 UTC (the
#                     latest snapshot is always available for that time)
#   NODE_VERSION      the newest Node 22 (LTS) release on nodejs.org
#   NODE_SHA256       its linux-arm64 tarball's entry in SHASUMS256.txt
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
cur_sha=$(current NODE_SHA256)
[ -n "$cur_snapshot" ] && [ -n "$cur_node" ] && [ -n "$cur_sha" ] || { echo "bump.sh: cannot read the pins in $BUILD" >&2; exit 1; }

new_snapshot=$(date -u -d yesterday +%Y%m%dT000000Z 2>/dev/null || date -u -v-1d +%Y%m%dT000000Z)
index=$(curl -fsSL https://nodejs.org/dist/index.json)
new_node=$(echo "$index" | jq -r '[.[] | select(.version | startswith("v22.")) | select(.lts != false)] | .[0].version | ltrimstr("v")')
[ -n "$new_node" ] && [ "$new_node" != null ] || { echo "bump.sh: no Node 22 LTS release in nodejs.org/dist/index.json" >&2; exit 1; }
new_sha=$(curl -fsSL "https://nodejs.org/dist/v$new_node/SHASUMS256.txt" | awk -v f="node-v$new_node-linux-arm64.tar.xz" '$2 == f { print $1 }')
[ -n "$new_sha" ] || { echo "bump.sh: no sha256 for node-v$new_node-linux-arm64.tar.xz" >&2; exit 1; }

echo "DEBIAN_SNAPSHOT $cur_snapshot -> $new_snapshot"
echo "NODE_VERSION    $cur_node -> $new_node"
changed=no
[ "$cur_snapshot" != "$new_snapshot" ] && changed=yes
[ "$cur_node" != "$new_node" ] && changed=yes
if [ "$changed" = yes ] && [ "$CHECK" = 0 ]; then
  sed -i.bak \
    -e "s/^DEBIAN_SNAPSHOT=\${DEBIAN_SNAPSHOT:-$cur_snapshot}/DEBIAN_SNAPSHOT=\${DEBIAN_SNAPSHOT:-$new_snapshot}/" \
    -e "s/^NODE_VERSION=$cur_node/NODE_VERSION=$new_node/" \
    -e "s/^NODE_SHA256=$cur_sha/NODE_SHA256=$new_sha/" "$BUILD"
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
