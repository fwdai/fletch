#!/bin/sh
# The fingerprint of what the cloud image is made from in this directory:
# twelve hex digits over the names and contents of the files that decide what
# the image holds and how it starts. It is the second half of the image's tag
# (<box image version>-<fingerprint>), so a change to any of them is another
# tag, which the registry does not have yet and so gets; nothing has to be
# bumped by hand, and no change can keep a tag that names other bytes.
#
#   inputs.sh [DIR]     DIR holds the files; this directory by default
#
# The inputs: build.sh (the root's content), entrypoint, and build-docker.sh
# (the image's config: its entrypoint and PATH). test.sh, publish.sh and the
# README decide nothing the image contains. The box image is the other input,
# named by its own version, which names one set of its inputs.
set -eu

DIR=${1:-$(cd "$(dirname "$0")" && pwd)}
if command -v sha256sum >/dev/null 2>&1; then
  sha() { sha256sum | cut -d' ' -f1; }
else
  sha() { shasum -a 256 | cut -d' ' -f1; }
fi
for file in build.sh build-docker.sh entrypoint; do
  [ -f "$DIR/$file" ] || { echo "inputs.sh: no $DIR/$file" >&2; exit 1; }
  printf '%s %s\n' "$file" "$(sha < "$DIR/$file")"
done | sha | cut -c1-12
