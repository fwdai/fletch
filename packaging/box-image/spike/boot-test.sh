#!/bin/sh
# Spike, Mac side: boot the layered image with two layer disks and check the
# guest over ssh (assumptions A14 to A16 in README.md). Needs the runtime on
# PATH: `eval "$(scripts/box-dev.sh)"`, and the workflow's artifact unpacked.
#
#   packaging/box-image/spike/boot-test.sh <artifact dir>
set -eu
OUT=${1:?usage: boot-test.sh <dir with base.erofs and layer-*.erofs>}
OUT=$(cd "$OUT" && pwd)
ID=spike
export FLETCH_BOX_RUN_ROOT=${FLETCH_BOX_RUN_ROOT:-/tmp/fletch-box-spike}
B="$FLETCH_BOX_RUN_ROOT/$ID"
rm -rf "$B"
mkdir -p "$B"

echo "== booting base + devtools + browser (console: $B/console.log)"
fletch-box boot --id "$ID" --image "$OUT/base.erofs" \
  --layer "$OUT/layer-devtools.erofs" --layer "$OUT/layer-browser.erofs" \
  --data "$B/data.img" \
  --env FLETCH_BOX_ID=$ID --env "FLETCH_BOX_HOME=$HOME" \
  --env "FLETCH_BOX_WRITABLE_ROOT=$HOME/.fletch/workspaces/$ID" \
  --env "FLETCH_BOX_RPC_DIR=$HOME/.fletch/rpc/$ID" \
  --allow cdn.playwright.dev > "$B/boot.log" 2>&1 &
BOOT=$!
for _ in $(seq 1 100); do
  grep -q "FLETCH_READY" "$B/console.log" 2>/dev/null && break
  grep -q "FLETCH_INIT_ERROR" "$B/console.log" 2>/dev/null && { cat "$B/console.log"; exit 1; }
  kill -0 $BOOT 2>/dev/null || { echo "boot exited:"; cat "$B/boot.log"; exit 1; }
  sleep 0.3
done
grep -E "ready in|spawned" "$B/boot.log" || true
grep -E "fletch-init:" "$B/console.log" || true

ssh_() { fletch-box ssh "$ID" -- "$@"; }
echo "== A14: layer mounts"
ssh_ sh -c 'mount | grep -E " type (erofs|overlay) " ; cat /run/layers/*/.fletch-layer'
echo "== A14: devtools and browser through the overlay"
ssh_ sh -c 'gcc --version | head -1; python3 --version; chromium --version; node -v; ls /usr/local/include/node/node_api.h'
echo "== A14: headless screenshot in the guest"
ssh_ sh -c 'cd /tmp && chromium --headless=new --no-sandbox --disable-dev-shm-usage --screenshot=/tmp/s.png --window-size=400,200 about:blank 2>/dev/null; ls -la /tmp/s.png'
echo "== A1: no Mesa/LLVM files"
ssh_ sh -c 'ls /usr/lib/aarch64-linux-gnu/libLLVM* 2>&1 | head -1'
echo "== A16: memory after boot"
ssh_ free -m
echo "== fletch-guest exec still works"
ssh_ sh -c 'umask 077; printf "HOME=/data/home\nPATH=/usr/local/bin:/usr/bin:/bin\n" > /data/run/t.env'
ssh_ fletch-guest exec --env-file /data/run/t.env --cwd /tmp -- sh -c 'echo exec ok: $HOME'

echo "== stopping"
fletch-box stop "$ID"
wait $BOOT || true
grep -E "stop|clean" "$B/boot.log" | tail -3 || true
echo "== done; compare 'ready in' above with image 0.2.0 (README: 0.16-1.0 s)"
