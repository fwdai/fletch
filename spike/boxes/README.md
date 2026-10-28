# Boxes spike, phase 1

Throwaway code behind the findings recorded in `docs/boxes.md` ("Spike findings"). It boots
Calina's agent image (`~/Code/calina`, its built `sandbox/libkrun/dist` and
`sandbox/image/out/dev/base.erofs`) under libkrun from inside a Fletch sandbox and proves the
host contract: no shared folder, SSH-shaped channels over vsock, git both ways, egress through a
host proxy, a dev-server port forwarded out, and a workspace cloned from a template disk.

Not product code. Nothing here is built by cargo or shipped.

| File | What |
| --- | --- |
| `krun-run.c` | The VM runner: disks, root remount, vsock ports in both directions, env, console. Linked against Calina's `libkrun.1.dylib`, ad-hoc signed with `hypervisor.entitlements`. |
| `mkdisk.py` | Pads a guest script to a 512-byte multiple so it can be attached as a raw disk and run as `/bin/sh /dev/vdc`. libkrun's init re-splits a quoted `sh -c` line, so scripts never travel in argv. |
| `guest/probe1.sh` | First boot: kernel, mounts, toolchain versions. |
| `guest/service.sh`, `guest/service2.sh` | Long-running guest: exec channel on vsock 7001 (`sh -s` per connection), `git upload-pack` on 9418, diagnostics. |
| `guest/probe2.sh` | Boot of a cloned template disk: repo and CLIs present. |
| `host/gitserve.py` | Serves one repository's `upload-pack` on a unix socket: what the guest clones from. |
| `host/egress_proxy.py` | HTTP CONNECT proxy on a unix socket with a host allow-list and a log. |
| `host/bridge.py` | TCP `127.0.0.1:PORT` to a unix socket, so a browser reaches a forwarded guest port. |
| `host/execclient.py` | Sends a script to the guest's `sh -s` over a listen-mode vsock socket, prints the output. |
| `host/sockpipe.py` | stdio to unix socket, for `git fetch "ext::python3 -I sockpipe.py SOCK"` on the host. |
| `host/ptyclient.py` | Attaches to a guest PTY over a listen-mode socket and reports what it draws. |

## Reproducing

```sh
LIB=~/Code/calina/sandbox/libkrun/dist; W=/tmp/fletch-box-spike; mkdir -p $W/bin $W/ws1
clang -O2 -Wall -o $W/bin/krun-run krun-run.c $LIB/libkrun.1.dylib -Wl,-rpath,$LIB
codesign -s - --entitlements hypervisor.entitlements $W/bin/krun-run
cp -c ~/Code/calina/sandbox/image/out/dev/base.erofs $W/base.erofs     # APFS clone, instant
truncate -s 8g $W/ws1/data.img                                         # sparse; the guest formats it
python3 -I mkdisk.py guest/service2.sh $W/service2.img

python3 -I host/gitserve.py $W/git.sock ~/Code/fletch/.git &           # --strict needs the .git dir
python3 -I host/egress_proxy.py $W/egress.sock $W/egress.log registry.npmjs.org platform.claude.com api.anthropic.com &
rm -f $W/ws1/*.sock                                                    # libkrun binds listen sockets itself; stale ones are EEXIST
DYLD_LIBRARY_PATH=$LIB $W/bin/krun-run --mem 2048 \
  --disk root:$W/base.erofs:ro --disk data:$W/ws1/data.img --disk script:$W/service2.img:ro \
  --root-disk /dev/vda:erofs:ro \
  --vsock 6000:$W/egress.sock --vsock 9419:$W/git.sock \
  --vsock-listen 7001:$W/ws1/sh.sock --vsock-listen 9418:$W/ws1/gitout.sock --vsock-listen 5173:$W/ws1/web.sock \
  -- /opt/calina/init /bin/sh /dev/vdc &

printf 'cd /data/workspace && git -c protocol.ext.allow=always clone "ext::socat - VSOCK-CONNECT:2:9419" repo\n' \
  | python3 -I host/execclient.py connect $W/ws1/sh.sock 300
```

Do not use macOS `nc -U` against the listen sockets: it returns nothing after five seconds. Use
`execclient.py` or `sockpipe.py`.
