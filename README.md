<div align="center">

<h1>
  <img src="docs/fletch_dark.png" alt="" width="48" valign="middle">
  Fletch
</h1>

### Run a fleet of AI coding agents in parallel — each in its own sandbox and git worktree.

Claude Code, Codex, Cursor, OpenCode, and more — all on one repo, at the same time, without stepping on each other.

[![Download for macOS](https://img.shields.io/badge/Download%20for%20macOS-Apple%20Silicon%20%26%20Intel-111?style=for-the-badge&logo=apple)](https://fletch.sh)

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
![Platform: macOS 13+](https://img.shields.io/badge/Platform-macOS%2013%2B-lightgrey.svg)
![Built with Tauri](https://img.shields.io/badge/Built%20with-Tauri%202-24C8DB.svg)

<!-- Drop a short demo GIF here. Record one full loop: spawn an agent → it works → review the diff → commit/PR. Save it to docs/demo.gif. -->
<img src="docs/fletch_control_room.jpg" alt="Fletch: spawning multiple AI coding agents in parallel, each in its own git worktree" width="820">

</div>

---

One coding agent at a time is a bottleneck. You wait, review, and repeat — serially. Fletch lets you run a dozen at once, each in its own git worktree and macOS sandbox, so they physically can't touch each other's files or the rest of your machine.

Kick off five agents on five tasks. Watch each as a clean chat or its raw terminal. See edits land as live diffs. Commit, push, or open a PR — all from one window. Spawn another when you need more throughput; discard one — worktree and branch with it — in a click.

## Why Fletch

- **Actual parallelism.** Every agent gets its own git worktree and branch. Two agents editing the same file is impossible — they're on different checkouts of the same repo. No collisions, no locking, no waiting your turn.
- **Sandboxed by default.** Each agent runs under a per-agent macOS `sandbox-exec` profile that blocks writes outside its worktree. It can't trash your repo, your other agents, or your machine.
- **Your agents, your keys.** Fletch drives the CLIs you already installed and pay for — Claude Code, Codex, Cursor, OpenCode, Pi. No extra subscription, no model lock-in, no proxy.
- **One cockpit per agent.** Normalized chat view of reasoning and tool calls, a native terminal for the raw TUI, live diffs as edits land, and an integrated Git/PR panel — plus optional Run and Terminal panels.
- **From clone to PR without a shell.** Start a project from a GitHub repo or a fresh one, work it with agents, and ship the PR — never dropping to the command line.
- **Fully local.** A native desktop app. Your code and transcripts stay on your machine; nothing routes through a Fletch server.

## Supported agents

Fletch normalizes every agent's transcript into one consistent view — they all look and behave the same to you.

| Agent                                                             | Status                      |
| ----------------------------------------------------------------- | --------------------------- |
| [Claude Code](https://docs.anthropic.com/en/docs/claude-code)     | ✅ Supported                |
| [Codex](https://github.com/openai/codex)                          | ✅ Supported                |
| [Cursor Agent](https://cursor.com)                                | ✅ Supported                |
| [OpenCode](https://opencode.ai)                                   | ✅ Supported                |
| [Pi](https://pi.dev/)                                             | ✅ Supported                |
| [Antigravity](https://antigravity.google/product/antigravity-cli) | ✅ Supported (experimental) |

Install and authenticate the CLIs you want; Fletch detects them on your `PATH` and common install locations.

## Download

**[Download Fletch for macOS →](https://fletch.sh)** — universal (Apple Silicon & Intel), signed, notarized, and self-updating.

**Requirements**

- macOS 13+ (`sandbox-exec` is macOS-only)
- At least one supported agent CLI (e.g. `claude`)
- `git`, plus `gh` (GitHub CLI) for PR and project-creation features

No VM, no Docker, no containers.

## How it works

1. **Point** Fletch at a local git repo.
2. **Spawn** an agent with a task — Fletch creates an isolated per-agent workspace at `~/.fletch/workspaces/<id>/`, with the repo checked out in a subdir as a lightweight clone that shares your repo's git objects.
3. **Run** the agent's CLI in that checkout under `sandbox-exec`, bridged through a local PTY.
4. **Watch** output stream into a tab — switch between the normalized chat view and the raw terminal.
5. **Review** edits as live diffs; browse and edit files directly.
6. **Ship** — commit, push, open or merge a PR, or discard the agent and its workspace in one click.

Every session is captured to a local SQLite store, so reopening it replays the full transcript.

## Workflows

Spawning agents one at a time is great for ad-hoc work. For a repeatable process — *plan, implement in parallel, review in a loop, ship* — Fletch has **workflows**: a definition you build once and launch on any task.

A workflow is a tree of **blocks**:

- **Step** — one sandboxed agent with a **goal** and a **gate** (the condition that means it's done: wrote a `verdict.json`, moved `HEAD`, produced an artifact, passed the project's tests, or got your approval).
- **Parallel** — fan several agents out from the same point, with a join policy (`all` / `any`) and optional merge of their work.
- **Loop** — repeat a body (e.g. `review → fix`) until a step's verdict says `done` or a max iteration count is hit.
- **Orchestrate** — an orchestrator agent supervises child agents, answers their questions, and can dynamically compose bounded sub-workflows.

The engine — not an LLM — owns control flow. It's built to be **robust and observable**:

- **Filesystem handoffs.** Each run gets a per-run *blackboard* mounted read-write into every step's sandbox; steps leave notes and a structured verdict for the next agent.
- **Host-brokered messaging.** Steps `report` / `ask` along edges you declare; questions with no orchestrator pause the run for *you* to answer.
- **Bounded spend.** Turn, iteration, wall-clock, and (where the provider exposes it) token budgets are enforced. Exceeding one pauses the run — it never silently overspends or dies.
- **No silent hangs.** Every wait has a deadline; every pause names its cause (approval, question, blocked gate, budget, conflict, stall) and offers its action in the monitor.
- **Fully resumable.** Every engine decision is an append-only journal event; runs resume after an app restart, and each step attempt's chat is preserved and replayable forever.

**Using it:** define a workflow in **Settings → Workflows** (or import a shareable YAML file); launch it on a task from a new workspace's **Workflow** tab; watch it in the run monitor — a live timeline, each attempt's chat, a budget meter, and banners for anything that needs you. Finalize pushes the run branch and opens a PR.

## Isolation & security

Each agent runs as **your user** under a per-agent `sandbox-exec` profile that denies writes by default, re-allowing only:

- the agent's workspace root at `~/.fletch/workspaces/<id>/` (its per-repo checkouts live in subdirs) and its RPC mailbox at `~/.fletch/rpc/<id>/`
- temp dirs: `/private/tmp`, `/private/var/tmp`, and `/private/var/folders`
- per-user app state: `~/.claude` and `~/.claude.json` (plus `CLAUDE_CONFIG_DIR` when set), `~/.npm`, `~/.cache`, `~/.config`, `~/.local`, `~/Library/Caches`, and `~/Library/Application Support`
- each per-turn agent's own on-disk state store: `~/.codex`, `~/.cursor`, `~/.gemini`, and `~/.pi`
- the PTY and device files terminal programs need (`/dev/null`, `/dev/zero`, `/dev/tty*`, `/dev/ptmx`, `/dev/pts/*`)

See [`src-tauri/src/sandbox/seatbelt.rs`](src-tauri/src/sandbox/seatbelt.rs) for the exact profile — the code is canonical. Run-panel processes (a project's setup/dev command) use a broader profile that additionally grants toolchain dirs like `~/.cargo`, `~/.rustup`, and `~/go` so real project builds succeed.

This is a **write-protection sandbox, not a VM**: an agent can still read what your user can read and reach the network. It's "can't trash your repo or machine," not "air-gapped." Choose tasks accordingly.

### Workspaces are clones, not worktrees

Each workspace checkout is a `git clone --shared` of your repo, not a linked `git worktree`. A linked worktree's `.git` file points back inside your real repo's `.git` — agents would need write access there, and a writable `.git/hooks` means code execution on your host the next time you run git. With a shared clone, the agent's entire writable `.git` lives inside its sandboxed workspace: **your repo's `.git` is never writable by an agent**. The clone borrows your repo's object store instead of copying it, so spawning an agent costs kilobytes and milliseconds regardless of history size, and your repo stays pristine — no worktree entries, no "branch already checked out elsewhere" collisions, and discarding an agent just deletes its directory. The flip side of borrowing objects is that a workspace is tied to its source repo staying in place. See [`src-tauri/src/sandbox/provision.rs`](src-tauri/src/sandbox/provision.rs) for the full design.

### Remote actions & credentials

Agents can ask the host to run a small, fixed set of git operations on their behalf through an RPC broker: `git push`, PR creation (`open_pr`), and a credentialed `git fetch`. These run on the host with your GitHub credentials — and today they run **without a confirmation prompt**. The mitigation is narrow but real: the credentials never enter the sandbox (the ops are brokered host-side), and the broker accepts only that closed, small op set. The consequence is that an agent can publish code and open pull requests under your identity, so choose tasks and repos accordingly.

## Build from source

```bash
bun install
bun tauri dev
```

**Toolchain:** [Bun](https://bun.com) 1.3+ and a stable Rust toolchain. Frontend is React 18 + TypeScript + Zustand + xterm.js; backend is Rust via [Tauri 2](https://tauri.app).

```
src/                  React + TypeScript frontend (store, adapters, components)
src-tauri/src/        Rust backend (supervisor, sessions, sandbox, git, gh)
src-tauri/migrations/ SQLite schema
```

Run the tests:

```bash
bun run test                  # frontend (vitest)
cd src-tauri && cargo test    # backend
```

## Contributing

Issues and pull requests welcome. Planning a larger change? Open an issue first so we can align on direction. Keep PRs focused and run both test suites before submitting.

## License

[AGPL-3.0](LICENSE). See [NOTICE](NOTICE) for attribution.
