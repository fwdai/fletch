# Tart Agent Orchestrator — v1 Design

**Working name:** `algiers` (placeholder — final name TBD)
**Status:** Approved for implementation planning
**Date:** 2026-05-24

## Goal

A Tauri desktop app that lets the user spawn multiple Claude Code agents on a single repo, each running in:

- Its own **git worktree** (so file edits don't collide), and
- Its own **Tart VM** (so "yolo mode" — `claude --dangerously-skip-permissions` — can't damage the host).

The v1 bar: spawn N agents on the same repo, each works in parallel without conflicting with the others, the user can watch and interject via a per-agent terminal, and shutting an agent down leaves no residue.

## Non-goals (v1)

- Multi-repo workspaces
- Remote / cloud agents
- Automated base-image build pipeline (build script lives in repo, run manually)
- In-app diff viewer, PR creation, log search
- Agent-to-agent coordination
- Persistent agent state across app restarts (nice-to-have; not a blocker for v1)

## Architecture

```
┌───────────────────────────────── Host (macOS) ─────────────────────────────────┐
│                                                                                │
│  Tauri app                                                                     │
│  ┌─────────────────────┐         ┌────────────────────────────────────┐        │
│  │   Web frontend      │◀──IPC──▶│   Rust backend                     │        │
│  │   - Agent list      │         │   - AgentSupervisor                │        │
│  │   - xterm.js panes  │         │   - VM lifecycle (shells out to    │        │
│  │   - Spawn dialog    │         │     `tart`, `git`, `ssh`)          │        │
│  └─────────────────────┘         │   - PTY I/O bridge (ssh ↔ xterm)   │        │
│                                  │   - State persisted to JSON file   │        │
│                                  └────────────┬───────────────────────┘        │
│                                               │                                │
│  ~/repo/.worktrees/<agent-id>/  ◀────virtiofs────┐                             │
│                                               │  │                             │
│                                ┌──────────────┴──┴───────────────┐             │
│                                │  Tart VM: agent-<id>            │             │
│                                │  (CoW clone of base image)      │             │
│                                │  - Ubuntu 22.04                 │             │
│                                │  - /workspace ← virtiofs mount  │             │
│                                │  - claude --dangerously-...     │             │
│                                └─────────────────────────────────┘             │
│                                       (one VM per agent, N in parallel)        │
└────────────────────────────────────────────────────────────────────────────────┘
```

## Components

### Frontend (TypeScript + a web framework — likely SolidJS or Svelte for size, TBD in plan)

- **AgentList** — left rail, shows all agents with status badges (`spawning`, `running`, `idle`, `stopped`, `error`). "Spawn agent" button.
- **SpawnDialog** — task prompt textarea, branch name input (default `agent/<short-id>`), submit.
- **AgentPane** — tabbed right panel, one xterm.js per agent, bound to the agent's PTY stream.
- **WorkspaceBar** — top bar showing current repo path and a "Choose repo…" button.

### Backend (Rust)

- **`workspace`** — load/save the workspace JSON, resolve repo paths, manage `.worktrees/` directory.
- **`vm`** — thin wrapper over the `tart` CLI: `clone`, `run`, `ip`, `stop`, `delete`. Returns typed errors.
- **`agent`** — represents one agent's full lifecycle. Owns its VM name, worktree path, PTY handle, and state. Exposes `spawn()`, `attach()`, `kill()`.
- **`supervisor`** — registry of all live agents. Routes Tauri IPC commands to the right agent. Emits state-change events to the frontend.
- **`pty_bridge`** — opens an SSH session with a PTY, pumps bytes between the SSH channel and a Tauri event stream so xterm.js sees a live terminal. Handles resize events from the frontend.

Each module is small, single-purpose, and testable in isolation (the `vm` module can be mocked at the CLI-invocation boundary for `agent`/`supervisor` tests).

## Data flow

### Spawn an agent

1. User clicks "Spawn agent", enters task + branch name.
2. Backend creates worktree: `git worktree add .worktrees/<agent-id> -b <branch>`.
3. Backend clones base VM: `tart clone <base> agent-<id>`.
4. Backend starts VM in background: `tart run agent-<id> --no-graphics --dir=workspace:<abs-worktree-path>`.
5. Backend polls `tart ip agent-<id>` until SSH is reachable.
6. Backend SSHes in and runs a small init: `sudo mount -t virtiofs workspace /workspace`.
7. Backend opens an SSH PTY session: `ssh -t … "cd /workspace && claude --dangerously-skip-permissions '<task>'"`.
8. Backend pipes PTY bytes to xterm.js via a Tauri event channel keyed by agent ID.
9. Frontend opens (or focuses) the agent's terminal tab.

State transitions: `spawning → running → idle/error/stopped`.

### Interact

User types in xterm.js → bytes go over Tauri IPC → backend writes them to the SSH PTY stdin → claude sees them. Output is the reverse path. Resize events propagate from xterm.js down to the SSH PTY.

### Teardown

1. User clicks "Stop" on an agent.
2. Backend closes the SSH session.
3. Backend runs `tart stop agent-<id>` then `tart delete agent-<id>`.
4. Worktree is **kept by default** (user may have un-committed work to inspect); a separate "Discard worktree" button runs `git worktree remove --force .worktrees/<agent-id>`.

## Base image

Built manually for v1. Documented in `scripts/build-base-image.md`:

1. `tart clone ghcr.io/cirruslabs/ubuntu:latest base-dev`
2. `tart run base-dev` (boot it)
3. SSH in, install: node, npm, git, build-essentials, `@anthropic-ai/claude-code`, common languages as needed.
4. Add the host's SSH public key to `~/.ssh/authorized_keys`.
5. Configure passwordless `sudo` for the default user (needed for the virtiofs mount step).
6. `tart stop base-dev`. Done — `base-dev` is now the source image.

The app reads the base-image name from workspace config (default: `base-dev`).

## State persistence

A single JSON file at `~/Library/Application Support/<app>/workspaces.json`:

```json
{
  "workspaces": [
    {
      "repo_path": "/Users/alex/code/foo",
      "base_image": "base-dev",
      "agents": [
        { "id": "a1b2", "name": "refactor-auth", "branch": "agent/a1b2",
          "status": "stopped", "task": "...", "created_at": "..." }
      ]
    }
  ]
}
```

Live PTY sessions are **not** persisted — if the app restarts, agents show as `stopped` (their VMs may still be running; we surface that and offer "reattach" or "kill"). v1 is fine with "restart the app = restart your agents."

## Error handling

Errors that need first-class handling (each surfaces as a toast + per-agent error state):

- **Base image missing** — direct the user to the build script.
- **Tart not installed / wrong version** — preflight check on app start.
- **VM fails to boot / SSH never reachable** — 60s timeout, then mark agent `error`, leave VM in place for inspection, expose "Force destroy" button.
- **virtiofs mount fails** — caught at step 6 of spawn; this is the spike-0 risk. If detected, surface a clear error pointing at the fallback path (see Open questions).
- **Worktree creation fails** (branch conflict, dirty index) — fail before touching the VM, show git's error verbatim.
- **Claude CLI exits unexpectedly** — agent state goes `idle`; user can re-run from the terminal pane.

Everything else (network blips on the SSH channel, transient `tart` failures) is best-effort retried once, then surfaced.

## Testing approach

- **`vm` module**: unit tests mock the CLI runner; verify we build correct argv and parse output correctly. Plus one integration test gated on `TART_AVAILABLE=1` that actually clones/runs/deletes a tiny VM.
- **`agent` / `supervisor`**: unit tests with a fake VM backend, covering state-machine transitions and error paths.
- **`pty_bridge`**: hardest to test — start with manual verification, add an integration test using a local ssh-to-localhost loop once the shape settles.
- **Frontend**: component tests for AgentList state rendering. xterm.js binding is verified manually.
- **End-to-end smoke test**: a script that spawns one agent, sends `echo hello`, asserts it appears in the captured output, then tears down. Runs on the dev machine, not CI (CI doesn't have Apple Silicon + Tart).

## Open questions / spike-0

**Critical, must resolve before substantive implementation:**

1. **virtiofs from Linux guest under Tart.** Does `--dir=workspace:<path>` + `mount -t virtiofs workspace /workspace` work reliably on a current Tart + cirruslabs Ubuntu image? Write performance acceptable for `npm install` / `cargo build`?
   - **Fallback if it fails:** Use `git clone --shared` over SSH from a host-side bare mirror, agents push back. Documented but not implemented in v1 unless needed.

**Smaller open questions, OK to resolve during planning:**

2. Frontend framework choice — SolidJS, Svelte, or plain TS? (Lean: Svelte for ecosystem + Tauri examples.)
3. App name — placeholder is `algiers` (branch name); pick before first release.
4. SSH key management — generate a dedicated keypair on first run, or reuse `~/.ssh/id_ed25519`? (Lean: generate a dedicated one, store under app data dir.)
5. Concurrency cap — should the UI prevent spawning more than `<system-RAM / 2GB>` agents, or just let the user push it? (Lean: soft warning, no hard cap.)
