# Box base image

The Debian root filesystem every box boots (design: `docs/boxes.md`, spec: `docs/boxes-spec.md` §6).
`build.sh` turns `packages.txt`, `rootfs/` and the cross-built `fletch-guest` into:

| File            | What                                                                                   |
| --------------- | -------------------------------------------------------------------------------------- |
| `base.erofs`    | Read-only root filesystem, lz4hc erofs, attached read-only as `/dev/vda` to every box  |
| `manifest.json` | `version`, `arch`, `file`, `size`, `sha256` (what `fletch-box`'s `ImageStore` pins) and `packages` |

Each box's data disk is not part of the image: the host hands over an empty sparse file and init formats it.

## Building

`build.sh` runs as root on Debian arm64 (a container or CI). On a Mac:

```sh
packaging/box-image/build-mac.sh                      # -> packaging/box-image/out, version 0.0.0-dev
packaging/box-image/build-mac.sh out/0.1.0 0.1.0      # a release build
```

`build-mac.sh` cross-builds `crates/fletch-guest` for `aarch64-unknown-linux-musl` (rustup's self-contained
musl target, `rust-lld`, no C toolchain), then runs `build.sh` in `docker run --platform linux/arm64
debian:trixie` with this directory and the binary mounted. `out/` is gitignored.

Inputs that pin the result:

- **Debian packages** from `snapshot.debian.org` at `DEBIAN_SNAPSHOT` (default in `build.sh`), so a rebuild
  months later installs the same versions. Bump the timestamp deliberately to pick up security updates.
- **Node 22 LTS** from one nodejs.org tarball, checked against its `SHASUMS256` entry. A tarball rather than the
  NodeSource apt repository: one URL and one hash, no third-party key or repo metadata to keep in step.
- **Timestamps**: every inode gets `SOURCE_DATE_EPOCH` (default 2026-01-01); fontconfig caches are rebuilt
  after the mtimes are pinned so they stay valid. **Identity**: a fixed filesystem UUID. Postinst-generated
  ssh host keys, apt lists, logs and `machine-id` are removed.

## What the guest looks like

`/opt/fletch/init` (`rootfs/opt/fletch/init`) is the workload libkrun's init runs. It reads its inputs from the
kernel environment (`krun_set_env`), never from argv:

| Env                         | Meaning                                                                  |
| --------------------------- | ------------------------------------------------------------------------ |
| `FLETCH_BOX_WRITABLE_ROOT`  | host path of the agent's writable root, bind-mounted from `/data/ws`     |
| `FLETCH_BOX_RPC_DIR`        | host path of the RPC mailbox, from `/data/rpc`                           |
| `FLETCH_BOX_HOME`           | host path of `$HOME`, from `/data/home` (mounted first; the others may lie inside it) |
| `FLETCH_BOX_AUTHORIZED_KEY` | one OpenSSH public key line, written to `/data/ssh/authorized_keys`      |
| `FLETCH_BOX_ID`             | optional, shown in log lines                                             |

| Path / port             | What                                                                                      |
| ----------------------- | ----------------------------------------------------------------------------------------- |
| `/`                     | this image, erofs, read-only (`/dev/vda`)                                                 |
| `/data`                 | the workspace disk (`/dev/vdb`, ext4 `nosuid,nodev,discard,noinit_itable`); formatted on first boot 4 MiB short of the device, `e2fsck -y` when it does not mount, reformatted only if fsck cannot repair it |
| `/data/{ws,rpc,home,run}` | owned by `agent`; `/data/ssh` holds the host key and `authorized_keys` (root, StrictModes-clean) |
| `/data/run/<launch>.env` | env files the host writes for `fletch-guest exec --env-file`                              |
| `/opt/fletch/tools`     | the `tools/` directory of the tools disk (`/dev/vdc`, ext4, read-only), when one is attached |
| `/tmp`, `/run`, `/var/tmp` | tmpfs                                                                                  |
| vsock 22                | `fletch-guest vsock-listen 22 -- sshd -i -e -f /etc/ssh/sshd_config.fletch`: one sshd per connection, no TCP listener |
| vsock 6000              | the host's egress proxy; `fletch-guest egress-loopback 8899 6000` exposes it as `http://127.0.0.1:8899` |
| vsock 5000              | control: `fletch-guest wait-stop 5000`; the host sends the line `stop`                    |

Console markers, in order: `FLETCH_HOSTKEY <type> <base64>` (from `/data/ssh/host_key.pub`), `FLETCH_READY`
(all three vsock ports are bound: each service reports through a FIFO before init goes on), and on any failure
`FLETCH_INIT_ERROR <text>` followed by a non-zero exit. On `stop` init kills every process, `sync`s, unmounts
the binds and `/data` and exits; libkrun's init then powers the VM off.

Users: `root` and `agent` (uid/gid 1000, home `/data/home`, shell `/bin/bash`, no password, not locked).
sshd (`rootfs/etc/ssh/sshd_config.fletch`) allows `agent` only, public key only, sftp through
`/usr/lib/openssh/sftp-server`, TCP forwarding on, nothing accepted from the client environment.
`/usr/local/bin/fletch-guest` links to `/opt/fletch/bin/fletch-guest` so `ssh box fletch-guest exec ...` works
from the login shell's default PATH.

Identity paths: erofs is read-only, so init shadows the first component of each host path (`/Users` on a Mac
host, `/home` or `/root` on Linux) with a tmpfs and creates the tree inside it. The image ships an empty `/Users`
for that. Paths under the guest's own directories (`/usr`, `/var`, `/opt`, `/etc`, ...) are refused.

## Booting by hand

With the spike runner (`spike/boxes/README.md`; `/tmp/fletch-box-spike/bin/krun-run` and Calina's dylibs):

```sh
LIB=~/Code/calina/sandbox/libkrun/dist; W=/tmp/fletch-box-dev; S=spike/boxes; mkdir -p $W/ws
ssh-keygen -q -t ed25519 -N '' -f $W/key
truncate -s 8g $W/ws/data.img                                            # sparse; init formats it
python3 -I $S/host/egress_proxy.py $W/egress.sock $W/egress.log deb.debian.org registry.npmjs.org &
rm -f $W/ws/*.sock                                                       # libkrun binds listen sockets itself (EEXIST)
DYLD_LIBRARY_PATH=$LIB /tmp/fletch-box-spike/bin/krun-run --mem 2048 \
  --disk root:packaging/box-image/out/base.erofs:ro --disk data:$W/ws/data.img \
  --root-disk /dev/vda:erofs:ro \
  --vsock-listen 22:$W/ws/ssh.sock --vsock 6000:$W/egress.sock --vsock-listen 5000:$W/ws/ctl.sock \
  --env FLETCH_BOX_ID=dev --env FLETCH_BOX_HOME=$HOME \
  --env FLETCH_BOX_WRITABLE_ROOT=$HOME/.fletch/workspaces/dev --env FLETCH_BOX_RPC_DIR=$HOME/.fletch/rpc/dev \
  --env "FLETCH_BOX_AUTHORIZED_KEY=$(cat $W/key.pub)" \
  -- /opt/fletch/init
```

The console prints `FLETCH_HOSTKEY ...` and `FLETCH_READY`. Then, with an ssh config:

```
Host box
  User agent
  IdentityFile /tmp/fletch-box-dev/key
  IdentitiesOnly yes
  UserKnownHostsFile /tmp/fletch-box-dev/known_hosts     # "box <type> <base64>" from the FLETCH_HOSTKEY line
  ProxyCommand python3 -I spike/boxes/host/sockpipe.py /tmp/fletch-box-dev/ws/ssh.sock
```

```sh
ssh -F $W/cfg box uname -a
ssh -F $W/cfg box 'umask 077; cat > /data/run/t.env' <<EOF
HOME=$HOME
PATH=/usr/local/bin:/usr/bin:/bin
EOF
ssh -F $W/cfg box fletch-guest exec --env-file /data/run/t.env --cwd $HOME -- env
sftp -F $W/cfg box
printf 'stop\n' | python3 -I spike/boxes/host/sockpipe.py $W/ws/ctl.sock        # clean stop
```

Never use macOS `nc -U` against the sockets: it returns nothing. Use `sockpipe.py`, `execclient.py` or ssh.

## Tools disks

`tools/<provider>.sh <version>` installs one provider's CLI into `/data/tools/<provider>` from inside a guest
booted on a fresh sparse disk with the proxy variables set (`fletch-guest exec --env-file`). After a clean stop
the host marks the disk read-only and attaches it as `/dev/vdc`; init exposes its `tools/` directory as
`/opt/fletch/tools`, so `PATH` gets `/opt/fletch/tools/<provider>/bin`. Each script ends with the CLI's
`--version`. The host side that boots the box and runs them (`fletch-box tools build`) is phase 4.

## Publishing a release

Images are GitHub releases on this repository tagged `box-image-v<version>`, each holding `base.erofs` and
`manifest.json`; the app downloads `<base URL>/base.erofs` and checks the size and sha256 its `ImageStore`
pins from the manifest.

```sh
v=0.1.0
packaging/box-image/build-mac.sh packaging/box-image/out/$v $v
gh release create box-image-v$v --title "Box image $v" --notes "Box base image $v" \
  packaging/box-image/out/$v/base.erofs packaging/box-image/out/$v/manifest.json
```

Never publish two different `base.erofs` under one version: an app that has one treats it as installed.
