#!/bin/sh
# Installs Pi onto a tools disk; see claude.sh for the contract. The CLI ends
# up at /opt/fletch/tools/pi/bin/pi.
#
#   pi.sh <version>
set -eu
VERSION=${1:?usage: pi.sh <version>}
PREFIX=/data/tools/pi

mkdir -p "$PREFIX"
npm install -g --prefix "$PREFIX" --cache /tmp/npm-cache --no-fund --no-audit \
  "@earendil-works/pi-coding-agent@$VERSION"
"$PREFIX/bin/pi" --version
