#!/bin/sh
# Builds dist/ with build.sh unless it was already built from the same build.sh
# and patches. The bundle hook (packaging/box-bundle/prepare.sh) runs this;
# run build.sh directly to force a rebuild.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
DIST="$HERE/dist"
STAMP="$DIST/.inputs"

# A sha256 over the names and contents of build.sh and every patch.
hash=$(cd "$HERE" && find build.sh patches -type f | LC_ALL=C sort |
  while IFS= read -r f; do shasum -a 256 "$f"; done | shasum -a 256 | cut -d' ' -f1)
if [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$hash" ] &&
  [ -f "$DIST/libkrun.1.dylib" ] && [ -f "$DIST/libkrunfw.5.dylib" ]; then
  echo "libkrun: up to date ($DIST)"
  exit 0
fi

echo "== building libkrun (about a minute the first time)"
case $(xcode-select -p 2> /dev/null) in
  *Xcode*.app/*) ;;
  *)
    echo "libkrun: needs Xcode (not just the Command Line Tools) for libclang:" >&2
    echo "  install Xcode, then: sudo xcode-select -s /Applications/Xcode.app" >&2
    exit 1
    ;;
esac
command -v cargo > /dev/null || { echo "libkrun: needs Rust (https://rustup.rs)" >&2; exit 1; }
"$HERE/build.sh"
echo "$hash" > "$STAMP"
