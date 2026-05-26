# Quorum

> Run multiple [Claude Code](https://docs.anthropic.com/en/docs/claude-code) sessions against one repo, each in its own git worktree, sandbox, and terminal tab.

**Status:** v1 / pre-release. macOS only.

## What it does

- Point it at a git repo.
- Click "+ Spawn", give the agent a task in plain English.
- It creates `.worktrees/<id>` on a fresh branch.
- It launches `claude --dangerously-skip-permissions --permission-mode bypassPermissions` in that worktree under `sandbox-exec`.
- The sandbox allows reads and network, but limits writes to the agent worktree plus standard Claude/cache/temp locations.
- It streams the process through a local PTY into an xterm.js tab.
- You can switch between running sessions, type into any terminal, stop a process, or remove its worktree and branch.

## Architecture (one paragraph)

A Tauri 2 app. Frontend is React 18 + TypeScript + Zustand + xterm.js. The Rust backend owns a `Supervisor` that creates a git worktree, returns the agent record so the frontend can mount the terminal, then starts `sandbox-exec -f <profile> claude ...` in a local PTY and bridges PTY output to the frontend over Tauri events. Workspace state persists as JSON in `~/Library/Application Support/com.quorum.desktop/`.

## Requirements

- macOS 13+ (`sandbox-exec` is macOS-only)
- Claude Code installed. The app looks for `claude` on `PATH`, via a login shell, and common install locations such as `~/.local/bin/claude`, `/opt/homebrew/bin/claude`, and `/usr/local/bin/claude`.
- [Bun](https://bun.com/) 1.3+
- Rust (stable)

That's it. No VM, no Docker, no Tart, no install dance.

## First-time setup

```bash
bun install
bun tauri dev
```

## Project layout

```
src/                       # React 18 + TS frontend
  main.tsx                 # entry
  App.tsx                  # shell layout
  api.ts                   # typed Tauri IPC + event wrappers
  store.ts                 # Zustand store
  components/              # WorkspaceBar, ChooseRepoDialog, AgentList,
                           # AgentPanes, AgentTerminal, SpawnDialog
src-tauri/
  src/lib.rs               # Tauri setup + IPC registration
  src/pty_session.rs       # local PTY around the Claude process
  src/agent.rs             # per-agent lifecycle
  src/sandbox.rs           # per-agent sandbox-exec profile
  src/git.rs               # git worktree add/remove/prune + branch -D
  src/workspace.rs         # workspace JSON state + agent records
  src/supervisor.rs        # registry + Tauri command coordination
  src/commands.rs          # #[tauri::command] handlers (thin)
```

## Isolation

Each agent gets its own git worktree and branch. The process also runs under a macOS `sandbox-exec` profile that denies writes by default and re-allows writes to:

- the agent worktree under `.worktrees/<id>`
- `/private/tmp`, `/private/var/tmp`, and `/private/var/folders`
- `~/.claude`, `~/.claude.json`, `~/.npm`, `~/.cache`, `~/.config`, and `~/.local`
- PTY/basic device files required by terminal programs

This is a write-protection sandbox, not a full VM. Claude still runs as your user, can read files your user can read, and can use the network.

## Notes

- Claude is started after the frontend has selected the agent, so xterm is mounted before Claude performs terminal startup negotiation.
- Terminal output is buffered in the frontend so switching between agent tabs can replay recent output.
- Removing an agent stops the process, removes its `.worktrees/<id>` directory, deletes its branch, and drops the app record.

## License

TBD.
