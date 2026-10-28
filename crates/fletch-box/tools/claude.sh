#!/bin/sh
# Installs Claude Code onto a tools disk. Runs inside a guest booted on a fresh
# data disk (`fletch-box tools build claude <version>`) with the proxy
# variables in the environment; the disk is then attached read-only as
# /opt/fletch/tools, so the CLI ends up at /opt/fletch/tools/claude/bin/claude.
#
#   claude.sh <version>
set -eu
VERSION=${1:?usage: claude.sh <version>}
PREFIX=/data/tools/claude

mkdir -p "$PREFIX"
npm install -g --prefix "$PREFIX" --cache /tmp/npm-cache --no-fund --no-audit \
  "@anthropic-ai/claude-code@$VERSION"
"$PREFIX/bin/claude" --version
