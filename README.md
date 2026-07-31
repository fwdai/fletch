<div align="center">

<h1>
  <img src="docs/fletch_dark.png" alt="" width="48" valign="middle">
  Fletch
</h1>

### A new kind of IDE for agentic engineering.

Run Claude Code, Codex, Cursor, OpenCode and more in sealed workspaces, each with its own repo clone and a served index of your codebase, chained into deterministic workflows that plan, build, review, and test.

[![Download for macOS](https://img.shields.io/badge/Download%20for%20macOS-Apple%20Silicon%20%26%20Intel-111?style=for-the-badge&logo=apple)](https://fletch.sh)

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
![Platform: macOS 13+](https://img.shields.io/badge/Platform-macOS%2013%2B-lightgrey.svg)
![Built with Tauri](https://img.shields.io/badge/Built%20with-Tauri%202-24C8DB.svg)

<img src="docs/fletch-demo.gif" alt="Fletch running seven agents in parallel. One agent's task is followed from its tool calls through tests passing to a merged pull request." width="900">

<sub>[Watch the full walkthrough →](https://fletch.sh/fletch-demo.mp4)</sub>

</div>

<!-- The GIF is a self-contained 26s loop (1280x800, 5.3 MB): seven agents in the
     sidebar, one agent's task from tool calls to tests passing, ending on the
     merged PR. Source is quorum-demo/fletch-command-palette.gif; regenerate there,
     not here. The 42s film behind the walkthrough link is a different cut.
     A GIF autoplays and renders everywhere (mirrors, forks, VS Code preview),
     which the previous GitHub-hosted mp4 player did not — it needed a click and
     only worked on github.com. To restore that embed instead, drop this URL back
     on its own line outside the <div> above (it can't be regenerated from the
     CLI — GitHub only mints these on web-UI upload):
     https://github.com/user-attachments/assets/a8fb53d6-d893-4382-894a-fb173cd29f10 -->

---

Fletch is a native macOS app for engineers who use AI agents to build real software and want predictable results, not just more output. Running more agents stopped being the hard part a while ago. Trusting what they produce is the hard part now.

Fletch is opinionated about how agent-written code should reach your main branch:

- **Agents don't get your checkout.** Each one works in an isolated clone of your repo, inside an OS-level sandbox that denies writes anywhere else. Your working copy is untouchable.
- **Process beats prompting.** Repeatable work runs as a workflow (plan → build → review → test) where a step is done when a _verifiable condition_ holds: tests passed, a commit landed, you approved. Not when the agent says so.
- **Nothing merges without you.** Live diffs, explicit approval gates, and your review sit between the agents and the merge.

If you want to fire off a dozen agents and merge whatever comes back, there are simpler tools. Fletch is for the part after the demo: shipping agent-written code you're willing to maintain.

## What you get

**Real isolation.** Every agent gets its own full clone of your repo (shared git objects make spawning effectively free), sandboxed by macOS Seatbelt by default or by an opt-in Docker container per agent. Either way, an agent can't touch your repo, your machine, or another agent's work.

**Agents that know your codebase.** Isolation has a cost nobody mentions: an agent alone in a fresh clone has to rediscover your project by grepping. So Fletch indexes your repo's symbols and call graph and serves it to every agent as an MCP server — an agent asks where a symbol is defined and what calls it instead of reading files until it finds out. The index is built in a throwaway mirror clone and copied into each workspace, so nothing is ever written to your repo, and a file watcher keeps it current as the agent edits. Built on [codegraph](https://github.com/colbymchenry/codegraph), which Fletch vendors and drives rather than reimplements. Best-effort by design: if indexing fails, the agent falls back to searching files and the run continues. On by default, with a toggle in Settings.

**Deterministic workflows.** Define a pipeline once (architect, coder, reviewer, tester) and launch it on any task. Steps hand context forward, loops repeat _review → fix_ until a verdict says done, parallel blocks fan work out and join it back. Control flow belongs to the engine, not to an LLM: gates are checked, budgets (turns, iterations, wall-clock, tokens) are enforced, every pause names its cause, and runs survive an app restart. Workflows are shareable YAML.

**Control.** Watch every agent as it works: a normalized chat of reasoning and tool calls (every harness looks the same), the raw terminal when you want it, live diffs as edits land. And sign off on what ships: approval gates in workflows, in-app file editing, and a full git/PR panel. Commit, push, open and merge PRs, and pull unresolved review comments straight into an agent's chat to fix. The reviewer in the loop is you.

**Specialist agents.** Base harnesses are generalists. On top of them you define custom agents (a role, a model, a reasoning budget, standing instructions, plus skills and MCP tools) and assign the right one to each workflow step: an _Architect_ pinned to your strongest model, a _Reviewer_ that refuses to pass unhandled errors.

**Parallel by default.** Kick off as many agents as you like on one repo; isolation makes collisions structurally impossible. In this category that's the ante, not the pitch.

**Local-first, your subscriptions.** Fletch drives the CLIs you already installed and pay for: no middleman keys, no markup, no proxy. Your code and transcripts stay on your machine; nothing routes through a Fletch server. Free and AGPL.

## Quick start

1. **[Download Fletch](https://fletch.sh)**: universal binary, signed, notarized, self-updating.
2. Onboarding checks the three things you need — `git`, GitHub access, and at least one agent CLI — and can set each up in place: install git or an agent CLI for you, and connect GitHub with a device code (no GitHub CLI involved).
3. Point Fletch at a repo and spawn an agent with a task, or define a workflow and launch it.

Requires macOS 13+. No VM, no required containers: Docker is an optional isolation engine, not a prerequisite.

## Supported harnesses

| Harness                                                           | Status                      |
| ----------------------------------------------------------------- | --------------------------- |
| [Claude Code](https://docs.anthropic.com/en/docs/claude-code)     | ✅ Supported                |
| [Codex](https://github.com/openai/codex)                          | ✅ Supported                |
| [Cursor Agent](https://cursor.com)                                | ✅ Supported                |
| [OpenCode](https://opencode.ai)                                   | ✅ Supported                |
| [Pi](https://pi.dev/)                                             | ✅ Supported                |
| [Antigravity](https://antigravity.google/product/antigravity-cli) | ✅ Supported (experimental) |

Fletch detects installed CLIs automatically and normalizes their transcripts into one consistent view.

## Sandboxing

Fletch ships two isolation engines. The guarantee is the same under both: an agent's writes stay inside its own workspace. The trade-off is friction versus depth.

- **Seatbelt (default).** Each agent runs under a per-agent macOS `sandbox-exec` profile that denies writes outside its workspace. Nothing to install, nothing to configure, no startup cost. But it's write protection, not a VM: the agent runs as your user, reads what you can read, and has network access.
- **Docker (opt-in).** Switch the engine and each agent runs in its own container. Truer sandboxing: the agent sees only what's mounted into its container, so the rest of your filesystem isn't reachable even for reads. It requires Docker on your machine (and every harness except Antigravity supports it), and it's still not a silver bullet: the network is open, and a container is not a VM.

Under both engines, workspaces are full shared-object clones, so an agent's writable `.git` lives entirely inside its sandbox; your repo's `.git` is never writable by an agent. Pushes and PR creation run host-side through a brokered RPC, so credentials never enter the sandbox, and each agent's capability grant constrains where it may publish: no agent can push the branch its work is reviewed against (nor `main`/`master`), and a workflow step agent can't publish at all. For its own branch, though, an agent still pushes and opens a PR without a confirmation prompt, under your identity. Choose tasks and repos accordingly.

The full write-up is [`docs/isolation.md`](docs/isolation.md) — the canonical, code-accurate account of both engines, the clone model, and the honest threat model. The code is canonical too: [`sandbox/seatbelt.rs`](src-tauri/src/sandbox/seatbelt.rs), [`sandbox/docker/`](src-tauri/src/sandbox/docker), and [`sandbox/provision.rs`](src-tauri/src/sandbox/provision.rs).

## Documentation

Concepts, the workflow reference, and the isolation design live at **[fletch.sh/docs](https://fletch.sh/docs)**.

## Community

Questions, workflows worth stealing, and anything that isn't a bug go in **[Discussions](https://github.com/fwdai/fletch/discussions)**. Two threads to start with:

- **[Known gaps in the beta](https://github.com/fwdai/fletch/discussions/523)** — what doesn't work yet, written down rather than discovered. Read this before you file anything.
- **[What would you fix first](https://github.com/fwdai/fletch/discussions/526)** — the one that actually shapes what gets built next.

Reproducible bugs belong in [Issues](https://github.com/fwdai/fletch/issues). Common questions are answered on the [FAQ](https://fletch.sh/faq).

## Build from source

```bash
bun install
bun tauri dev
```

**Toolchain:** [Bun](https://bun.com) 1.3+ and a stable Rust toolchain. React 18 + TypeScript + Zustand + xterm.js frontend; Rust backend via [Tauri 2](https://tauri.app).

```
src/                  frontend (store, adapters, workflows, components)
src-tauri/src/        backend (supervisor, sessions, sandbox, workflow engine, git, github)
src-tauri/migrations/ SQLite schema
```

```bash
bun run test                  # frontend (vitest)
cd src-tauri && cargo test    # backend
```

## Contributing

Issues and pull requests welcome. Planning a larger change? Open an issue first so we can align on direction. Keep PRs focused and run both test suites before submitting.

## License

[AGPL-3.0](LICENSE). See [NOTICE](NOTICE) for attribution.
