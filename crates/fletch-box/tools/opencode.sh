#!/bin/sh
# Installs OpenCode onto a tools disk; see claude.sh for the contract. The CLI
# ends up at /opt/fletch/tools/opencode/bin/opencode.
#
#   opencode.sh <version>
set -eu
VERSION=${1:?usage: opencode.sh <version>}
PREFIX=/data/tools/opencode

mkdir -p "$PREFIX"
npm install -g --prefix "$PREFIX" --cache /tmp/npm-cache --no-fund --no-audit \
  "opencode-ai@$VERSION"
"$PREFIX/bin/opencode" --version
