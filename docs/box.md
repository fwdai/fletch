# Boxes: the VM sandbox

A box is a libkrun micro-VM per agent on Apple silicon (Hypervisor.framework)
or on an x86-64 or arm64 Linux host (KVM, for `fletch-host`): no network device, no
shared folder unless the user grants one, reached from the host only over SSH
on vsock and an HTTP CONNECT egress proxy with an allow-list. The guest holds
the agent's tree at the same absolute paths the host uses. This page is the map;
each part's README has the detail.

| Part | Where | What |
| --- | --- | --- |
| Runtime | `crates/fletch-box` | libkrun helper (`fletch-vm`), VM spec, SSH channel, egress proxy, image store, tools disks; the `fletch-box` CLI for manual use |
| Guest agent | `crates/fletch-guest` | Static musl binary in the image: vsock listeners, `exec`, `tail`, the stop hook |
| Image | `packaging/box-image` | The Debian root every box boots and its layers; `build-docker.sh` (builds, tests, checks), `bump.sh`, `publish.sh` |
| libkrun | `packaging/libkrun` | libkrun 1.19 + patches and the libkrunfw 6.12 kernel the app ships (dylibs) and Linux `fletch-host` ships (`.so`) |
| Bundle | `packaging/box-bundle` | How the helper and dylibs are signed and placed in the app; the flat Linux layout beside `fletch-host` |
| Engine | `crates/fletch-core/src/sandbox/boxes` | Boots boxes for agents: registry, setup, settings, the guest-home mechanism (`home.rs`: seeding, credential sync, session mirrors) |
| Provider adapters | `crates/fletch-core/src/sandbox/provider` | What each provider CLI keeps outside the checkout and how its credential reaches a launch (`ProviderHome`), shared with every engine, and the provider support gate (`supports`) |

## What a box boots

