# Layered box image: assumptions and the spike that tests them

Status: spike. Nothing here is wired into the app; `build.sh` and the released
image are untouched. The spike builds the proposed layout, mounts it the way the
guest would, and runs the checks below. Run it with Actions → *Box image spike*
(`.github/workflows/box-image-spike.yml`), which uploads the images and a
`report.md`; the VM-only checks are `boot-test.sh` on a Mac.

## The design under test

| Image | Content | Mounted as |
| --- | --- | --- |
| `base.erofs` | Debian trixie minbase, bash, git, ssh, curl/jq/rg, Node 22 (no headers), the shared libraries Playwright's browsers need, a small font set, `fletch-guest` and init | `/` (as today) |
| `layer-devtools.erofs` | build-essential, pkg-config, python3 + pip + venv, Node headers | read-only overlay over `/usr`, `/etc`, `/var` |
| `layer-browser.erofs` | Debian `chromium`, `fonts-noto-core` | same |

A layer is the file-level delta between the base tree and the base tree with the
layer's packages installed (`rsync --compare-dest`), packed as its own erofs with
a `/.fletch-layer` marker. The guest init mounts every erofs disk after the data
disk under `/run/layers/<n>` and overlays each top-level directory the layers
carry onto the root, later disks on top. The tools disk is recognised by its
filesystem (ext4) rather than by its device letter.

All images use `mkfs.erofs -zlz4hc,12 -C65536 -Ededupe -Efragments -Eztailpacking`
instead of today's `-zlz4hc` at the default 4 KiB cluster. The guest kernel
(libkrunfw 6.12) decodes LZ4 only, so zstd is not an option without a custom kernel.

Mesa's software rasteriser (`libgallium`, `libLLVM`, `libz3`, 184 MB) is removed
with dpkg `path-exclude`: `libgbm1` hard-depends on it in trixie, but a headless
browser never loads it. Likewise `/usr/share/locale`, docs and man pages.

## Assumptions and how each is tested

| # | Assumption | Test | Where |
| --- | --- | --- | --- |
| A1 | Headless Chromium renders with `libgbm` present but Mesa/LLVM files excluded | `chromium --headless=new --screenshot` of a page with text, emoji and a canvas produces a PNG; `LD_DEBUG` shows no gallium/LLVM; the files are absent | CI `T-chromium` |
| A2 | The base's library set (Playwright's debian13 list + fontconfig) is enough for Playwright's own downloaded browser, no browser layer needed | `npm i playwright && npx playwright install chromium-headless-shell`, then a screenshot script, on base+devtools | CI `T-playwright` |
| A3 | No agent CLI needs python, gcc or anything beyond the slim base to install and start | claude, codex, cursor, opencode and pi installed and `--version`ed as `agent` on the base alone | CI `T-agents-*` |
| A4 | `openssh-server` works with `systemd-standalone-sysusers` in place of systemd | `sshd -t` with a generated host key; the `sshd` user exists; no `systemd` package in the base | CI `T-sshd` |
| A5 | Dropping locales, docs and man pages under `LANG=C.UTF-8` breaks nothing | Every test runs with that environment | CI (all) |
| A6 | A layer's delta fits in `/usr`, `/etc` and `/var`, and removes no base file | The build lists each delta's top-level dirs and fails on others; devtools and browser work through the overlay | CI build + `T-dev`, `T-chromium` |
| A7 | The loader finds layer libraries without a merged `/etc/ld.so.cache` | `python3` (libpython lives only in devtools) and `chromium` start; `ldd` on layer binaries reports nothing missing | CI `T-ldd` |
| A8 | The browser layer's fontconfig cache overlays the base's, so the system Chromium does not rebuild caches into `$HOME` (Playwright's browser bundles its own fontconfig and keeps a cache in `$HOME` on any image; `T-fontcache-pw` only records that) | After `T-chromium`, `~/.cache/fontconfig` is absent or empty | CI `T-fontcache` |
| A9 | Overlay lookup overhead is acceptable | Cold `ls -lR /usr` on the plain base vs the three-layer merge, caches dropped | CI `T-overlay-perf` |
| A10 | The new `mkfs.erofs` flags shrink the images substantially and the guest kernel reads them | `sizes.md` compares old and new flags on the same tree; the Mac boot mounts them | CI `T-sizes`, Mac |
| A11 | Builds are reproducible | `mkfs.erofs` twice on the same tree gives the same sha256 | CI `T-repro` |
| A12 | A rebuilt image shares most bytes with its predecessor under content-defined chunking | `chunk-delta.py` between the base and a variant with one new file and two modified files; expect over 80 % reuse | CI `T-delta` |
| A13 | GitHub release assets answer HTTP range requests | `curl -r 0-1023` on an existing `box-image-v*` asset returns 206 | CI `T-range` |
| A14 | Init can mount layer disks and overlay `/usr`, `/etc`, `/var` on the live root; ssh, `fletch-guest` and the tools disk keep working | `boot-test.sh`: boot base + two layers, check mounts, gcc, python3, chromium, `fletch-guest exec` over ssh | Mac |
| A15 | libkrun attaches five virtio-blk disks (base, data, tools, two layers) | Same boot | Mac |
| A16 | Boot time and idle memory do not regress | `boot-test.sh` prints boot time and `free -m`; compare with image 0.2.0 | Mac |
| A17 | Size targets: base ≤ 200 MB, devtools ≤ 150 MB, browser ≤ 180 MB packed | `sizes.md` | CI `T-sizes` |

Not covered by the spike: the host-side delta downloader, the layer settings UI,
host page-cache sharing between boxes, and the scheduled dependency-bump workflow
(it reuses the snapshot pinning the build already relies on).

## Files

| File | What |
| --- | --- |
| `packages/{base,devtools,browser,browser-shell}.txt` | The package split; `browser-shell` is a variant of `browser` with Debian's `chromium-headless-shell`, built and measured beside it |
| `dpkg-excludes.cfg` | dpkg `path-exclude` rules, applied at build and persisted in the image for layer installs |
| `init` | The guest init with layer mounting (a copy of `../rootfs/opt/fletch/init` plus one step) |
| `build-layers.sh` | Builds base and layers, three base variants for size comparison, the delta variant, `manifest.json`, `sizes.md` |
| `test-layers.sh` | Loop-mounts the images, assembles the overlay root, runs the T-* checks, writes `report.md` |
| `chunk-delta.py` | Gear-hash content-defined chunking of two files, reports reused bytes |
| `run.sh` | Container entrypoint: build then test |
| `boot-test.sh` | Mac: boots the artifact with `fletch-box boot --layer …` and checks the guest over ssh |
