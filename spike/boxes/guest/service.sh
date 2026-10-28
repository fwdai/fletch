# Long-running guest for the spike. Runs as: /opt/calina/init /bin/sh /dev/vdc
# Host -> guest (libkrun listen-mode vsock ports): 7001 shell-exec, 9418 git upload-pack, 5173 dev server.
# Guest -> host (init already forwards 7000->5000 control, 8899->6000 egress): 9419 git clone source.
mkdir -p /data/workspace
# Exec channel: each host connection gets a shell reading commands from the stream.
socat -lf /dev/null VSOCK-LISTEN:7001,reuseaddr,fork EXEC:"/bin/sh -s",stderr &
# Git out: the host fetches this repo's objects.
socat -lf /dev/null VSOCK-LISTEN:9418,reuseaddr,fork EXEC:"git upload-pack --strict /data/workspace/repo" &
echo SERVICE_READY
# Stay up until the host says stop (a file the exec channel creates).
while [ ! -f /tmp/stop ]; do sleep 0.2; done
echo SERVICE_STOP
