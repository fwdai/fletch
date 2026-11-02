#!/bin/sh
# The GitHub release of a built image, `box-image-v<VERSION>` for aarch64 and
# `box-image-v<VERSION>-<arch>` for any other architecture (x86_64): the
# manifest's `arch` picks it, and each architecture's release is published on
# its own. It holds manifest.json, the images, their delta manifests, sizes.md,
# and inputs.json. Idempotent on
# one rule: a version names one set of inputs. The inputs are the git ids at
# HEAD of what the image is made from ($INPUTS: what build.sh reads, and the
# guest agent), recorded in the release as inputs.json. The scripts around it
# (this one, test.sh, the README) are not inputs: changing them needs no bump.
#
# The release goes up as a draft, which the app cannot see, and is published
# only once it carries every file at its size: a published release is one the
# app can install. A draft a failed publish left behind is replaced.
#
#   publish.sh OUT_DIR VERSION     no release: publish it. The same inputs
#                                  already released: nothing to do, exit 0.
#                                  Other inputs under this version, or a
#                                  release missing files: exit 1.
#   publish.sh --check VERSION [ARCH]
#                                  the same verdicts without publishing (ARCH
#                                  defaults to aarch64), for a pull request: a
#                                  change to the image without a VERSION bump
#                                  fails here, not on main.
#
# Needs `gh` logged in (GH_TOKEN in CI) and the repository checked out.
set -eu

INPUTS="packaging/box-image/build.sh packaging/box-image/packages packaging/box-image/dpkg-excludes.cfg
  packaging/box-image/rootfs crates/fletch-guest"
inputs() {
  sep='{'
  for p in $INPUTS; do
    printf '%s"%s":"%s"' "$sep" "$p" "$(git rev-parse "HEAD:$p")"
    sep=,
  done
  echo '}'
}
# Reads `<name> <size>` lines; prints each name the release $1 does not carry
# as an uploaded asset of that size (every name when its assets cannot be read).
absent() {
  assets=$(gh release view "$1" --json assets \
    -q '.assets[] | select((.state // "uploaded") == "uploaded") | "\(.name) \(.size)"') || assets=
  while read -r name size; do
    printf '%s\n' "$assets" | grep -qxF "$name $size" || echo "$name"
  done
}
# The release tag of <version> <arch>; aarch64's predates every other
# architecture's and carries no suffix (PinnedImage's release_url agrees).
tag_of() {
  case "$2" in
    aarch64) echo "box-image-v$1" ;;
    *) echo "box-image-v$1-$2" ;;
  esac
}
# <tag>: none | draft | same | incomplete (these inputs, but a file its manifest
# lists is missing) | different | unknown (a release without inputs.json)
status() {
  tag=$1
  draft=$(gh release view "$tag" --json isDraft -q .isDraft 2>/dev/null) || { echo none; return; }
  [ "$draft" = false ] || { echo draft; return; }
  remote=$(gh release download "$tag" --pattern inputs.json --output - 2>/dev/null || true)
  [ -n "$remote" ] || { echo unknown; return; }
  [ "$remote" = "$(inputs)" ] || { echo different; return; }
  files=$(gh release download "$tag" --pattern manifest.json --output - 2>/dev/null |
    jq -r '.images[] | "\(.file) \(.size)"' 2>/dev/null || true)
  if [ -n "$files" ] && [ -z "$(printf '%s\n' "$files" | absent "$tag")" ]; then
    echo same
  else
    echo incomplete
  fi
}
verdict() { # <tag> <status>: prints the sentence; exits 1 when the version must be bumped
  case "$2" in
    none) echo "no release $1 yet";;
    draft) echo "$1 is a draft left by a publish that did not finish; publishing replaces it";;
    same) echo "release $1 holds these inputs";;
    incomplete)
      echo "::error::release $1 lacks files its manifest.json lists, so the app cannot install it; bump packaging/box-image/VERSION"
      exit 1;;
    different)
      echo "::error::release $1 exists with other inputs; bump packaging/box-image/VERSION (a version names one set of inputs)"
      exit 1;;
    unknown)
      echo "::error::release $1 exists without inputs.json (published before this check); bump packaging/box-image/VERSION"
      exit 1;;
  esac
}

