# Box image

The root filesystem every box boots and the layers it overlays. The map of the
whole VM side, releases and workflows is [docs/box.md](../../docs/box.md); this
is the pipeline.

| File | What |
| --- | --- |
| `VERSION` | The image version; `bump.sh` moves it, the release is `box-image-v<VERSION>` |
| `packages/base.txt`, `devtools.txt`, `browser.txt` | The package split: the base and one file per layer |
| `dpkg-excludes.cfg` | dpkg path rules for every install: docs, man, locales, Mesa/LLVM |
| `rootfs/` | Files copied over the base: `opt/fletch/init`, `etc/ssh/sshd_config.fletch` |
| `build.sh` | Root, Debian arm64 or amd64 (the images are the build machine's architecture), privileged: mmdebstrap the base, apt the layers into copies of it, rsync the deltas, `mkfs.erofs` each, `manifest.json`, `sizes.md` |
| `test.sh` | Mounts the output as init would and runs the checks (chroot as `agent`); `report.md` |
| `build-docker.sh` | The one way to build, on a Mac (Docker Desktop) or in CI: fletch-guest, `build.sh` in a container, `test.sh` with `--test`, the delta manifests, an install with the app's installer |
| `boot-check.sh` | Boots the output on a Mac (or a Linux host with KVM) with its layers and checks the guest over ssh |
| `bump.sh` | Moves the pins in `build.sh` (Debian snapshot, Node 22 and its arm64 and x64 tarball hashes), VERSION and the app's pin |
| `publish.sh` | Creates the GitHub release from an output directory: `box-image-v<VERSION>` for an aarch64 build, `box-image-v<VERSION>-x86_64` for an x86_64 one (the manifest's `arch`); a draft, published once every file is there; refuses an existing version |

`build.sh`, `packages/`, `dpkg-excludes.cfg`, `rootfs/` and `crates/fletch-guest`
are the image's inputs: a change to them needs a VERSION bump (docs/box.md,
"Releases and updates"). The scripts around them do not.

## Outputs

`base.erofs`, `layer-<name>.erofs`, `manifest.json`, `sizes.md` (from
`build.sh`), and a `<file>.chunks.json` per image (`fletch-box image chunk`, run
by `build-docker.sh`; the app fetches only changed chunks on an update).

Inputs that pin the result: Debian packages from `snapshot.debian.org` at
`DEBIAN_SNAPSHOT`, Node from one nodejs.org tarball checked against its
`SHASUMS256` entry, every inode at `SOURCE_DATE_EPOCH`, a fixed UUID per image.
The same inputs and the same `fletch-guest` give byte-identical images.

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
| `/data` | the workspace disk (`/dev/vdb`, ext4), formatted on first boot, `e2fsck` when it does not mount |
| `/run/layers/<n>` | each layer image (an erofs after the data disk with `/.fletch-layer`), whose `usr`, `etc`, `var` are overlaid read-only on the root, later disks on top |
| `/opt/fletch/tools` | the `tools/` directory of the tools disk (the ext4 after the data disk), when one is attached |
| `/tmp`, `/run`, `/var/tmp` | tmpfs |
| vsock 22 | `fletch-guest vsock-listen 22 -- sshd -i`: one sshd per connection, no TCP listener |
| vsock 6000 | the host's egress proxy, exposed as `http://127.0.0.1:8899` |
| vsock 5000 | control: the host sends the line `stop` |

Console markers, in order: `FLETCH_HOSTKEY <type> <base64>`, `FLETCH_READY`,
and on any failure `FLETCH_INIT_ERROR <text>` then a non-zero exit. Users:
`root` and `agent` (uid 1000, home `/data/home`); sshd allows `agent` only,
public key only. Host paths are shadowed with a tmpfs at their first component
(`/Users` on a Mac host); paths under the guest's own directories are refused.

## Tools disks

`crates/fletch-box/tools/<provider>.sh <version>` installs one provider's CLI
into `/data/tools/<provider>` from inside a guest booted on the base alone with
a fresh disk as `/data` and the proxy variables set. After a clean stop the host
marks the disk read-only and attaches it; init exposes its `tools/` as
`/opt/fletch/tools`. The host side is `fletch_box::tools::build`.

## By hand

```sh
eval "$(scripts/box-dev.sh)"                 # stages the runtime, builds the CLI
fletch-box boot --id dev --image out/base.erofs --layer out/layer-devtools.erofs \
  --data /tmp/dev-data.img --env FLETCH_BOX_ID=dev --env FLETCH_BOX_HOME=$HOME \
  --env FLETCH_BOX_WRITABLE_ROOT=$HOME/.fletch/workspaces/dev --env FLETCH_BOX_RPC_DIR=$HOME/.fletch/rpc/dev
fletch-box ssh dev -- gcc --version
fletch-box stop dev
```
