# Cloud image

The box image as one OCI image, for a cloud box: a machine at a cloud
provider (Fly.io first) that the engine reaches over ssh across the network,
with the guest contract of a box on this machine. The map of the VM side is
[docs/box.md](../../docs/box.md); the box image itself is
[packaging/box-image](../box-image/README.md).

| File | What |
| --- | --- |
| `inputs.sh` | The fingerprint of the files below that decide what the image holds; the tag is `<box image VERSION>-<fingerprint>` |
| `entrypoint` | `/opt/fletch/cloud-entrypoint`, the image's `ENTRYPOINT`: the guest contract over TCP |
| `build.sh` | Root, Debian trixie: extracts a built box image's base and layers, flattens them, applies the cloud changes, writes `rootfs.tar` |
| `build-docker.sh` | The one way to build, on a Mac (Docker Desktop) or in CI: `build.sh` in a container, `docker import`, `test.sh` with `--test` |
| `test.sh` | Runs the image as the engine reaches one and checks it over ssh; `report.md` |
| `publish.sh` | Pushes the image unless the registry has its tag |

## How it is built

From a box image's built files, never by building one: `manifest.json`,
`base.erofs` and `layer-<name>.erofs`, as `packaging/box-image/build-docker.sh`
writes them to `packaging/box-image/out` or a release carries them. What a box
contains is decided there alone.

`build.sh` checks every file against the manifest's sha256, extracts each
image with `fsck.erofs --extract` (erofs-utils 1.8.6 in trixie: it decodes the
images' lz4hc clusters, dedupe and fragments, and as root keeps owners, modes
with their setuid bits, hard links and times), and copies each layer's
directories (`usr`, `etc`, `var`, as its `/.fletch-layer` marker names them)
over the base in the manifest's order, later on top: what init's overlays
show a box. A layer only adds or replaces files (`rsync --compare-dest`, then
`mkfs.erofs` without xattrs), so it holds neither a whiteout nor an opaque
directory and the copy is the overlay; `build.sh` refuses a layer with a
device node. The copy is `tar`, which unlinks a file before replacing it, so a
file hard-linked in the base keeps its other names. The box images carry no
extended attributes (`mkfs.erofs -x -1`), so the cloud image has none either.

The cloud changes, and nothing else:

- `/opt/fletch/cloud-entrypoint`, from `entrypoint`.
- `/etc/apt/apt.conf.d/90fletch-proxy` is removed: apt goes out directly, as
  everything on the machine does.
- `/etc/docker/daemon.json` loses `dns` (the containers' sentinel DNS) and
  `proxies` (the egress loopback); the data root (`/data/docker`), the
  nftables backend and the bridge stay. Nothing starts dockerd in a cloud box;
  one started by hand (`sudo dockerd &`) finds a config that works.
- `/etc/fletch-cloud-image`: `fletch-cloud-image <tag> (<arch>, box image
  <version>, cloud inputs <fingerprint>)`, beside the box image's `/etc/fletch-image`.
- The image config: the entrypoint, `PATH`, and
  `org.opencontainers.image.source` (the repository).

`/etc/resolv.conf` (empty) and `/etc/hosts` stay the box image's: the runtime
replaces both, Docker by binds of its own and the provider's init by writing
them.

## The entrypoint

It runs as root at every start of the machine: under the provider's init, or
as PID 1 under `docker run`. It reads the environment init reads:

| Env | Meaning |
| --- | --- |
| `FLETCH_BOX_WRITABLE_ROOT` | host path of the agent's writable root, bound from `/data/ws` |
| `FLETCH_BOX_RPC_DIR` | host path of the RPC mailbox, from `/data/rpc` |
| `FLETCH_BOX_HOME` | host path of `$HOME`, from `/data/home` (bound first; the others may lie inside it) |
| `FLETCH_BOX_AUTHORIZED_KEY` | one OpenSSH public key line, written to `/data/ssh/authorized_keys` |
| `FLETCH_BOX_ID` | optional, shown in log lines |
| `FLETCH_BOX_SSH_PORT` | optional, sshd's port (2222: a Fly Machine's own sshd holds 22; the engine's `guest::CLOUD_SSH_PORT`, which it also passes, and which `scripts/check-cloud-image.sh` holds to this default) |

| Path / port | What |
| --- | --- |
| `/data` | the machine's volume: `ws`, `rpc`, `home`, `run` (0700) and `tools` owned by `agent`, `ssh` root's; on a runner's machine also `host` (0700, `agent`'s), which the engine makes |
| the three host paths | bind mounts of `/data/ws`, `/data/rpc`, `/data/home`, each resolving to itself; the directories between a path and its first component belong to `agent`. The root is writable, so they are made in it, with no tmpfs over the first component; init's refusals hold (a path under `/bin`, `/etc`, `/usr`, … or with one component) |
| `/opt/fletch/tools` | `/data/tools` bound read-only, as a tools disk is mounted: the host installs each provider's CLI into `/data/tools/<provider>` over ssh (the layout a tools disk has, `crates/fletch-box/tools/<provider>.sh`, run with no proxy by `fletch_box::tools::install_cli`, which records the version in `/data/tools/<provider>/.fletch-version`), and a launch finds it at `/opt/fletch/tools/<provider>/bin`. A bind, not a link, since a confined launch has `/data` covered |
| `/data/ssh/host_key` | the ssh host key, made once; the host reads `host_key.pub` (one OpenSSH public key line) through the provider's exec and pins it (`HostKey::from_public_key`) |
| `/data/host/start` | a host service, when the disk holds one (a runner's: [docs/cloud-runner-setup.md](../../docs/cloud-runner-setup.md)): run at every start, before sshd, as `agent`, in the background, by `/bin/sh` with an empty environment, its output in the machine's log. A box has none. The engine names the path `guest::CLOUD_HOST_SERVICE`, which `scripts/check-cloud-image.sh` holds to the entrypoint's |
| port 2222 | `sshd -D -e -f /etc/ssh/sshd_config.fletch`, on every address, IPv4 and IPv6; key only, `agent` only |

On a failure it prints `FLETCH_INIT_ERROR <text>` and exits non-zero before
sshd starts. Its mounts are made again at every start; `/data`, and the
directories made on the root, stay. Where it reproduces a step of init's, its
comment names the step: a change to one is carried to the other.

The host service hook is the one step that is not init's. A runner (a
`fletch-host` on a provider machine, made by the desktop) is a machine of
this image whose disk holds `/data/host/start`, written by the engine that
set it up; the entrypoint starts it and leaves it to keep its service up.
What the service runs and with which flags is the script's, not the image's:
a machine keeps the image it was made with, and the engine writes the script.

