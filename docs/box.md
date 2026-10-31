# Boxes: the VM sandbox

A box is a libkrun micro-VM per agent on Apple silicon: no network device, no
shared folder unless the user grants one, reached from the host only over SSH
on vsock and an HTTP CONNECT egress proxy with an allow-list. The guest holds
the agent's tree at the same absolute paths the host uses. This page is the map;
each part's README has the detail.

| Part | Where | What |
| --- | --- | --- |
| Runtime | `crates/fletch-box` | libkrun helper (`fletch-vm`), VM spec, SSH channel, egress proxy, image store, tools disks; the `fletch-box` CLI for manual use |
| Guest agent | `crates/fletch-guest` | Static musl binary in the image: vsock listeners, `exec`, `tail`, the stop hook |
| Image | `packaging/box-image` | The Debian root every box boots and its layers; `build.sh`, `test.sh`, `bump.sh`, `publish.sh` |
| libkrun | `packaging/libkrun` | libkrun 1.19 + patches and the libkrunfw 6.12 kernel the app ships |
| Bundle | `packaging/box-bundle` | How the helper and dylibs are signed and placed in the app |
| Engine | `crates/fletch-core/src/sandbox/boxes` | Boots boxes for agents: registry, setup, settings, provider homes |

## What a box boots

Disks, in order: `base.erofs` read-only as `/`; a per-box sparse ext4 data disk
as `/data` (the agent's tree, home and ssh key live there); the provider's
tools disk (the agent CLI, built once per CLI version); then the image layers.
Init (`packaging/box-image/rootfs/opt/fletch/init`) tells the disks after the
data disk apart by content: ext4 is the tools disk, an erofs with a
`/.fletch-layer` marker is a layer. Each layer's directories (`usr`, `etc`,
`var`) become a read-only overlay on the root. Folders the user shares arrive
as virtio-fs at their own paths.

| Image | Content | Packed |
| --- | --- | ---: |
| `base.erofs` | Debian trixie minbase, bash, git, ssh, curl/jq/rg, Node 22, the shared libraries any Chromium needs, a small font set, `fletch-guest` | ~160 MB |
| `layer-devtools.erofs` | build-essential, pkg-config, python3 + pip + venv, Node headers. On by default | ~115 MB |
| `layer-browser.erofs` | Debian `chromium-headless-shell`, `fonts-noto-core`. Off by default; Playwright projects download their own browser and need only the base | ~165 MB |

The layers are the file-level delta between the base and the base with their
packages installed (`packages/<name>.txt`), so nothing is duplicated. Mesa's
software GL and LLVM, locales, docs and man pages are excluded with dpkg path
rules (`dpkg-excludes.cfg`); a headless browser never loads them. Images are
lz4hc erofs (64 KiB clusters, dedupe): the guest kernel decodes LZ4 only.

Settings (Settings › Sandbox › Box): memory, CPUs, idle stop, image URL, and a
toggle per layer (`box_layers`). A layer turned on is downloaded once and
attached to every box from its next boot.

## Releases and updates

An image release is the GitHub release `box-image-v<VERSION>` (VERSION file in
`packaging/box-image`): `manifest.json` (per image: file, size, sha256,
packages), the `.erofs` files, and `<file>.chunks.json` delta manifests. The
app boots the one version `PinnedImage::VERSION` names
(`crates/fletch-box/src/image_store/mod.rs`) and downloads it on first use:
the manifest, then the base and the enabled layers, each verified against its
size and sha256 as it arrives. With an older version on disk only the chunks
that changed are fetched (content-defined chunking, HTTP range requests);
a release asset without a chunk list, or a server without ranges, falls back
to a whole download. Installed images live under `<data dir>/box/images/<version>/`.

Two things are coupled and nothing else: the image VERSION and the app's pin.
One rule governs publishing: **a version names one set of inputs**. `publish.sh`
records the source trees (`packaging/box-image`, `crates/fletch-guest`) in the
release as `inputs.json`; publishing the same inputs again is a no-op, other
inputs under an existing version are an error, so every run of the Box image
workflow checks this first and a change without a VERSION bump fails at the
pull request.

## Workflows

| Workflow | When | Does |
| --- | --- | --- |
| **Box image** (`box-image.yml`) | PR touching `packaging/box-image`, `crates/fletch-guest` | Checks VERSION is unreleased or released from these inputs; builds and tests; uploads the images as an artifact; publishes nothing |
| | Push to `main` touching them | Same, then publishes `box-image-v<VERSION>`; a no-op when the release with these inputs exists |
| | Run workflow | Same on any branch; `publish` to publish from a branch before merging (so the pin never names a missing release) |
| **Box image bump** (`box-image-bump.yml`) | Mondays 06:00 UTC, or by hand | `bump.sh`: newest Debian snapshot and Node 22 release, VERSION patch bump, app pin bump, committed on `chore/box-image-bump`. Builds and tests that commit. Tests green: publishes the release. Either way opens or refreshes the PR with the report. Merge it to adopt (main's publish then finds the release in place); close it to skip |

Both run `.github/actions/box-image`: fletch-guest cross-build, `build.sh` in
a privileged `debian:trixie` container on the arm64 runner, `test.sh` in the
same container, `fletch-box image chunk` for the delta manifests, the report in
the job summary. Test failures block publishing.

Changing the image by hand (packages, init, exclusions): edit, bump VERSION,
open a PR; the PR build is the test. Then either run **Box image** with
`publish` on the branch and bump the pin in the same PR, or merge and let
`main` publish, then bump the pin in a follow-up.

The bump PR is opened with the Actions token, so the repository's regular CI
does not run on it; the bump job ran the image tests itself, and the only code
change is the pin constant.

## Testing

- `test.sh` (CI, or `build-mac.sh --test`): loop-mounts the images, assembles
  the overlay root as init would, and checks from a chroot as `agent`: the base
  tools, sshd, the loader through the overlay, gcc/python/node-gyp from devtools,
  Playwright's own browser on the base libraries, the browser layer's screenshot
  without Mesa/LLVM, fontconfig, every agent CLI installing and starting on the
  base alone.
- `boot-check.sh`: boots a built image with its layers on a Mac through
  `fletch-box boot --layer` and checks the mounts, the tools and a screenshot
  over ssh. What CI cannot run: init, libkrun, the real kernel.
- `crates/fletch-box`: unit tests for the image store (manifests, installs,
  delta downloads against a local HTTP server, chunking); VM tests behind
  `--ignored` need the dev runtime.
- `crates/fletch-core` `boxes::tests`: end to end against a real box
  (`FLETCH_BOX_IMAGE_DIR`, `--ignored`).

## Building locally

```sh
packaging/box-image/build-mac.sh --test          # -> packaging/box-image/out, needs Docker Desktop
packaging/box-image/boot-check.sh                # boots out/ with both layers
```

Point the `box_image_url` setting at `file://<path to out>` to run the app on
a local build (the manifest's version must be the pin).
