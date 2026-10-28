#!/bin/sh
# Checks a built app the way the engine will use it: only Helpers/fletch-vm
# carries the hypervisor entitlement, and that helper boots a guest with the
# dylibs from the bundle's own Frameworks folder (no lib dir given).
#
#   packaging/box-bundle/check.sh APP IMAGE [INIT]
#
# APP is a built Fletch.app; IMAGE is a base.erofs whose init prints the
# FLETCH_READY marker (the product image), or any image whose init runs a
# command when INIT is given (Calina's: /opt/calina/init, with a script disk
# that prints the markers). Port of Calina's sandbox/bundle/check.sh.
set -eu

APP=$(cd "${1:?usage: check.sh APP IMAGE [INIT]}" && pwd)
IMAGE=${2:?usage: check.sh APP IMAGE [INIT]}
INIT=${3:-/opt/fletch/init}
VM="$APP/Contents/Helpers/fletch-vm"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)

echo "== signatures"
codesign --verify --strict "$APP"
codesign -d --entitlements - --xml "$VM" 2>/dev/null | grep -q com.apple.security.hypervisor ||
  { echo "check: $VM lacks the hypervisor entitlement" >&2; exit 1; }
if codesign -d --entitlements - --xml "$APP/Contents/MacOS/fletch" 2>/dev/null | grep -q hypervisor; then
  echo "check: the app itself carries the hypervisor entitlement" >&2
  exit 1
fi
echo "ok   app valid; only Helpers/fletch-vm has com.apple.security.hypervisor"

# Under /tmp, not $TMPDIR: socket paths must stay under 104 bytes.
RUN=$(mktemp -d /tmp/fletch-box-check.XXXXXX)
trap 'rm -rf "$RUN"' EXIT
# An empty sparse data disk, as the engine creates it; the guest formats it on first boot.
dd if=/dev/zero of="$RUN/data.img" bs=1m count=0 seek=1024 2> /dev/null

set -- --id check --cpus 1 --mem 1024 \
  --disk-ro "base:$IMAGE" --disk "data:$RUN/data.img" --root-disk /dev/vda:erofs:ro \
  --vsock-listen "22:$RUN/ssh.sock" --vsock-listen "5000:$RUN/ctl.sock" \
  --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin --env HOME=/root --env TERM=xterm \
  --workdir /
if [ "$INIT" = /opt/fletch/init ]; then
  set -- "$@" -- "$INIT"
else
  # A test init needs a script that prints the markers and exits.
  printf 'echo FLETCH_HOSTKEY ssh-ed25519 AAAA\necho FLETCH_READY\nsync; exit 0\n' > "$RUN/script.sh"
  python3 -I -c 'import sys; d=open(sys.argv[1],"rb").read(); open(sys.argv[2],"wb").write(d+b"\n"*((-len(d))%512))' "$RUN/script.sh" "$RUN/script.img"
  set -- "$@" --disk-ro "script:$RUN/script.img" -- "$INIT" /bin/sh /dev/vdc
fi

echo "== booting $IMAGE with $VM (libkrun from the bundle)"
mkfifo "$RUN/stdin"
"$VM" "$@" < "$RUN/stdin" > "$RUN/console.log" 2>&1 &
pid=$!
# The helper runs until its stdin closes or the guest stops.
exec 3> "$RUN/stdin"
for _ in $(seq 1 50); do
  libs=$(lsof -p "$pid" 2>/dev/null | grep -o '/[^ ]*libkrun[^ ]*dylib' | sort -u) || true
  [ -z "$libs" ] || { echo "$libs" | sed 's/^/     loaded /'; break; }
  sleep 0.2
done
for _ in $(seq 1 100); do
  grep -q '^FLETCH_READY' "$RUN/console.log" 2>/dev/null && break
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.1
done
grep -E '^FLETCH_|fletch-vm:' "$RUN/console.log" | sed 's/^/   /'
if ! grep -q '^FLETCH_READY' "$RUN/console.log"; then
  exec 3>&-
  echo "check: FAIL (no FLETCH_READY)" >&2
  exit 1
fi
if [ "$INIT" = /opt/fletch/init ]; then
  # The product init stops on the control port.
  printf 'stop\n' | "$VM" pipe "$RUN/ctl.sock" || true
fi
wait "$pid" || true
exec 3>&-
case "$libs" in
  *"$APP/Contents/Frameworks/"*) echo "check: PASS" ;;
  *) echo "check: FAIL (libkrun not loaded from the bundle: $libs)" >&2; exit 1 ;;
esac
