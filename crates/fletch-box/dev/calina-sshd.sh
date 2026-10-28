#!/bin/sh
# Dev stand-in for the product init, run on Calina's Alpine image as
# `fletch-box boot --init /opt/calina/init --script-disk dev/calina-sshd.sh`
# against a data disk that has the spike's dropbear under /data/opt/ssh
# (docs/boxes.md, phase 2a). It follows the guest contract in src/guest.rs:
# host key marker, authorized key from the env, sshd on vsock 22, FLETCH_READY,
# then wait for `stop` on vsock 5000. The product image replaces all of this
# with /opt/fletch/init and fletch-guest.
set -u
export LD_LIBRARY_PATH=/data/opt/ssh/usr/lib
SSH=/data/opt/ssh
KEY=$SSH/host_key

fail() { echo "FLETCH_INIT_ERROR $*"; exit 1; }
[ -x "$SSH/usr/sbin/dropbear" ] || fail "no dropbear on the data disk"
[ -f "$KEY" ] || "$SSH/usr/bin/dropbearkey" -t ed25519 -f "$KEY" >/dev/null 2>&1 || fail "no host key"

# The root fs is read-only; root's home gets a tmpfs for authorized_keys
# (mode 700: dropbear refuses a world-writable home).
mount -t tmpfs -o mode=700 tmpfs /root || fail "tmpfs on /root"
mkdir -p /root/.ssh && chmod 700 /root/.ssh
[ -n "${FLETCH_BOX_AUTHORIZED_KEY:-}" ] || fail "FLETCH_BOX_AUTHORIZED_KEY is empty"
printf '%s\n' "$FLETCH_BOX_AUTHORIZED_KEY" > /root/.ssh/authorized_keys
chmod 600 /root/.ssh/authorized_keys

pub=$("$SSH/usr/bin/dropbearkey" -y -f "$KEY" 2>/dev/null | grep '^ssh-') || fail "cannot read the host key"
echo "FLETCH_HOSTKEY $(echo "$pub" | cut -d' ' -f1,2)"

"$SSH/usr/sbin/dropbear" -F -E -R -r "$KEY" -p 127.0.0.1:2223 >/tmp/dropbear.log 2>&1 &
socat VSOCK-LISTEN:22,reuseaddr,fork TCP:127.0.0.1:2223 >/tmp/relay22.log 2>&1 &
sleep 0.2
echo FLETCH_READY

socat -u VSOCK-LISTEN:5000,reuseaddr - | while read -r line; do [ "$line" = stop ] && break; done
sync
exit 0
