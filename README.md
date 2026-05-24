# algiers

> Spawn multiple [Claude Code](https://docs.anthropic.com/en/docs/claude-code) agents on the same repo, each in its own git worktree and its own Tart-managed VM. Yolo mode (`--dangerously-skip-permissions`) is safe because the host filesystem is out of reach.

**Status:** v1 / pre-release. Apple Silicon only.

## What it does

- You point it at a git repo.
- You click "Spawn agent", give it a task in plain English.
- It creates a `.worktrees/<id>` for the agent on a fresh branch.
- It clones a pre-baked Tart VM (Ubuntu + claude code) — fast, near-zero disk thanks to APFS copy-on-write.
- It mounts the worktree into the VM via virtiofs at `/workspace`.
- It SSHes in, launches `claude --dangerously-skip-permissions`, and pipes the terminal to a xterm.js pane in the app.
- N agents in parallel, fully isolated from each other and from your host.

## Architecture (one paragraph)

A Tauri 2 app. The frontend is Svelte 5 + xterm.js. The Rust backend owns a `Supervisor` that wraps a small `Vm` module (shells out to `tart`), a `WorkspaceManager` (persists a single JSON file in `~/Library/Application Support/com.algiers.app/`), and a `pty_bridge` (local PTY around `ssh -tt` so terminal resize + interactive prompts work). Tart itself is bundled as a Tauri sidecar binary so end users don't need to `brew install` anything. The full design is in [`docs/superpowers/specs/`](docs/superpowers/specs/).

## Requirements

- macOS 13+ on Apple Silicon (Virtualization.framework + Tart's hard requirement)
- Xcode command-line tools (for `git`, `ssh`, `ssh-keygen`)
- Node 20+, npm
- Rust (stable)

That's it for development. End users of the packaged app need none of the above.

## First-time setup

```bash
# 1. Clone, install JS deps. postinstall fetches the latest Tart release
#    into src-tauri/binaries/ where Tauri's sidecar bundler expects it.
npm install

# 2. Run the dev app. First run will:
#    - generate ~/Library/Application Support/com.algiers.app/id_ed25519_algiers
#      (the keypair used to SSH into guest VMs)
npm run tauri dev
```

## Building the base image (one-time, manual for v1)

The app expects a Tart VM named e.g. `base-dev` to exist before you spawn an agent. See [`scripts/build-base-image.md`](scripts/build-base-image.md) for the walk-through.

Short version:
```bash
tart clone ghcr.io/cirruslabs/ubuntu:latest base-dev
tart run base-dev   # in another terminal, ssh in and install node/git/claude/etc.
# Bake in the host's public key (printed in the app's UI under "Show SSH key")
# Then stop the VM — base-dev is now your reusable source image.
tart stop base-dev
```

## Project layout

```
src/                       # Svelte 5 frontend
  lib/api.ts               # typed Tauri IPC + event wrappers
  lib/store.svelte.ts      # reactive store (runes)
  lib/*.svelte             # WorkspaceBar, AgentList, AgentPanes, AgentTerminal, SpawnDialog
src-tauri/
  src/lib.rs               # Tauri setup, sidecar resolution, IPC registration
  src/vm.rs                # Tart CLI wrapper (mockable via TartCli trait)
  src/git.rs               # git worktree add/remove
  src/workspace.rs         # workspace JSON state + agent records
  src/keys.rs              # ed25519 keypair generation
  src/pty_bridge.rs        # SSH-over-PTY plumbing
  src/agent.rs             # per-agent spawn/shutdown lifecycle
  src/supervisor.rs        # registry + Tauri command coordination
  src/commands.rs          # #[tauri::command] handlers (thin)
  binaries/                # bundled tart binary (gitignored, fetched by postinstall)
  resources/third-party/   # licenses for bundled binaries
scripts/
  download-tart.sh         # postinstall: fetch latest tart release
  build-base-image.md      # base VM image walkthrough
  generate-placeholder-icons.py
docs/superpowers/specs/    # design doc(s)
```

## Roadmap (out of v1 scope)

- Multi-repo workspaces
- Remote / cloud agents
- In-app diff viewer / PR creation
- Auto base-image building from a declarative spec
- Agent-to-agent coordination

## License

TBD. Tart (Apache-2.0) is redistributed under its own license; see `src-tauri/resources/third-party/tart/`.