Disks, in order: `base.erofs` read-only as `/`; a per-box sparse ext4 data disk
as `/data` (the agent's tree, home and ssh key live there); the tools disks
(one per provider CLI, built once per CLI version: the owner's provider's,
waited for, then every other provider's that is built by boot time, so a
tenant of another provider — the local reviewer — runs from the same box); then
the image layers. Init (`packaging/box-image/rootfs/opt/fletch/init`) tells
the disks after the data disk apart by content: ext4 is a tools disk, an erofs
with a `/.fletch-layer` marker is a layer. The tools disks' `tools/` are
mounted at `/opt/fletch/tools` (one bound, several overlaid with the first on
top, image 0.3.5 and later). Each layer's directories (`usr`, `etc`, `var`)
become a read-only overlay on the root. Folders the user shares arrive as
virtio-fs at their own paths. Block devices are fixed at boot: a launch whose
provider's disk is not aboard (built after the boot) restarts the box with it,
unless another launch's session is attached — then it is refused with why, and
the disk is aboard at the box's next start.

| Image | Content | Packed |
| --- | --- | ---: |
| `base.erofs` | Debian trixie minbase, bash, git, ssh, curl/jq/rg, Node 22, the shared libraries any Chromium needs, a small font set, `fletch-guest` | ~160 MB |
| `layer-devtools.erofs` | build-essential, pkg-config, python3 + pip + venv, Node headers | ~115 MB |
| `layer-browser.erofs` | Debian `chromium-headless-shell`, `fonts-noto-core`. Playwright projects download their own browser and need only the base; Puppeteer's download is off the allow-list (`storage.googleapis.com`) | ~165 MB |

Every box gets both layers, with no setting. A layer is a read-only file the
boxes share: it costs disk once and no guest memory until something reads it,
and with block devices fixed at boot, one left out could not be added to a
running box.

The layers are the file-level delta between the base and the base with their
packages installed (`packages/<name>.txt`), so nothing is duplicated. Mesa's
software GL and LLVM, locales, docs and man pages are excluded with dpkg path
rules (`dpkg-excludes.cfg`); a headless browser never loads them. Images are
lz4hc erofs (64 KiB clusters, dedupe): the guest kernel decodes LZ4 only.

Every agent process starts through `fletch-guest exec`, which clears the
environment and loads the launch's env file. All ssh sessions are the one
guest user `agent`, so a tenant that must only read its host's files (the
local reviewer) is confined by namespaces instead of a file mode, under one
rule: it writes nothing it shares with the coder (`boxes::confined_view`).
`exec --read-only <dir>` names each of the host agent's identity binds (its
writable root, the home, its mailbox; one nested in another is covered by it)
and each folder the box shares writable that no writable grant of the
reviewer's covers. `--writable <path>` names each path that is the reviewer's
own inside them: its writable root and mailbox, its `CODEX_HOME` (under that
root), and its writable folder grants (a grant inside a share is carved out
of it; a share inside a grant is the reviewer's whole). Every shared location
a CLI writes is a private copy-on-write view instead: `--cow <dir> <store>`
for its provider's state dirs (as the provider adapter lays them out in the
guest, `ProviderHome::shared_state`), the agent scratch dirs and the
project's toolchain cache, `--cow-file <file> <copy>` for the adapter's state
files (`~/.claude.json`), each store and copy under
`<reviewer root>/.fletch-cow/`. The reviewer reads the coder's state there,
and everything it writes, a file that did not exist before included, lands
in its own copy, so nothing it could plant (an MCP config, a hook, a plugin)
is ever run by the coder, with no per-provider list of what a CLI executes.
Its session transcripts are tailed or pulled from its copy. What the host
writes in the guest for the launch, the provider's seed and its credential
files, it writes in the same view (`boxes::executor::Scope`: each operation
runs under `fletch-guest exec` with the launch's options): a seeded config
or a shipped login lands in the reviewer's copy, where its CLI reads it; a
refreshed login is read back from there; a removed one is a whiteout there,
hiding the coder's file rather than exposing it; and the coder's files are
never written. The host creates the directories before the launch, and each
file the view copies, from the seed, where it does not exist yet (the copy
is bound on it); the guest makes the stores and copies inside the
confinement. The command gets a user namespace mapping
`agent` to itself; a mount namespace in which each `<dir>` is bound on itself
with every mount below it and the whole subtree made read-only in one
`mount_setattr(2)`, each `<path>` (a directory or file, reached through no
symbolic link) bound again and made writable, each `--cow` directory covered
by an overlay of itself, with options
`lowerdir=<dir>,upperdir=<store>/upper,workdir=<store>/work,userxattr,index=off`
(`userxattr` is what lets an unprivileged user namespace mount it; with no
index, the host's own operations may mount the same store again), each `--cow-file`
file covered by a bind of its copy (made from it the first time), and
`/data` (the other route to the same trees) covered by an empty tmpfs; and a
pid namespace with a private `/proc`, so no other process's `/proc/<pid>/root`
leads back to the writable view. Every capability is then dropped and
`no_new_privs` set; a namespace the command creates inherits the mounts
locked. A step that fails exits 125 before the command runs. There is no
fallback: a guest without the options (images up to 0.3.0) or one that
cannot set them up refuses the reviewer's launch and asks for the box image
to be updated; the app pins an image whose guest has them. What it does not
cover: scratch outside every bind (`/tmp`, `/var/tmp`, `/run`), which the
reviewer may write as the guest user, and the network namespace, which is the
guest's.

Settings (Settings › Sandbox › Linux VM): memory, CPUs, disk and idle stop, as
scales. Memory and CPUs stop at what the host has (`box_runtime_probe`'s
`host_cpus` and `host_memory_mib`), and disk reaches new boxes only, since a
data disk is sized when it is created. The image URL is on Settings › Developer.

## Releases and updates

An image release is the GitHub release `box-image-v<VERSION>` (VERSION file in
`packaging/box-image`) for aarch64, and `box-image-v<VERSION>-x86_64` for
x86-64 Linux hosts, built from the same inputs with the same file names (a box
never emulates, so each host downloads its own architecture's; the manifest's
`arch` must match): `manifest.json` (per image: file, size, sha256,
packages), the `.erofs` files, and `<file>.chunks.json` delta manifests. The
app boots the one version `PinnedImage::VERSION` names
(`crates/fletch-box/src/image_store/mod.rs`) and downloads it on first use:
the manifest, then the base and the layers, each verified against its
size and sha256 as it arrives. With an older version on disk only the chunks
that changed are fetched (content-defined chunking, HTTP range requests);
a release asset without a chunk list, or a server without ranges, falls back
to a whole download. Installed images live under `<data dir>/box/images/<version>/`.

Two things are coupled and nothing else: the image VERSION and the app's pin.
One rule governs publishing: **a version names one set of inputs**. The inputs
are what the image is made from: `build.sh` (with its pins), `packages/`,
`dpkg-excludes.cfg`, `rootfs/` and `crates/fletch-guest`. `publish.sh` records
their git ids in the release as `inputs.json`; publishing the same inputs again
is a no-op, other inputs under an existing version are an error, so every run
of the Box image workflow checks this first and a change to the image without
a VERSION bump fails at the pull request. The scripts around the image
(`test.sh`, `publish.sh`, the README) are not inputs and need no bump.

A release goes up as a draft, which the app cannot see, and is published only
once every file is there at its size; a published release missing a file its
manifest lists is an error too (bump VERSION). Every build is installed with
the app's own installer before it can be published (`build-docker.sh`), and the
app release workflow refuses to ship a pin whose release is missing files
(`scripts/check-box-image.sh`).

## Workflows

| Workflow | When | Does |
| --- | --- | --- |
| **Box image** (`box-image.yml`) | PR touching `packaging/box-image`, `crates/fletch-guest` | Per architecture: checks VERSION is unreleased or released from these inputs; builds and tests; uploads the images as an artifact; publishes nothing |
| | Push to `main` touching them | Same, then publishes `box-image-v<VERSION>` and `box-image-v<VERSION>-x86_64`; each a no-op when its release with these inputs exists |
| | Run workflow | Same on any branch; `publish` to publish from a branch before merging (so the pin never names a missing release) |
| **Box image bump** (`box-image-bump.yml`) | The 1st of each month, 06:00 UTC, or by hand | `bump.sh`: newest Debian snapshot and Node 22 release, VERSION patch bump, app pin bump, committed and pushed on `chore/box-image-bump`. Builds and tests that commit on both architectures. Tests green: publishes that architecture's release. Either way opens or refreshes the PR with both reports. Merge it to adopt (main's publish then finds the releases in place); close it to skip |

Both run `.github/actions/box-image`, which runs `build-docker.sh --test`, the
same build as on a Mac: fletch-guest cross-build, `build.sh` in a privileged
`debian:trixie` container on the arm64 runner (and on the x86-64 runner for
the x86_64 release), `test.sh` in the same container,
`fletch-box image chunk` for the delta manifests, an install with the app's own
installer; then the report in the job summary. Any failure blocks publishing.

Changing the image by hand (packages, init, exclusions): edit, bump VERSION
and the pin together (`scripts/check-version.sh` fails CI otherwise), open a
PR; the PR build is the test. Then either run **Box image** with `publish` on
the branch before merging, or merge and let `main` publish; until it has, the
pin names a release that is not there yet.

The bump PR is opened with the Actions token, so the repository's regular CI
does not run on it; the bump job ran the image tests itself, and the only code
change is the pin constant.

## Testing

- `test.sh` (CI, or `build-docker.sh --test`): loop-mounts the images, assembles
  the overlay root as init would, and checks from a chroot as `agent`: the base
  tools, sshd, the loader through the overlay, gcc/python/node-gyp from devtools,
  Playwright's own browser on the base libraries, the browser layer's screenshot
  without Mesa/LLVM, fontconfig, every agent CLI installing and starting on the
  base alone.
- `boot-check.sh`: boots a built image with its layers on a Mac, or on a Linux
  host with KVM, through `fletch-box boot --layer` and checks the mounts, the
  tools and a screenshot over ssh. What CI cannot run: init, libkrun, the real
  kernel.
- `crates/fletch-box`: unit tests for the image store (manifests, installs,
  delta downloads against a local HTTP server, chunking); VM tests behind
  `--ignored` need the dev runtime.
- `crates/fletch-core` `boxes::tests`: end to end against a real box
  (`FLETCH_BOX_IMAGE_DIR`, `--ignored`).

## Building locally

```sh
packaging/box-image/build-docker.sh --test       # -> packaging/box-image/out, needs Docker Desktop
packaging/box-image/boot-check.sh                # boots out/ with both layers
```

The build is for the machine's own architecture: a Mac or an arm64 Linux host
builds the aarch64 image, an x86-64 Linux host (with Docker) the x86_64 one.

On a Linux host the box needs KVM: `/dev/kvm` writable by the user
(`sudo usermod -aG kvm $USER`, then log in again). To boot the released image
there without building one:

```sh
eval "$(scripts/box-dev.sh)"                     # builds libkrun.so.1, stages out/, builds the CLI
fletch-box image install --layers all            # this host's architecture's release, pinned version
packaging/box-image/boot-check.sh ~/.fletch/box-dev/images/<version>
```

Point the `box_image_url` setting at `file://<path to out>` to run the app on
a local build (the manifest's version must be the pin).
