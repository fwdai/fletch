#!/bin/sh
# Publish a built image directory as the GitHub release `box-image-v<VERSION>`:
# manifest.json, the images, their delta manifests and sizes.md, with release
# notes from the manifest. Refuses a version that has a release: an app that
# holds one file under a version treats it as installed, so a version never
# names two different builds. Needs `gh` logged in (GH_TOKEN in CI).
#
#   publish.sh OUT_DIR VERSION
set -eu
OUT=${1:?usage: publish.sh OUT_DIR VERSION}
VERSION=${2:?usage: publish.sh OUT_DIR VERSION}
TAG="box-image-v$VERSION"
M="$OUT/manifest.json"
[ -f "$M" ] || { echo "publish.sh: no $M" >&2; exit 1; }
[ "$(jq -r .version "$M")" = "$VERSION" ] || { echo "publish.sh: $M is version $(jq -r .version "$M"), not $VERSION" >&2; exit 1; }
if gh release view "$TAG" >/dev/null 2>&1; then
  echo "publish.sh: release $TAG exists; images are never republished under a version" >&2
  exit 1
fi
for f in $(jq -r '.images[].file' "$M"); do
  [ -f "$OUT/$f" ] || { echo "publish.sh: $M lists $f, which is not in $OUT" >&2; exit 1; }
  [ -f "$OUT/$f.chunks.json" ] || echo "publish.sh: no $f.chunks.json; updates to this file will not be deltas" >&2
done

NOTES=$(mktemp)
{
  echo "Box image for Fletch (\`packaging/box-image\`): the root filesystem every box boots and the layers it overlays. See docs/box.md."
  echo
  cat "$OUT/sizes.md"
  echo
  echo "The app boots this image once \`PinnedImage::VERSION\` in \`crates/fletch-box/src/image_store/mod.rs\` is \`$VERSION\`. Each file is verified against \`manifest.json\`; a host with an older image fetches only the chunks that changed (\`*.chunks.json\`)."
  echo
  echo "Package lists with versions are in \`manifest.json\`; each package's licence is in the image under \`/usr/share/doc/<package>/copyright\`."
} > "$NOTES"

# --latest=false: the app updater's endpoint serves the repository's *latest*
# release (docs/RELEASING.md); an image release must never become it.
set -f
gh release create "$TAG" --target "${GITHUB_SHA:-HEAD}" --title "Box image v$VERSION" \
  --notes-file "$NOTES" --latest=false \
  "$M" "$OUT/sizes.md" $(ls "$OUT"/*.erofs "$OUT"/*.chunks.json 2>/dev/null)
set +f
rm -f "$NOTES"
echo "published $TAG"
