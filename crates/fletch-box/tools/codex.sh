#!/bin/sh
# Installs Codex onto a tools disk; see claude.sh for the contract. The CLI ends
# up at /opt/fletch/tools/codex/bin/codex.
#
#   codex.sh <version>
set -eu
VERSION=${1:?usage: codex.sh <version>}
PREFIX=/data/tools/codex

mkdir -p "$PREFIX"
npm install -g --prefix "$PREFIX" --cache /tmp/npm-cache --no-fund --no-audit \
  "@openai/codex@$VERSION"
"$PREFIX/bin/codex" --version