Not in a cloud box, since the machine has a network of its own and its
provider stops it: vsock, the egress proxy (and with it the allow-list: the
machine's traffic is its own), the control port, the console markers, the
system overlay (`/usr`, `/etc` and `/var` are the root's, so `sudo apt-get
install` lasts as long as the provider keeps the root filesystem), Docker's
socket activation and the containers' egress, and the `fletch-box` hostname.

The host reaches the guest with the channel's remote routes
(`crates/fletch-box/src/channel`): `Route::tcp(host, port)` for sshd on a TCP
endpoint, `Route::tls(host, port)` behind a proxy that terminates TLS and
forwards by server name (Fly.io's `tls` handler), with `openssl s_client`
as the `ProxyCommand`.

## Building and testing locally

```sh
packaging/box-image/build-docker.sh              # the box image, -> packaging/box-image/out
packaging/cloud-image/build-docker.sh --test     # -> ghcr.io/fwdai/fletch-cloud:<VERSION>-<fingerprint>, local
```

Instead of building the box image, `packaging/box-image/out` may hold a
release's `manifest.json` and `.erofs` files
(`https://github.com/fwdai/fletch/releases/download/box-image-v<VERSION>/<file>`,
`box-image-v<VERSION>-x86_64` for x86_64). The cloud image is the box image's
architecture, and `test.sh` runs it, so it is tested on a machine of that
architecture: a Mac builds and tests the aarch64 one.

`test.sh` runs the image privileged with the four variables, a volume at
`/data` and port 2222 published on loopback, reads the host key with `docker
exec` and connects with this machine's ssh in a config of the channel's shape
for `Route::tcp`, the key pinned under `box`. It checks: the refusals; the
agent's sessions; sshd on the IPv4 and IPv6 wildcards; the host paths (owner,
`realpath`, the same trees as `/data`); `/opt/fletch/tools` showing
`/data/tools` read-only, to a confined launch too; `fletch-guest exec` with an
env file under `/data/run`; git, node and npm; https and `sudo apt-get update`
with no proxy; `sudo`; dockerd's config and no dockerd; the image's names; no host
service on a box's disk; then a stop and a start, after which the host key
and `/data` are the same, the binds are back, and a start script put in
`/data/host` before the stop ran as `agent` with none of the machine's
environment.

The confined launch makes a user namespace as the guest user, on the kernel of
the machine the test runs on. Where that kernel gives an unprivileged user
namespace no capabilities (Ubuntu's AppArmor policy, the default from 24.04),
the check fails and the test says so: `sudo sysctl -w
kernel.apparmor_restrict_unprivileged_userns=0`, as the workflow does on its
runner, then test again.

## Tags and publishing

`ghcr.io/fwdai/fletch-cloud:<box image VERSION>-<fingerprint>`, x86_64 only (the
machines a cloud box runs on; the box image's x86_64 release is
`box-image-v<VERSION>-x86_64`). The Box image workflow's x86-64 job builds and
tests it from the images it built on every run, and pushes it where it
publishes the release, unless the registry has the tag (`publish.sh`); a pull
request pushes nothing.

A tag names one image, with nothing to bump. Its second half is the
fingerprint `inputs.sh` prints: twelve hex digits over `build.sh`,
`build-docker.sh` (the image's config) and `entrypoint`, the files here that
decide what the image holds. A change to any of them is another tag, which
the registry does not have and so gets; `test.sh`, `publish.sh` and this
README leave it alone, and a new box image VERSION is a new tag by itself.
`scripts/check-cloud-image.sh`, run by CI, holds the fingerprint to exactly
that.

The app runs the image of its own pins: `PinnedImage::cloud_image()`
(`crates/fletch-box/src/image_store`), `<CLOUD_REPOSITORY>:<VERSION>-<CLOUD_FINGERPRINT>`.
A change to the inputs moves `PinnedImage::CLOUD_FINGERPRINT` in the same
pull request, as a box image change moves `PinnedImage::VERSION`;
`scripts/check-cloud-image.sh` fails CI when the pin is not the fingerprint
`inputs.sh` prints, and when the entrypoint's default sshd port is not the
engine's `guest::CLOUD_SSH_PORT`. Until main's Box image workflow has pushed
the tag, a new cloud box of a build with that pin cannot pull its image.

To run cloud boxes on an image of one's own (a private registry, a tag not
pushed yet), set `FLETCH_CLOUD_IMAGE` to the whole image reference in the
app's environment (`FLETCH_CLOUD_IMAGE=registry.example/me/fletch-cloud:dev`):
new machines, and machines replaced on their disk, run it instead of the pin.
A machine that exists keeps the image it was made with.

ghcr makes a new package private. A provider pulls the image without
credentials only once the package is public (its settings on GitHub, once).
