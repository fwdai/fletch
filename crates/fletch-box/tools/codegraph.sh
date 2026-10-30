#!/bin/sh
# Installs codegraph, Fletch's code index, onto a tools disk next to the
# provider CLI. The host has already downloaded the release bundle and checked
# it against its pinned SHA-256 (fletch-core's `codegraph::install`) and pushed
# it in, so nothing is fetched here. The disk is then attached read-only as
# /opt/fletch/tools, so the launcher ends up at
# /opt/fletch/tools/codegraph/bin/codegraph.
#
#   codegraph.sh <bundle.tar.gz>
set -eu
BUNDLE=${1:?usage: codegraph.sh <bundle.tar.gz>}
PREFIX=/data/tools/codegraph

rm -rf "$PREFIX"
mkdir -p "$PREFIX"
# The bundle's one top-level directory (codegraph-linux-<arch>/) is dropped.
tar -xzf "$BUNDLE" -C "$PREFIX" --strip-components=1
rm -f "$BUNDLE"
CODEGRAPH_TELEMETRY=0 "$PREFIX/bin/codegraph" --version
