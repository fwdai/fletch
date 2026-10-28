# Box base image

The Debian root filesystem every box boots. `build.sh` turns `packages.txt`, `rootfs/` and the cross-built `fletch-guest` into:

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

With the runtime's CLI (`crates/fletch-box`; `scripts/box-dev.sh` stages the runtime and puts `fletch-box` on `PATH`):

```sh
eval "$(scripts/box-dev.sh)"
export FLETCH_BOX_RUN_ROOT=/tmp/fletch-box-dev                           # box dirs; default ~/.fletch/box-dev
fletch-box boot --id dev --image packaging/box-image/out/base.erofs --data $FLETCH_BOX_RUN_ROOT/dev/data.img \
  --env FLETCH_BOX_ID=dev --env FLETCH_BOX_HOME=$HOME \
  --env FLETCH_BOX_WRITABLE_ROOT=$HOME/.fletch/workspaces/dev --env FLETCH_BOX_RPC_DIR=$HOME/.fletch/rpc/dev \
  --allow deb.debian.org
```

`boot` creates the data disk sparse when it is missing (init formats it), generates the box's key pair and passes
`FLETCH_BOX_AUTHORIZED_KEY` itself, serves the egress proxy (the defaults plus `--allow`) and stays attached. The
console prints `FLETCH_HOSTKEY ...` and `FLETCH_READY`; the box dir (`$FLETCH_BOX_RUN_ROOT/dev/`) then holds the
key, `known_hosts`, the sockets and an `ssh_config` whose host alias is `box`. From another shell:

```sh
B=$FLETCH_BOX_RUN_ROOT/dev
fletch-box ssh dev -- uname -a
ssh -F $B/ssh_config box 'umask 077; cat > /data/run/t.env' <<EOF
HOME=$HOME
PATH=/usr/local/bin:/usr/bin:/bin
EOF
ssh -F $B/ssh_config box fletch-guest exec --env-file /data/run/t.env --cwd $HOME -- env
sftp -F $B/ssh_config box
fletch-box stop dev                                                      # clean stop, or Ctrl-C in the boot terminal
```

Never use macOS `nc -U` against the sockets: it returns nothing. Use ssh, or `fletch-vm pipe <socket>` (what the
`ssh_config`'s `ProxyCommand` runs).

## Tools disks

`crates/fletch-box/tools/<provider>.sh <version>` installs one provider's CLI into `/data/tools/<provider>` from inside a guest
booted on a fresh sparse disk with the proxy variables set (`fletch-guest exec --env-file`). After a clean stop
the host marks the disk read-only and attaches it as `/dev/vdc`; init exposes its `tools/` directory as
`/opt/fletch/tools`, so `PATH` gets `/opt/fletch/tools/<provider>/bin`. Each script ends with the CLI's
`--version`. The host side that boots the box and runs them is `fletch_box::tools::build`, which `fletch-box tools
build` and the engine's tools prebuild share.

## Publishing a release

Images are GitHub releases on this repository tagged `box-image-v<version>`, each holding `base.erofs` and
`manifest.json`; the app downloads `<base URL>/base.erofs`
(`https://fletch.sh/releases/download/box-image-v<version>/`, or the `box_image_url` setting)
and checks the size and sha256 its `ImageStore` pins from the manifest. The repository is private and the
app carries no GitHub credentials, so `fletch.sh` proxies `/releases/download/<tag>/<asset>` to the
release asset the same way it serves the updater's `latest.json` and artifacts; the website must route that
path before the first image release is usable. Bumping the image is three steps:

1. **Build and publish.** Actions → *Box image* → *Run workflow* with the version (`X.Y.Z`), or push a tag
   `box-image-v<version>`. The workflow (`.github/workflows/box-image.yml`) runs on GitHub's arm64 runner,
   builds `fletch-guest` and runs `build.sh` in a `debian:trixie` container (`build-mac.sh`, which is cargo
   plus docker and works there too), and creates the release with the pin values in its body. It refuses a
   version that already has a release: never publish two different `base.erofs` under one version, an app
   that has one treats it as installed.
2. **Pin.** `scripts/box-image-pin.sh box-image-v<version>` downloads the release's `manifest.json` and
   rewrites `PinnedImage::{VERSION, SIZE, SHA256}` in `crates/fletch-box/src/image_store/mod.rs`.
3. **Commit** that change; the next app release installs the new image on first use and prunes the old one.

For a local image (`build-mac.sh` → `out/`), point the `box_image_url` setting at `file://<path to out>` and
pin with `scripts/box-image-pin.sh packaging/box-image/out/manifest.json` (do not commit a dev pin).
