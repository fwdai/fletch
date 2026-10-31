#!/bin/sh
# Boot a built image with its layers on this Mac and check the guest over ssh:
# the layer overlays, the tools in them, a headless screenshot, fletch-guest.
# What test.sh cannot cover (init, libkrun, the real kernel). Stages the dev
# runtime and builds the CLI when `fletch-box` is not on PATH.
#
#   packaging/box-image/boot-check.sh [OUT_DIR]      # default packaging/box-image/out
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
OUT=${1:-$HERE/out}
OUT=$(cd "$OUT" && pwd)
for f in base.erofs layer-devtools.erofs layer-browser.erofs; do
  [ -f "$OUT/$f" ] || { echo "boot-check.sh: no $f in $OUT (build-mac.sh first)" >&2; exit 1; }
done
if ! command -v fletch-box >/dev/null 2>&1; then
  echo "== staging the box runtime and building fletch-box (scripts/box-dev.sh)"
  eval "$("$REPO/scripts/box-dev.sh")"
fi
ID=boot-check
export FLETCH_BOX_RUN_ROOT=${FLETCH_BOX_RUN_ROOT:-/tmp/fletch-box-check}
B="$FLETCH_BOX_RUN_ROOT/$ID"
rm -rf "$B"
mkdir -p "$B"

echo "== booting base + devtools + browser (console: $B/console.log)"
fletch-box boot --id "$ID" --image "$OUT/base.erofs" \
  --layer "$OUT/layer-devtools.erofs" --layer "$OUT/layer-browser.erofs" \
  --data "$B/data.img" \
  --env FLETCH_BOX_ID=$ID --env "FLETCH_BOX_HOME=$HOME" \
  --env "FLETCH_BOX_WRITABLE_ROOT=$HOME/.fletch/workspaces/$ID" \
  --env "FLETCH_BOX_RPC_DIR=$HOME/.fletch/rpc/$ID" > "$B/boot.log" 2>&1 &
BOOT=$!
for _ in $(seq 1 100); do
  grep -q "FLETCH_READY" "$B/console.log" 2>/dev/null && break
  grep -q "FLETCH_INIT_ERROR" "$B/console.log" 2>/dev/null && { cat "$B/console.log"; exit 1; }
  kill -0 $BOOT 2>/dev/null || { echo "boot exited:"; cat "$B/boot.log"; exit 1; }
  sleep 0.3
done
grep -E "ready in" "$B/boot.log" || true
grep -E "fletch-init:" "$B/console.log" || true

ssh_() { fletch-box ssh "$ID" -- "$@"; }
echo "== layer mounts"
ssh_ sh -c 'mount | grep -E " type (erofs|overlay) "'
echo "== devtools and browser through the overlay"
ssh_ sh -c 'gcc --version | head -1; python3 --version; chromium-headless-shell --version 2>/dev/null | head -1; node -v; ls /usr/local/include/node/node_api.h'
echo "== headless screenshot in the guest"
ssh_ sh -c 'cd /tmp && chromium-headless-shell --no-sandbox --disable-dev-shm-usage --screenshot=/tmp/s.png --window-size=400,200 about:blank 2>/dev/null; ls -la /tmp/s.png'
echo "== memory after boot"
ssh_ free -m
echo "== fletch-guest exec"
ssh_ sh -c 'umask 077; printf "HOME=/data/home\nPATH=/usr/local/bin:/usr/bin:/bin\n" > /data/run/t.env'
ssh_ fletch-guest exec --env-file /data/run/t.env --cwd /tmp -- sh -c 'echo exec ok: $HOME'

echo "== stopping"
fletch-box stop "$ID"
wait $BOOT || true
