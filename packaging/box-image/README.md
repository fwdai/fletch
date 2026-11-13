# Box image

The root filesystem every box boots and the layers it overlays. The map of the
whole VM side, releases and workflows is [docs/box.md](../../docs/box.md); this
is the pipeline.

| File | What |
| --- | --- |
| `VERSION` | The image version; `bump.sh` moves it, the release is `box-image-v<VERSION>` |
| `packages/base.txt`, `devtools.txt`, `browser.txt`, `containers.txt` | The package split: the base and one file per layer |
| `dpkg-excludes.cfg` | dpkg path rules for every install: docs, man, locales, Mesa/LLVM |
| `rootfs/` | Files copied over the base: `opt/fletch/init`, `etc/ssh/sshd_config.fletch`, `etc/sudoers.d/agent`, `etc/apt/apt.conf.d/90fletch-proxy`, `etc/docker/daemon.json`, `etc/skel/.docker/config.json` |
| `build.sh` | Root, Debian arm64 or amd64 (the images are the build machine's architecture), privileged: mmdebstrap the base, apt the layers into copies of it (the containers layer from Docker's apt repository, its key pinned), rsync the deltas, `mkfs.erofs` each, `manifest.json`, `sizes.md` |
| `test.sh` | Mounts the output as init would and runs the checks (chroot as `agent`); `report.md` |
| `build-docker.sh` | The one way to build, on a Mac (Docker Desktop) or in CI: fletch-guest, `build.sh` in a container, `test.sh` with `--test`, the delta manifests, an install with the app's installer |
| `boot-check.sh` | Boots the output on a Mac (or a Linux host with KVM) with its layers and checks the guest over ssh |
| `bump.sh` | Moves the pins in `build.sh` (Debian snapshot, Node 22 and its arm64 and x64 tarball hashes, Bun and its zip hashes), VERSION and the app's pin |
| `publish.sh` | Creates the GitHub release from an output directory: `box-image-v<VERSION>` for an aarch64 build, `box-image-v<VERSION>-x86_64` for an x86_64 one (the manifest's `arch`); a draft, published once every file is there; refuses an existing version |

`build.sh`, `packages/`, `dpkg-excludes.cfg`, `rootfs/` and `crates/fletch-guest`
are the image's inputs: a change to them needs a VERSION bump (docs/box.md,
"Releases and updates"). The scripts around them do not.

## Outputs

`base.erofs`, `layer-<name>.erofs`, `manifest.json`, `sizes.md` (from
`build.sh`), and a `<file>.chunks.json` per image (`fletch-box image chunk`, run
by `build-docker.sh`; the app fetches only changed chunks on an update).

Inputs that pin the result: Debian packages from `snapshot.debian.org` at
`DEBIAN_SNAPSHOT`, Docker's packages at the versions `packages/containers.txt`
names from `download.docker.com`, whose signing key is checked against
`DOCKER_KEY_SHA256`, Node from one nodejs.org tarball checked against its
`SHASUMS256` entry, Bun from one GitHub release zip checked against its
`SHASUMS256` entry, every inode at `SOURCE_DATE_EPOCH`, a fixed UUID per image.
The same inputs and the same `fletch-guest` give byte-identical images.
`bump.sh` does not move the Docker pins: a new Docker release is a hand edit
of `packages/containers.txt`.

## What the guest looks like

`/opt/fletch/init` is the workload libkrun's init runs. It reads its inputs
from the kernel environment (`krun_set_env`), never from argv:

| Env | Meaning |
| --- | --- |
| `FLETCH_BOX_WRITABLE_ROOT` | host path of the agent's writable root, bind-mounted from `/data/ws` |
| `FLETCH_BOX_RPC_DIR` | host path of the RPC mailbox, from `/data/rpc` |
| `FLETCH_BOX_HOME` | host path of `$HOME`, from `/data/home` (mounted first; the others may lie inside it) |
| `FLETCH_BOX_AUTHORIZED_KEY` | one OpenSSH public key line, written to `/data/ssh/authorized_keys` |
| `FLETCH_BOX_ID` | optional, shown in log lines |
| `FLETCH_BOX_SHARES`, `FLETCH_BOX_SHARE_<i>` | folders the user shared: `ro:<path>` or `rw:<path>`, virtio-fs device `fletch-share-<i>` mounted at `<path>` |

