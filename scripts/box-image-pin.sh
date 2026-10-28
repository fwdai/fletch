#!/bin/sh
# Pins a box image into the app: rewrites PinnedImage::{VERSION, SIZE, SHA256}
# in crates/fletch-box/src/image_store/mod.rs from an image manifest.json
# (packaging/box-image/README.md, "Publishing a release").
#
#   scripts/box-image-pin.sh box-image-v0.1.0                       # a published release (gh)
#   scripts/box-image-pin.sh packaging/box-image/out/manifest.json  # a local build
#
# Then commit the change; `cargo test --manifest-path crates/fletch-box/Cargo.toml`
# checks the pin's shape.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
FILE="$ROOT/crates/fletch-box/src/image_store/mod.rs"
SRC=${1:?usage: box-image-pin.sh <manifest.json | box-image-v<version>>}

if [ -f "$SRC" ]; then
  MANIFEST=$SRC
else
  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT
  # gh takes the repository from this checkout's remote.
  (cd "$ROOT" && gh release download "$SRC" --pattern manifest.json --dir "$TMP")
  MANIFEST="$TMP/manifest.json"
fi

VERSION=$(jq -r .version "$MANIFEST")
SIZE=$(jq -r .size "$MANIFEST")
SHA256=$(jq -r .sha256 "$MANIFEST" | tr 'A-F' 'a-f')
echo "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' || { echo "pin: bad version '$VERSION'" >&2; exit 1; }
echo "$SIZE" | grep -Eq '^[1-9][0-9]*$' || { echo "pin: bad size '$SIZE'" >&2; exit 1; }
echo "$SHA256" | grep -Eq '^[0-9a-f]{64}$' || { echo "pin: bad sha256 '$SHA256'" >&2; exit 1; }
# 848302080 -> 848_302_080, the way the constant is written.
SIZE_RS=$(echo "$SIZE" | rev | sed 's/\([0-9]\{3\}\)/\1_/g; s/_$//' | rev)

export VERSION SIZE_RS SHA256
perl -0pi -e '
  s/(pub const VERSION: &\x27static str = ")[^"]*(";)/$1$ENV{VERSION}$2/;
  s/(pub const SIZE: u64 = )[0-9_]+(;)/$1$ENV{SIZE_RS}$2/;
  s/(pub const SHA256: &\x27static str =\s*")[0-9a-fA-F]*(";)/$1$ENV{SHA256}$2/;
' "$FILE"

if ! grep -q "VERSION: &'static str = \"$VERSION\"" "$FILE" ||
  ! grep -q "SIZE: u64 = $SIZE_RS;" "$FILE" ||
  ! grep -q "\"$SHA256\"" "$FILE"; then
  echo "pin: $FILE was not rewritten as expected" >&2
  exit 1
fi
echo "pinned box image $VERSION ($SIZE bytes, sha256 $SHA256) in ${FILE#"$ROOT"/}"
