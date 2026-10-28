# Diagnostic service: reports vsock listen support, and keeps a guest-initiated exec channel open
# to host vsock 7002 (reverse shell for the spike only; the host side is a plain unix socket listener).
mkdir -p /data/workspace
echo "socat: $(socat -V 2>&1 | grep -i -E 'version|vsock' | tr '\n' ' ')"
echo "vsock modules: $(ls /sys/module 2>/dev/null | grep -i vsock | tr '\n' ' ')"
echo "proc vsock: $(ls /proc/net 2>/dev/null | grep -i vsock | tr '\n' ' ')"
# Listen-mode test target, with errors visible this time.
socat VSOCK-LISTEN:7001,reuseaddr,fork EXEC:"/bin/sh -s",stderr 2>&1 | sed 's/^/socat7001: /' &
socat VSOCK-LISTEN:9418,reuseaddr,fork EXEC:"git upload-pack --strict /data/workspace/repo" 2>&1 | sed 's/^/socat9418: /' &
sleep 0.3
echo "listeners: $(cat /proc/net/vsock 2>/dev/null | grep -c . || echo n/a)"
echo SERVICE_READY
# Reverse exec channel: reconnect for each host session.
while [ ! -f /tmp/stop ]; do
  socat VSOCK-CONNECT:2:7002 EXEC:"/bin/sh -s",stderr 2>/dev/null || sleep 0.3
done
echo SERVICE_STOP
