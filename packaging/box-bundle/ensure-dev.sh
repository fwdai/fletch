#!/bin/sh
# Best-effort dev build of the box runtime: libkrun (packaging/libkrun/ensure.sh,
# skipped when dist/ is current) and an ad hoc signed fletch-vm, staged into
# packaging/box-bundle/out/ by prepare.sh, where a `tauri dev` binary looks
# (RuntimePaths::dev). `bun run tauri dev` runs this before Vite (scripts/tauri.sh).
#
# Never fails the dev build: on any problem it prints one warning naming the
# fix and exits 0; the Sandbox page's probe then says what is missing. The full
# output is in the log it names. libkrun needs Xcode (not only the Command Line
# Tools), so a contributor without it simply has no boxes.
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
TMP=${TMPDIR:-/tmp}
LOG="${TMP%/}/fletch-box-ensure-dev.log"

if [ "$(uname -sm)" != "Darwin arm64" ]; then
  echo "box runtime: skipped (boxes run on macOS on Apple silicon)"
  exit 0
fi

# Inside a Fletch sandbox ~/.cargo/registry is read-only; cargo then needs a
# CARGO_HOME elsewhere (packaging/libkrun/README.md). A fresh one re-downloads
# the registry index once.
if [ -z "${CARGO_HOME:-}" ] && [ -d "$HOME/.cargo/registry" ] && ! [ -w "$HOME/.cargo/registry" ]; then
  CARGO_HOME="${TMP%/}/fletch-cargo-home"
  export CARGO_HOME
fi

if "$ROOT/packaging/box-bundle/prepare.sh" > "$LOG" 2>&1; then
  echo "box runtime: ready (packaging/box-bundle/out; log: $LOG)"
  exit 0
fi

# The last line a script of ours wrote is the reason; ensure.sh and prepare.sh
# each end their failures with one that names the fix.
reason=$(grep -E '^(libkrun|prepare\.sh|build\.sh):' "$LOG" | tail -1 | sed -E 's/^[^:]+: //')
case "$reason" in
  "") reason="see $LOG" ;;
  *Xcode*) reason="needs Xcode (install it, then: sudo xcode-select -s /Applications/Xcode.app)" ;;
esac
echo "box runtime: not built; $reason (boxes stay unavailable; log: $LOG)" >&2
exit 0
