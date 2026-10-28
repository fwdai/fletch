#!/bin/sh
# The box runtime's environment for the fletch-box CLI and the VM tests. Stages
# the runtime like the dev app does (packaging/box-bundle/ensure-dev.sh ->
# packaging/box-bundle/out), builds the CLI, and prints the exports the tests
# and `RuntimePaths::from_env` read. Use it as: eval "$(scripts/box-dev.sh)"
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/packaging/box-bundle/out"
TARGET="$ROOT/crates/fletch-box/target/release"

"$ROOT/packaging/box-bundle/ensure-dev.sh" >&2
[ -x "$OUT/fletch-vm" ] || { echo "box-dev.sh: no runtime in $OUT (see the warning above)" >&2; exit 1; }
cargo build --release --quiet --manifest-path "$ROOT/crates/fletch-box/Cargo.toml" --bin fletch-box >&2

echo "export FLETCH_BOX_HELPER=\"$OUT/fletch-vm\""
echo "export FLETCH_BOX_LIB_DIR=\"$OUT\""
echo "export PATH=\"$TARGET:\$PATH\""
