<div align="center">

# Quorum

### Run a fleet of AI coding agents in parallel — each in its own sandbox and git worktree.

Quorum is an open-source macOS app for running and managing multiple AI coding agents at once — Claude Code, Codex, Cursor, OpenCode, and more — against a single repository, without them stepping on each other.

[![Download for macOS](https://img.shields.io/badge/Download%20for%20macOS-Apple%20Silicon%20%26%20Intel-111?style=for-the-badge&logo=apple)](https://quorum.fwdai.org)

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
![Platform: macOS 13+](https://img.shields.io/badge/Platform-macOS%2013%2B-lightgrey.svg)
![Built with Tauri](https://img.shields.io/badge/Built%20with-Tauri%202-24C8DB.svg)

<!-- Drop a short demo GIF here. Record one full loop: spawn an agent → it works → review the diff → commit/PR. Save it to docs/demo.gif. -->
<img src="docs/demo.gif" alt="Quorum: spawning multiple AI coding agents in parallel, each in its own git worktree" width="820">

</div>

---

A *quorum* is the number of members a body needs present to do business. Convene one on your codebase: many coding agents, each isolated in its own worktree and sandbox, all working the same problem in parallel — with you presiding.

Every agent runs in its own git worktree under a macOS sandbox, so you can run a dozen at once and they physically can't touch each other's files — or write outside their box. Watch each one as a readable chat or its raw terminal, see its edits as live diffs the moment they land, and commit, push, or open a PR from the same window. Need more throughput? Spawn another. Done with one? Discard it — worktree and branch with it — in a click.

## Why Quorum

- **Real parallelism, no collisions.** Every agent gets its own git worktree and branch. Two agents editing the same file is impossible — they're working different checkouts of the same repo.
- **Isolated by default.** Each agent runs under a macOS `sandbox-exec` profile that denies writes outside its worktree (plus the agent's own cache/config). It's a write-protection boundary, not a full VM — honest about what it does.
- **Bring your own agent.** Quorum drives the CLIs you already use and pay for — Claude Code, Codex, Cursor, OpenCode, Pi — through one interface. No new model subscription, no lock-in.
- **One cockpit.** A normalized chat view of every agent's reasoning and tool calls, a native terminal for its raw TUI, a live feed of its diffs as it edits, an integrated Git/PR panel, plus optional Run and Terminal panels — all per agent.
- **Start projects, not just sessions.** Clone a repo from GitHub or create a new one (local + published) without dropping to a shell.
- **Local and private.** A native desktop app. Your code and agent transcripts stay on your machine; nothing routes through a Quorum server.

## Supported agents

Quorum normalizes each agent's transcript into one consistent view, so they all look and behave the same to you.

| Agent | Status |
| --- | --- |
| [Claude Code](https://docs.anthropic.com/en/docs/claude-code) | ✅ Supported |
| [Codex](https://github.com/openai/codex) | ✅ Supported |
| [Cursor Agent](https://cursor.com) | ✅ Supported |
| [OpenCode](https://opencode.ai) | ✅ Supported |
| Pi | ✅ Supported (experimental) |
| Antigravity | 🔜 Coming soon |

You install and authenticate the agent CLIs you want to use; Quorum detects them on your `PATH` and common install locations.

## Download

**[Download Quorum for macOS →](https://quorum.fwdai.org)** (universal — Apple Silicon & Intel)

The app is signed, notarized, and updates itself in the background.

**Requirements**
- macOS 13+ (`sandbox-exec` is macOS-only)
- At least one supported agent CLI installed (e.g. `claude`)
- `git`, and `gh` (GitHub CLI) for the PR and project-creation features

No VM, no Docker, no containers.

## How it works

1. **Point** Quorum at a local git repo.
2. **Spawn** an agent with a task. Quorum creates an isolated worktree at `~/.quorum/worktrees/<id>/` on a fresh branch.
3. **Run.** It launches the agent's CLI in that worktree under `sandbox-exec`, bridged through a local PTY.
4. **Watch.** Output streams into a tab — switch between a normalized chat view and the raw native terminal.
5. **Review.** See the agent's edits as live diffs; browse and edit files directly.
6. **Ship.** Commit, push, open or merge a PR from the Git panel. Or discard the agent — and its worktree and branch — in one click.

Agent history is captured to a local SQLite store, so reopening a session replays the full transcript.

## Isolation & security

Each agent runs as **your user** under a per-agent `sandbox-exec` profile that denies writes by default and re-allows them only for:

- the agent's worktree root under `~/.quorum/worktrees/<id>/`
- `/private/tmp`, `/private/var/tmp`, and `/private/var/folders`
- the agent's own state: `~/.claude`, `~/.claude.json`, `~/.npm`, `~/.cache`, `~/.config`, and `~/.local`
- the PTY and device files terminal programs need (`/dev/tty*`, `/dev/ptmx`, `/dev/pts/*`, `/dev/null`, `/dev/zero`)

This list is the complete write-allow set; everything else is denied. See [`src-tauri/src/sandbox.rs`](src-tauri/src/sandbox.rs) for the exact profile.

This is a **write-protection sandbox, not a VM**: the agent can still read files your user can read and use the network. It's the difference between "can't trash the rest of your repo or machine" and "fully air-gapped." Choose tasks accordingly.

## Build from source

```bash
bun install
bun tauri dev
```

**Toolchain:** [Bun](https://bun.com) 1.3+ and a stable Rust toolchain. The frontend is React 18 + TypeScript + Zustand + xterm.js; the backend is Rust via [Tauri 2](https://tauri.app).

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

Issues and pull requests are welcome. If you're planning a larger change, open an issue first so we can align on direction. Please keep PRs focused and run both test suites before submitting.

## License

[AGPL-3.0](LICENSE). See [NOTICE](NOTICE) for attribution.