if [ "${1:-}" = --check ]; then
  VERSION=${2:?usage: publish.sh --check VERSION [ARCH]}
  TAG=$(tag_of "$VERSION" "${3:-aarch64}")
  verdict "$TAG" "$(status "$TAG")"
  exit 0
fi

OUT=${1:?usage: publish.sh OUT_DIR VERSION | --check VERSION [ARCH]}
VERSION=${2:?usage: publish.sh OUT_DIR VERSION}
M="$OUT/manifest.json"
[ -f "$M" ] || { echo "publish.sh: no $M" >&2; exit 1; }
[ "$(jq -r .version "$M")" = "$VERSION" ] || { echo "publish.sh: $M is version $(jq -r .version "$M"), not $VERSION" >&2; exit 1; }
ARCH=$(jq -r .arch "$M")
case "$ARCH" in
  aarch64 | x86_64) ;;
  *) echo "publish.sh: $M is for arch '$ARCH'; expected aarch64 or x86_64" >&2; exit 1 ;;
esac
TAG=$(tag_of "$VERSION" "$ARCH")
[ -f "$OUT/sizes.md" ] || { echo "publish.sh: no $OUT/sizes.md" >&2; exit 1; }
# What the release carries, as the positional parameters, each file named: the
# manifest, sizes.md, inputs.json (written below), then every image the
# manifest lists and its chunk list.
set -- "$M" "$OUT/sizes.md" "$OUT/inputs.json"
for f in $(jq -r '.images[].file' "$M"); do
  [ -f "$OUT/$f" ] || { echo "publish.sh: $M lists $f, which is not in $OUT" >&2; exit 1; }
  set -- "$@" "$OUT/$f"
  if [ -f "$OUT/$f.chunks.json" ]; then
    set -- "$@" "$OUT/$f.chunks.json"
  else
    echo "publish.sh: no $f.chunks.json; updates to this file will not be deltas" >&2
  fi
done
[ $# -gt 3 ] || { echo "publish.sh: $M lists no images" >&2; exit 1; }
STATUS=$(status "$TAG")
verdict "$TAG" "$STATUS"
case "$STATUS" in
  none) ;;
  draft) gh release delete "$TAG" --yes ;;
  *) exit 0 ;;
esac

# Fails with $1, removing the draft if there is one (never a published release).
abandon() {
  if [ "$(gh release view "$TAG" --json isDraft -q .isDraft 2>/dev/null || true)" = true ]; then
    gh release delete "$TAG" --yes || true
    echo "::error::$1; the draft is removed, nothing was published"
  else
    echo "::error::$1"
  fi
  exit 1
}

inputs > "$OUT/inputs.json"
NOTES=$(mktemp)
trap 'rm -f "$NOTES"' EXIT
{
  echo "Box image for Fletch (\`packaging/box-image\`), $ARCH: the root filesystem every box boots and the layers it overlays. See docs/box.md."
  echo
  cat "$OUT/sizes.md"
  echo
  echo "The app boots this image once \`PinnedImage::VERSION\` in \`crates/fletch-box/src/image_store/mod.rs\` is \`$VERSION\`. Each file is verified against \`manifest.json\`; a host with an older image fetches only the chunks that changed (\`*.chunks.json\`). \`inputs.json\` names the source trees this was built from."
  echo
  echo "Package lists with versions are in \`manifest.json\`; each package's licence is in the image under \`/usr/share/doc/<package>/copyright\`."
} > "$NOTES"

# --latest=false, on the draft and again on publishing it: the app updater's
# endpoint serves the repository's *latest* release (docs/RELEASING.md); an
# image release must never become it.
gh release create "$TAG" --draft --target "${GITHUB_SHA:-$(git rev-parse HEAD)}" \
  --title "Box image ${TAG#box-image-}" --notes-file "$NOTES" --latest=false "$@" ||
  abandon "creating the draft of $TAG failed"
lacking=$(for f in "$@"; do echo "${f##*/} $(($(wc -c < "$f")))"; done | absent "$TAG")
[ -z "$lacking" ] || abandon "the draft of $TAG lacks $(printf '%s' "$lacking" | tr '\n' ' ')"
gh release edit "$TAG" --draft=false --latest=false || abandon "publishing $TAG failed"
echo "published $TAG: $# files"
