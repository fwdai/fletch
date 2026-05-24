# Building the base Tart image

The app spawns agents by **cloning** a pre-baked base image. You build that
base image once. Cloning is APFS copy-on-write, so each agent VM costs
essentially zero extra disk and starts in seconds.

This walkthrough produces a base image called `base-dev` running Ubuntu 22.04
with Node, git, and the Claude Code CLI installed, plus the keypair the app
generated baked into `authorized_keys`.

## Prerequisites

- The app has been run at least once (so the SSH keypair exists at
  `~/Library/Application Support/com.algiers.app/id_ed25519_algiers.pub`)
- Tart is on your PATH — either via the app's bundled copy or `brew install
  cirruslabs/cli/tart`. (In dev you can use the bundled copy directly:
  `./src-tauri/binaries/tart-aarch64-apple-darwin`.)

## Steps

### 1. Pull the upstream Ubuntu base

```bash
tart clone ghcr.io/cirruslabs/ubuntu:latest base-dev
```

### 2. Boot it

```bash
tart run base-dev
```

Leave this running in one terminal. In another, get its IP:

```bash
tart ip base-dev
```

### 3. SSH in (initial password is `admin`)

```bash
ssh admin@$(tart ip base-dev)
# password: admin
```

### 4. Inside the VM, install dependencies

```bash
sudo apt-get update
sudo apt-get install -y curl git build-essential ca-certificates

# Node.js LTS via NodeSource
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt-get install -y nodejs

# Claude Code CLI
sudo npm install -g @anthropic-ai/claude-code

# Confirm
claude --version
```

### 5. Bake in the host's public key

Copy the contents of `~/Library/Application Support/com.algiers.app/id_ed25519_algiers.pub`
on your **host machine** (the app shows this under a "Show SSH key" button if
you wire one up; for now just `cat` it). Then inside the VM:

```bash
mkdir -p ~/.ssh && chmod 700 ~/.ssh
echo 'ssh-ed25519 AAAA…  algiers-agent' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

### 6. Allow passwordless sudo for the mount step

The app's spawn flow runs `sudo mount -t virtiofs workspace /workspace`. Make
that not prompt:

```bash
echo 'admin ALL=(ALL) NOPASSWD: /usr/bin/mount, /usr/bin/umount' | \
  sudo tee /etc/sudoers.d/algiers
sudo chmod 440 /etc/sudoers.d/algiers
```

### 7. Pre-create the mount point

```bash
sudo mkdir -p /workspace
sudo chown admin:admin /workspace
```

### 8. Shut down cleanly

```bash
sudo shutdown -h now
```

Back on the host, the `tart run` foreground process will exit. `base-dev` is
now your reusable image. **Do not run it again directly** — it's now the
source for clones.

### 9. Verify

```bash
tart list
# you should see base-dev listed.
```

In the app's "Choose repo…" dialog, enter `base-dev` as the base image. Spawn
an agent. It will clone `base-dev` into a per-agent VM, boot in seconds, and
launch `claude` against the mounted worktree.

## Updating the base image

Right now: rebuild from scratch by deleting `base-dev` and repeating the
steps. Future improvement: declarative base-image config + `algiers
rebuild-base`.

## Caveats

- **virtiofs reliability** on Linux guests in Tart is the highest-risk piece
  of v1. If `mount -t virtiofs workspace /workspace` errors with `unknown
  filesystem type`, your kernel may need the virtiofs module enabled. The
  cirruslabs ubuntu image as of late 2025 ships with it built in; if you
  build from a different Ubuntu source you may need
  `sudo modprobe virtiofs` and add it to `/etc/modules`.
- **Disk size**: APFS clones mean N agents add ~no disk overhead until they
  start writing. A heavy build (e.g. `cargo build` in a large workspace) can
  push real usage into the GBs per agent — keep an eye on it.
