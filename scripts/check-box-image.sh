#!/bin/sh
# The box image release the app pins can be downloaded: its manifest.json
# names the pinned version, and every file it lists is there at its size and
# serves byte ranges (what a delta update fetches). The app downloads this
# release on first use of a box, so an app release must not ship pinned to one
# that is missing files; the release workflow runs this before building. Sizes
# only: the build was installed with the app's own code, hashes included,
# before it was published (packaging/box-image/build-docker.sh).
#
#   scripts/check-box-image.sh            the pin, PinnedImage::VERSION
#   scripts/check-box-image.sh VERSION    another version
set -eu
cd "$(dirname "$0")/.."
version=${1:-$(sed -n 's/^ *pub const VERSION: &'"'"'static str = "\([^"]*\)";.*/\1/p' crates/fletch-box/src/image_store/mod.rs)}
[ -n "$version" ] || { echo "error: no PinnedImage::VERSION found in crates/fletch-box/src/image_store/mod.rs" >&2; exit 1; }
url="https://github.com/fwdai/fletch/releases/download/box-image-v$version"
manifest=$(curl -fsSL --retry 3 "$url/manifest.json") || {
  echo "error: the app pins box image $version, but $url/manifest.json does not download; publish it first (docs/box.md, \"Releases\")" >&2
  exit 1
}
[ "$(printf '%s' "$manifest" | jq -r .version)" = "$version" ] || { echo "error: $url/manifest.json is not version $version" >&2; exit 1; }
entries=$(printf '%s' "$manifest" | jq -er '.images[] | "\(.file)=\(.size)"') ||
  { echo "error: $url/manifest.json lists no images" >&2; exit 1; }
status=0
for entry in $entries; do
  file=${entry%%=*}
  size=${entry#*=}
  # One byte by range: the last Content-Range (after GitHub's redirect) ends
  # with the file's size.
  got=$(curl -fsSL --retry 3 -r 0-0 -o /dev/null -D - "$url/$file" 2>/dev/null | tr -d '\r' |
    sed -n 's|^[Cc]ontent-[Rr]ange: bytes 0-0/\([0-9]*\)$|\1|p' | tail -n 1)
  if [ "$got" = "$size" ]; then
    echo "box image $version: $file, $size bytes"
  else
    found=${got:+$got bytes}
    echo "error: box image $version: $url/$file is ${found:-missing}; the manifest says $size bytes" >&2
    status=1
  fi
done
exit $status