| Path / port | What |
| --- | --- |
| `/` | `base.erofs`, read-only (`/dev/vda`) |
| `/data` | the workspace disk (`/dev/vdb`, ext4), formatted on first boot, `e2fsck` when it does not mount; made read-only at stop, which leaves it clean |
| `/run/layers/<n>` | each layer image (an erofs after the data disk with `/.fletch-layer`) |
| `/usr`, `/etc`, `/var` | writable: each one overlay of the base's directory, the layers' on it (later disks on top), and the box's own changes in `/data/sys/<dir>/upper` |
| `/data/sys` | the box's changes to the system (`sudo apt-get install`), kept across boots; `/data/sys/image` names the image they were made on (`/etc/fletch-image`), and a boot of another image deletes them |
| `/opt/fletch/tools` | the `tools/` directories of the tools disks (the ext4 disks after the data disk), when any are attached: one bound, several overlaid with the first on top |
| `/tmp`, `/run`, `/var/tmp` | tmpfs |
| `/run/docker.sock` | with the containers layer: `root:docker` 0660, held by `fletch-guest socket-activate` until the first connection starts `dockerd` on it |
| `/data/docker` | Docker's data root: images, containers, volumes |
| vsock 22 | `fletch-guest vsock-listen 22 -- sshd -i`: one sshd per connection, no TCP listener |
| vsock 6000 | the host's egress proxy, exposed on TCP 8899 on every address: `http://127.0.0.1:8899` in the guest, `http://172.17.0.1:8899` from a container |
| vsock 5000 | control: the host sends the line `stop` |

Console markers, in order: `FLETCH_HOSTKEY <type> <base64>`, `FLETCH_READY`,
and on any failure `FLETCH_INIT_ERROR <text>` then a non-zero exit. Users:
`root` and `agent` (uid 1000, home `/data/home`, group `docker`, passwordless
`sudo`); sshd allows `agent` only, public key only. A confined `exec` runs with
`no_new_privs`, so `sudo` does not work there. Host paths are shadowed with a
tmpfs at their first component (`/Users` on a Mac host); paths under the
guest's own directories are refused. apt uses the image's `https` sources
(`snapshot.debian.org` at the image's snapshot) through the egress proxy
(`/etc/apt/apt.conf.d/90fletch-proxy`).

### Containers

The `containers` layer carries Docker Engine, containerd, and the compose and
buildx plugins. No Docker process runs after boot: init has `fletch-guest
socket-activate` listen on `/run/docker.sock`, and the first connection
execs `dockerd -H fd://` with the socket as fd 3 (`LISTEN_FDS`/`LISTEN_PID`,
systemd's protocol), so that connection is the daemon's first. Its log is
`/var/log/docker.log`. A daemon that exits is not started again until the
next boot. `/etc/docker/daemon.json`: data root `/data/docker`, the nftables
firewall backend (the guest kernel has no xtables; init sets
`net.ipv4.ip_forward`, which dockerd leaves alone with it), the default
bridge at `172.17.0.1/16`, and the egress proxy for pulls. Containers reach the
proxy at the bridge's gateway: init copies `/etc/skel/.docker/config.json`
(`proxies.default`: `http://172.17.0.1:8899`, `noProxy` the loopback and
Docker's private ranges) into the agent's home when it has no
`.docker/config.json`, and the docker CLI hands those settings to every
container it creates and every image build. A service reached by its compose
name is not in `noProxy`. The CLI's own requests take the proxy from its
environment, which every launch's env file sets (`HTTPS_PROXY`): a build
fetches its registry token in the client, so `docker build` in a session
without it cannot resolve the base image.

At stop every process is killed after 0.2 s, containers included; a
database in a container recovers as after a crash.

### `fletch-guest`, as the host uses it

| Command | What |
| --- | --- |
| `exec --env-file … [--read-only …] [--hide <path>]… -- <cmd>` | the launch path; each `--hide` file or socket that exists (never a directory) is covered by a bind of `/dev/null` in the confined view, so `--hide /run/docker.sock` leaves the command no route to the daemon. A socket recreated at the path after the launch is not covered |
| `listeners` | prints the sorted TCP ports listened on at `127.0.0.1`, `::1`, `0.0.0.0` or `::` in the guest's network namespace as one JSON array line (`[3000,5432]`), at once and on every change (polled each second), until killed; never 8899. A published container port shows up through Docker's userland proxy. Needs no root |
| `sudo -n /opt/fletch/bin/fletch-guest reclaim` | syncs, drops the page cache, compacts memory, so the guest's file cache goes back to the host; exits 0 |

## Tools disks

`crates/fletch-box/tools/<provider>.sh <version>` installs one provider's CLI
into `/data/tools/<provider>` from inside a guest booted on the base alone with
a fresh disk as `/data` and the proxy variables set. After a clean stop the host
marks the disk read-only and attaches it; init exposes its `tools/` as
`/opt/fletch/tools`, overlaying several disks when a box carries more than one
provider's CLI (the first attached on top). The host side is
`fletch_box::tools::build`.

## By hand

```sh
eval "$(scripts/box-dev.sh)"                 # stages the runtime, builds the CLI
fletch-box boot --id dev --image out/base.erofs --layer out/layer-devtools.erofs \
  --layer out/layer-containers.erofs \
  --data /tmp/dev-data.img --env FLETCH_BOX_ID=dev --env FLETCH_BOX_HOME=$HOME \
  --env FLETCH_BOX_WRITABLE_ROOT=$HOME/.fletch/workspaces/dev --env FLETCH_BOX_RPC_DIR=$HOME/.fletch/rpc/dev
fletch-box ssh dev -- gcc --version
fletch-box stop dev
```
