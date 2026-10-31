#!/bin/sh
# The GitHub release of a built image, `box-image-v<VERSION>`: manifest.json,
# the images, their delta manifests, sizes.md, and inputs.json. Idempotent on
# one rule: a version names one set of inputs. The inputs are the git tree ids
# of packaging/box-image and crates/fletch-guest at HEAD (what build.sh reads),
# recorded in the release as inputs.json.
#
#   publish.sh OUT_DIR VERSION     no release: publish it. The same inputs
#                                  already released: nothing to do, exit 0.
#                                  Other inputs under this version: exit 1.
#   publish.sh --check VERSION     the same verdicts without publishing, for a
#                                  pull request: a change to the image without
#                                  a VERSION bump fails here, not on main.
#
# Needs `gh` logged in (GH_TOKEN in CI) and the repository checked out.
set -eu

inputs() {
  printf '{"box_image":"%s","fletch_guest":"%s"}\n' \
    "$(git rev-parse HEAD:packaging/box-image)" "$(git rev-parse HEAD:crates/fletch-guest)"
}
# none | same | different | unknown (a release without inputs.json)
status() {
  tag="box-image-v$1"
  gh release view "$tag" >/dev/null 2>&1 || { echo none; return; }
  remote=$(gh release download "$tag" --pattern inputs.json --output - 2>/dev/null || true)
  [ -n "$remote" ] || { echo unknown; return; }
  if [ "$remote" = "$(inputs)" ]; then echo same; else echo different; fi
}
verdict() { # <version> <status>: prints the sentence; exits 1 when the version must be bumped
  case "$2" in
    none) echo "no release box-image-v$1 yet";;
    same) echo "release box-image-v$1 holds these inputs";;
    different)
      echo "::error::release box-image-v$1 exists with other inputs; bump packaging/box-image/VERSION (a version names one set of inputs)"
      exit 1;;
    unknown)
      echo "::error::release box-image-v$1 exists without inputs.json (published before this check); bump packaging/box-image/VERSION"
      exit 1;;
  esac
}

if [ "${1:-}" = --check ]; then
  VERSION=${2:?usage: publish.sh --check VERSION}
  verdict "$VERSION" "$(status "$VERSION")"
  exit 0
fi

OUT=${1:?usage: publish.sh OUT_DIR VERSION | --check VERSION}
VERSION=${2:?usage: publish.sh OUT_DIR VERSION}
TAG="box-image-v$VERSION"
M="$OUT/manifest.json"
[ -f "$M" ] || { echo "publish.sh: no $M" >&2; exit 1; }
[ "$(jq -r .version "$M")" = "$VERSION" ] || { echo "publish.sh: $M is version $(jq -r .version "$M"), not $VERSION" >&2; exit 1; }
for f in $(jq -r '.images[].file' "$M"); do
  [ -f "$OUT/$f" ] || { echo "publish.sh: $M lists $f, which is not in $OUT" >&2; exit 1; }
  [ -f "$OUT/$f.chunks.json" ] || echo "publish.sh: no $f.chunks.json; updates to this file will not be deltas" >&2
done
STATUS=$(status "$VERSION")
verdict "$VERSION" "$STATUS"
[ "$STATUS" = none ] || exit 0

inputs > "$OUT/inputs.json"
NOTES=$(mktemp)
{
  echo "Box image for Fletch (\`packaging/box-image\`): the root filesystem every box boots and the layers it overlays. See docs/box.md."
  echo
  cat "$OUT/sizes.md"
  echo
  echo "The app boots this image once \`PinnedImage::VERSION\` in \`crates/fletch-box/src/image_store/mod.rs\` is \`$VERSION\`. Each file is verified against \`manifest.json\`; a host with an older image fetches only the chunks that changed (\`*.chunks.json\`). \`inputs.json\` names the source trees this was built from."
  echo
  echo "Package lists with versions are in \`manifest.json\`; each package's licence is in the image under \`/usr/share/doc/<package>/copyright\`."
} > "$NOTES"

# --latest=false: the app updater's endpoint serves the repository's *latest*
# release (docs/RELEASING.md); an image release must never become it.
set -f
gh release create "$TAG" --target "${GITHUB_SHA:-HEAD}" --title "Box image v$VERSION" \
  --notes-file "$NOTES" --latest=false \
  "$M" "$OUT/sizes.md" "$OUT/inputs.json" $(ls "$OUT"/*.erofs "$OUT"/*.chunks.json 2>/dev/null)
set +f
rm -f "$NOTES"
echo "published $TAG"
