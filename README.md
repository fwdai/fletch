# algiers

> Spawn multiple [Claude Code](https://docs.anthropic.com/en/docs/claude-code) agents in parallel on a single repo, each in its own git worktree and a kernel-enforced macOS sandbox. Yolo mode (`claude --dangerously-skip-permissions`) is safe because each agent can only modify its own worktree.

**Status:** v1 / pre-release. macOS only.

## What it does

- Point it at a git repo.
- Click "+ Spawn", give the agent a task in plain English.
- It creates `.worktrees/<id>` on a fresh branch.
- It launches `claude` for that worktree inside `sandbox-exec` with a profile that allows writes **only** to that one worktree (plus standard temp dirs and claude's own state). No matter what the agent does — `rm -rf /`, edits to `~/.zshrc`, writes to other agents' worktrees — the kernel refuses.
- The agent's terminal streams to a tab in the app via a local PTY.
- N agents in parallel on the same repo, fully isolated from each other.

## Architecture (one paragraph)

A Tauri 2 app. Frontend is React 18 + TypeScript + Zustand + xterm.js. The Rust backend owns a `Supervisor` that creates a `git worktree`, writes a per-agent `sandbox-exec` profile, spawns `claude --dangerously-skip-permissions` under that profile inside a local PTY, and bridges the PTY to the frontend over Tauri events. Workspace state persists as JSON in `~/Library/Application Support/com.algiers.app/`.

## Requirements

- macOS 13+ (sandbox-exec ships with every macOS)
- `claude` on your PATH (`npm install -g @anthropic-ai/claude-code` or however you have it)
- Node 20+, npm
- Rust (stable)

That's it. No VM, no Docker, no Tart, no install dance.

## First-time setup

```bash
npm install
npm run tauri dev
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
  src/sandbox.rs           # builds the SBPL profile per agent
  src/pty_session.rs       # local PTY around the sandboxed claude process
  src/agent.rs             # per-agent lifecycle
  src/git.rs               # git worktree add/remove/prune + branch -D
  src/workspace.rs         # workspace JSON state + agent records
  src/supervisor.rs        # registry + Tauri command coordination
  src/commands.rs          # #[tauri::command] handlers (thin)
```

## Threat model

`sandbox-exec` is what Safari/Mail/Chrome use internally. The profile we generate per agent:

- **Allows reads** of the user's filesystem (claude needs to read its auth, system libs, etc.)
- **Denies writes** by default
- **Re-allows writes** narrowly to: the agent's `.worktrees/<id>/`, `/private/tmp`, `/private/var/folders`, `~/.claude`, `~/.npm`, `~/.cache`, `~/.config`, `~/.local`, and PTY devices.
- **Allows network** (claude reaches Anthropic + may run `git`/`npm` for the agent's task)

What this protects against:

- Agent runs `rm -rf /` → blocked at kernel.
- Agent writes to `~/.zshrc`, `~/.ssh/known_hosts`, `/etc/passwd` → blocked.
- Agent writes to another agent's worktree → blocked.

What it doesn't protect against:

- Reads. The agent can read everything you can. Don't put secrets in a path the agent can find if you're not OK with that.
- Network exfil. Allow-network is wide open. We could restrict to specific hosts if needed.

If you need stronger isolation (e.g. full VM-grade), the architecture is small enough to swap the sandbox layer for containers/VMs later.

## License

TBD.
