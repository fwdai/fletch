# Fletch — Feature Map

**Purpose.** This file is a complete, structured inventory of every user-facing feature and
capability in Fletch (the macOS desktop app in this repo; the repo/package is still named
`quorum`). It exists to drive user-facing documentation on the marketing/docs website
(`fletch.sh`). Each entry is written as documentation-ready prose — what the feature does, how
a user encounters and uses it, and its key behaviors and limits — followed by a `Docs status`
line (coverage on the existing website) and a `Source:` line (file paths for a doc writer to
verify details). Building-block details are folded into their parent feature per the granularity
rule; only things a user would name as a feature stand alone.

Paths in `Source:` lines are relative to the app repo root (`/Users/alex/Code/quorum`) unless
noted. Existing docs pages referenced under `Docs status` live at
`/Users/alex/Code/fletch.sh/src/content/docs/docs/`.

> **Update (2026-07-17):** the website docs were revised against this map. The two systemic
> problems below are now fixed site-wide (shared-clone/Docker framing corrected; GitHub pages
> rewritten to the native OAuth + REST/GraphQL path, verified in `src-tauri/src/github/client.rs`
> and `oauth.rs` — "replacing the `gh` CLI dependency" is stated in the code itself). The app
> README was rewritten upstream the same week and polished here; it no longer claims a `gh`
> requirement or worktrees.
>
> **Update 2 (2026-07-17, post-pull):** the map was refreshed against 55 new upstream commits
> (v0.7.10): reworked slash commands (local commands, disk discovery, plugins, skills-as-commands),
> per-project environment-variable sharing into runs, project deletion, per-model reasoning levels,
> draft-composer mentions, workflow launch attachments, the merge fallback, and the Density-setting
> removal. Entries marked *(new/reworked in 0.7.10)* below; the affected docs pages were updated.
>
> Original findings, kept for the record:
> 1. The docs said each agent gets a **git worktree** and that Fletch uses "No VM, no Docker, no
>    containers." In the current code the default provisioning is a **shared git clone**, worktrees
>    are a hidden dev-only mode, and **Docker is a fully supported, selectable sandbox engine.**
> 2. The docs described GitHub features as running on the **`gh` CLI**; the backend uses native
>    GitHub REST/GraphQL calls authenticated with Fletch's own OAuth device-flow token.
> Also: the workspace on-disk path is `~/.fletch/workspaces/<id>/` (docs said `~/.fletch/worktrees/<name>/`).

---

## 1. Isolation & Safety

### Isolated workspaces (shared clones + branches)
When you spawn an agent, Fletch gives it a private, disposable checkout of your repository so it
can edit, branch, and commit without ever touching your real working tree. Each workspace lives
under `~/.fletch/workspaces/<id>/`, with the repo checked out in a subdirectory. The checkout is a
`git clone --shared`: it borrows your repo's object store (via git "alternates") instead of copying
history, so spawning costs kilobytes and milliseconds regardless of repo size, and the agent's
entire writable `.git` lives inside its own sandbox — your repo's `.git` is never writable by an
agent. Checkouts start at a captured fork-point commit (a detached, immutable SHA taken after a
best-effort fetch of the base branch), so diffs are always measured against the exact starting
point. A branch is created only on the first push, and the agent names it itself (`fix/…`, `feat/…`,
`chore/…`). Because every workspace is a different checkout, two agents can never edit the same file.
The one trade-off of borrowing objects: a workspace is tied to its source repo staying in place.
(A linked-worktree mode exists but is a hidden developer-only flag, not exposed in the UI, and is
unsafe under Docker.)
- **Docs status:** `documented` — `concepts/isolated-workspaces.md`, `concepts/sandboxing.md`, and `getting-started/first-agent.md` (renamed from first-worktree, 301 redirect in place) now describe the shared-clone design and the `~/.fletch/workspaces/<id>/` path.
- **Source:** `src-tauri/src/sandbox/provision.rs`, `src-tauri/src/workspace.rs`, `src-tauri/src/supervisor/lifecycle.rs`, `src-tauri/src/git.rs`, `src-tauri/migrations/0001_initial_schema.sql`, `0008_worktree_base_sha.sql`

### macOS sandbox (Seatbelt / sandbox-exec)
Every agent process runs under a per-agent macOS `sandbox-exec` profile that denies file writes by
default and re-allows only what the agent legitimately needs: its own workspace root and checkouts,
its RPC mailbox (`~/.fletch/rpc/<id>/`), a workflow blackboard directory when one applies, temp
directories, the PTY/device files terminal programs need, and a curated allowlist of per-agent
config/cache directories (`~/.codex`, `~/.cursor`, `~/.gemini`, `~/.pi`, `~/.npm`, `~/.cache`,
`~/Library/Caches`, `~/Library/Application Support`, and so on). It deliberately does **not** grant
`~/.local/bin` (a PATH-hijack surface) or all of `~/.claude` (whose `settings.json` can define
host-executed hooks) — only safe write "islands" plus the credentials file. Fletch's own app-data
directory (holding the SQLite database with your transcripts) is denied both read and write. This is
a **write-protection boundary, not a VM**: an agent can still read anything your user can read and
reach the network — it's "can't trash your repo or machine," not "air-gapped."
- **Docs status:** `documented` — `concepts/sandboxing.md` (dedicated page: profile scope, deny-by-default, comparison with Docker), plus `concepts/isolated-workspaces.md`, `getting-started/first-agent.md`, `reference/faq.md`.
- **Source:** `src-tauri/src/sandbox/seatbelt.rs`, `src-tauri/src/sandbox/policy.rs`, `src-tauri/src/sandbox/mod.rs`, `README.md` (Isolation & security)

### Docker sandbox engine (optional)
Instead of Seatbelt, you can run agents inside Docker containers — a stronger isolation boundary
where builds and tests execute on Linux. You choose the engine in **Settings › General › Sandbox**;
the choice is stamped onto each agent when it first spawns, so switching engines never re-engines an
existing agent. Each agent process runs as its own `docker run --rm --init` container. Fletch
live-probes Docker availability and disables the option (with a "Start / Install Docker Desktop"
hint, and a one-click "Start Docker Desktop") when the daemon is down; a Docker-stamped agent whose
daemon is unavailable fails loudly rather than silently downgrading to Seatbelt. Docker is supported
for Claude Code, Codex, Cursor, OpenCode, and Pi — **not** Antigravity (its CLI has no
non-interactive login). The container mounts the writable workspace, the RPC mailbox, and `~/.claude`
(read-only except the credentials file); the source object store is mounted read-only and the real
`.git` never enters the container. Resource limits default to 4 GB memory and 2 CPUs and, along with
the container image, are configurable in the Experimental settings pane. The first image build shows
a progress toast; orphaned containers and stale images from dead app instances are swept on startup.
Threat model: live-process containment (containers run as root in this version), not a hardened trust
boundary — the clone + PR-review flow remains the real gate.
- **Docs status:** `documented` — `concepts/sandboxing.md` (engine choice, probe/start behavior, resource defaults, provider coverage, threat model), `reference/settings-and-storage.md`, `reference/faq.md` ("Do I need Docker?"); the "no Docker" claims are removed site-wide.
- **Source:** `src-tauri/src/sandbox/docker/{mod,engine,image,cleanup,cli,probe,progress}.rs`, `src/store/sandbox.ts`, `src/components/SettingsScreen/{GeneralPane,ExperimentalPane}.tsx`, `src/components/DockerBuildToast.tsx`, `src-tauri/migrations/0013_workspace_sandbox_engine.sql`

### Container authentication for Claude (Docker mode)
Because containers have no access to your macOS Keychain and Fletch injects no credentials by
default, running Claude Code in a Docker sandbox needs its own auth. Fletch resolves it per spawn
through a first-hit-wins chain (Keychain OAuth token → a stored `claude setup-token` → ambient
`ANTHROPIC_API_KEY`/OAuth env → a usable credentials file on the read-only mount → otherwise fail
with a "Connect Claude for containers" prompt). The **Connect Claude** flow in
**Settings › General › Sandbox** automates `claude setup-token` behind the scenes: it surfaces the
consent URL, relays the code prompt, and captures the token, with a manual token-paste fallback. It
is Docker-only — Seatbelt agents keep your normal CLI login.
- **Docs status:** `documented` — `concepts/sandboxing.md` (provider coverage + Connect Claude flow) and `reference/settings-and-storage.md` (settings location).
- **Source:** `src-tauri/src/sandbox/docker/{auth,setup_token}.rs`, `src/components/SettingsScreen/ContainerAuth.tsx`, `src/util/useClaudeSetup.ts`

---

## 2. Agents & Providers

### Multi-agent support & transcript normalization
Fletch doesn't ship a model — it drives the agent CLIs you already install and pay for, behind one
consistent interface. Supported agents: **Claude Code** (default), **Codex**, **Cursor Agent**, and
**OpenCode**, with **Pi** and **Antigravity** as experimental options. You pick the agent (and, where
the CLI allows, the model and reasoning level) when you spawn a workspace, and it stays fixed for that
workspace's life. Regardless of which agent runs, Fletch normalizes its live output *and* its on-disk
transcript into one common event model, so every agent renders the same way in the chat view —
messages, reasoning, tool calls (with nested subagent threads), and results — and re-opening a session
replays its full history. Token/cost accounting is normalized per provider (some report cumulative
totals, some deltas, some none). Antigravity is fixed-model and reports no usage.
- **Docs status:** `documented` — `concepts/agents.md`, `reference/faq.md`, `getting-started/installation.md`, and `index.md` list the agents and the "your keys, one view" model accurately.
- **Source:** `src/data/providers.ts`, `src/adapters/index.ts`, `src/adapters/<agent>/{normalize,reduce,policy,usage}.ts`, `src/adapters/types.ts`, `src-tauri/src/instructions.rs`

### CLI auto-detection & binary override
Fletch finds each agent CLI automatically, even when it's installed somewhere your GUI shell's `PATH`
doesn't include — it searches the process `PATH`, your login-shell `PATH` (picking up nvm/fnm/volta/
Homebrew), and common install locations (`~/.local/bin`, `~/.npm-global/bin`, `~/.bun/bin`,
`/opt/homebrew/bin`, `/usr/local/bin`). In **Settings › Providers** you can enable/disable each agent,
see its detected version and resolved binary path, **Re-scan system**, and set an explicit
**binary path override** for a CLI in an unusual location (a broken override is flagged rather than
silently ignored). The same resolver locates `git` and (historically) `gh`.
- **Docs status:** `documented` — `getting-started/installation.md` and `reference/faq.md` cover discovery, re-scan, and the override.
- **Source:** `src-tauri/src/bin_resolve.rs`, `src/components/SettingsScreen/{ProvidersPane,BinaryPathRow}.tsx`

### One-click agent install
For agents you don't have yet, Fletch can install them for you with the vendor's official installer,
streaming progress live and reporting success or failure. This is offered from the provider readiness
UI (onboarding and Settings › Providers). Claude and Codex are scriptable on Windows and Unix; Cursor
and OpenCode on Unix; Antigravity and Pi have no scripted installer and fall back to a docs link.
- **Docs status:** `documented` — `getting-started/installation.md` (one-click Install with the scripted-installer coverage list) and `reference/faq.md`.
- **Source:** `src-tauri/src/agent_install.rs`, `src/components/ProviderReadiness/index.tsx`

### Model catalog & selection
The composer's model picker (and the custom-agent editor) let you choose a model for agents whose
CLI supports it. The list is built at two levels: Fletch queries each installed CLI for the models it
offers, then enriches those IDs against the online **models.dev** catalog for context-window size and
reasoning support, caching the result for an hour and rebuilding in the background — so newly released
models appear without an app update, and a models.dev outage still yields a usable list (0.7.10:
refreshes dedupe and preserve the cache on partial failure). Reasoning / "thinking" levels are now
**dynamic per model** where the CLI reports them (e.g. Codex models exposing tiers above high), with
provider defaults otherwise; the level is fixed per session at spawn for providers like Claude.
Antigravity is fixed-model. A manual "Refresh models" action exists in the dev-only Developer pane.
- **Docs status:** `documented` — `concepts/agents.md` covers the live, self-updating model list and per-model effort levels.
- **Source:** `src-tauri/src/model_catalog.rs`, `src/data/modelCatalog/index.ts`, `src/data/providerDetail.ts`, `src/components/Composer/ModelPicker.tsx`

### Custom agents (presets)
A custom agent is a reusable preset that instances a built-in provider ("base agent") with a saved
configuration: a name and color, a description/role tagline, a chosen model and reasoning budget,
standing instructions (a system prompt), and any **Skills** and **MCP tools** it should have. You
create and edit them in **Settings › Custom agents**; they appear in the composer's model picker as
one-click options alongside the built-in agents. The editor adapts to the base (e.g. it disables the
model dropdown for fixed-model bases, and filters MCP tools to those the base can actually deliver).
Everything is snapshotted onto a session at spawn, so a running or resumed agent keeps exactly the
configuration it started with even if you later edit the preset.
- **Docs status:** `documented` — `concepts/agents.md`, `guides/tips.md`, and `reference/settings-and-storage.md`; the Skills/MCP attachments are covered there and in `reference/skills-and-tools.md`.
- **Source:** `src/components/SettingsScreen/CustomAgents/`, `src/storage/customAgents.ts`, `src-tauri/src/agent_profile.rs`

### Skills
Skills are named Markdown instruction documents that custom agents load on demand. Each has a name, a
one-line description (all the agent sees until it decides to open the document), and a Markdown body.
You manage them in **Settings › Skills** and attach them to agents in the agent editor. They work on
every base agent: at spawn, each attached skill is written into the sandbox and a compact index (name,
description, path) is appended to the agent's instructions, so the per-turn cost is one line per skill
until the agent chooses to read one.
- **Docs status:** `documented` — `reference/skills-and-tools.md`.
- **Source:** `src/components/SettingsScreen/Skills/`, `src/storage/skills.ts`, `src-tauri/src/agent_profile.rs` (materialize_skills)

### MCP servers (Tools)
Fletch lets you register Model Context Protocol tool servers and attach them to custom agents. In
**Settings › Tools** you add a server with a transport — a stdio command (with environment variables)
or an HTTP URL (with headers). Servers form a shared library; custom agents reference them by ID, and
the exact configuration is snapshotted onto a session at spawn. Delivery depends on the base agent:
Claude supports both stdio and HTTP servers, Codex supports stdio only, and other agents don't accept
MCP servers (the editor says so).
- **Docs status:** `documented` — `reference/skills-and-tools.md`.
- **Source:** `src/components/SettingsScreen/McpServers/`, `src/storage/mcpServers.ts`, `src/data/providers.ts` (MCP_SUPPORT), `src-tauri/src/agent_profile.rs`

---

## 3. Fleet Management & the Cockpit

### Parallel spawning & fleet management
Fletch is built to run many agents at once. You spawn an agent with a first task; it gets its own
workspace and runs immediately. Because workspaces can't collide, you can split one big job across
several agents, race the same task on two or three agents/models and keep the best result, or simply
keep work flowing — review one agent while another thinks and a third ships. Managing the fleet is a
click each: **spawn** (⌘N / "+ New agent"), **stop** a running agent, **archive** it (hides it but
keeps a restorable snapshot of its branch and diff), **restore** it, or **discard** it (permanently
deletes the workspace and its branch). Run-owned workflow agents clean up the same way. The app icon
badges the count of agents that finished while you weren't looking.
- **Docs status:** `documented` — `concepts/parallel-agents.md`, `getting-started/first-agent.md`, `guides/tips.md`, and `concepts/isolated-workspaces.md` cover parallelism, spawn, stop, archive vs discard.
- **Source:** `src-tauri/src/commands.rs` (spawn_agent, stop_agent, discard_agent, archive_agent, restore_agent), `src-tauri/src/supervisor/`, `src/store/workspace.ts`, `src/components/Sidebar/`

### Sidebar (projects, agents, search)
The left sidebar is the fleet's command center. Projects group your agents, drafts, and workflow runs;
each project header shows counts, a gear for project settings, a "+" to spawn (⌘N), and can be
collapsed and drag-reordered. A search box (focus with ⌘K) filters agents by name/task/branch, drafts
by name, and projects by label. Each agent row shows its name (shimmering while it works), its
provider/custom-agent identity, live status cues (working, "waiting for your input," "new results to
review," error), a dev-server port chip when running, and a PR/diffstat sub-line, plus a hover stats
popover (runtime, context %, tokens, cost). Row actions are Stop (while active) and Archive (when idle).
Draft rows (not-yet-spawned agents) can be discarded. The footer shows your account and a theme toggle.
- **Docs status:** `partially documented` — sidebar search and ⌘N/⌘K appear in `reference/keyboard-shortcuts.md`; the row cues, reorder, per-agent stats popover, and dev-server chip are undocumented.
- **Source:** `src/components/Sidebar/{index,AgentRow,ProjectGroup,SidebarHeader,SidebarFooter,AgentStatsPopover}.tsx`, `src/components/Sidebar/useProjectReorder.ts`

### Draft workspace & first-task screen
Clicking "+ New agent" opens a draft: a "What should be the first task?" screen with the composer, a
project picker, a base-branch picker (defaults to `main`), a re-rollable generated workspace name, and
a preview of where the checkout will live. Sending the first message spawns the agent from the freshest
state of the base branch. When workflows are defined, an Agent/Workflow segmented toggle appears here so
you can launch a workflow instead (see Orchestration).
- **Docs status:** `documented` — `getting-started/first-agent.md` walks through the draft, task, model, base-branch, and reroll.
- **Source:** `src/components/Workspace/EmptyWorkspace.tsx`, `src/components/Composer/{ProjectPicker,BranchPicker}.tsx`, `src-tauri/src/names.rs`

### Normalized chat view
The default way to watch an agent: a readable, scrolling transcript of everything it does. It renders
your prompts (with attachments), the agent's streaming Markdown replies, its reasoning/"thinking",
system and error notices, and its **tool calls** as collapsible rows with purpose-built presenters
(shell/Bash, Read, Edit/MultiEdit, Write, Grep, Glob, subagent threads, task creation, and a default).
Running tools show a spinner and subagent step count; subagent conversations nest inside the call that
spawned them. When an agent asks you a question (a plan approval or a multiple-choice question), a
**question card** appears inline — single-select (answerable with number keys), multi-select, or a
free-text "Something else…" — and folds into a summary once answered. Each finished turn shows how long
it ran, with copy and fork actions. Auto-scroll pins to the bottom until you scroll up. Git actions the
agent performs on your behalf fold into quiet "git action" chips.
- **Docs status:** `documented` — `concepts/agents.md` and `getting-started/first-agent.md` describe the chat view as the normalized reasoning + tool-call transcript.
- **Source:** `src/components/Workspace/ChatView.tsx`, `src/components/Workspace/messages/` (MessageItem, ToolRow, presenters, UserInput/QuestionCard)

### Chat navigation & search
The transcript has a turn navigator docked top-right: ▲/▼ buttons (or Alt+↑ / Alt+↓) step one user
turn at a time, and clicking the "n / N" counter opens a clickable outline of every prompt — the
closest thing Fletch has to a quick-jump. Press ⌘F to open find-in-conversation, which highlights all
matches and the active match with next/previous wrap-around and scroll-into-view.
- **Docs status:** `documented` — the shortcuts are listed in `reference/keyboard-shortcuts.md` ("Find in conversation", "Previous / next user turn"); the outline jump-list could be called out more.
- **Source:** `src/components/Workspace/ChatNav/index.tsx`, `src/components/Workspace/ChatSearch/{index,useChatSearch}.ts`

### Native (raw terminal) view
An experimental per-agent toggle flips the cockpit from the normalized chat to the agent's own raw
terminal UI, streamed verbatim into an embedded terminal, with keystrokes (slash commands, arrows,
paste) going straight to the agent process. It becomes available after the agent's first turn and is
enabled globally via **Settings › Experimental › Native terminal view**.
- **Docs status:** `documented` — `concepts/agents.md`, `getting-started/first-agent.md`, `reference/settings-and-storage.md`, and `reference/terminal.md` describe it as the experimental raw-TUI view (distinct from the Terminal side panel).
- **Source:** `src/components/Workspace/{NativeView,ViewToggle}.tsx`, `src/util/useXterm.ts`, `src/pty/`

### Composer, follow-ups, and triggers
The message box is where you talk to an agent and reach for actions as you type. Enter sends
(Shift+Enter for a newline); draft text survives view switches. While an agent is working, the button
is a **Stop**; start typing and it becomes **Send** — a mid-turn follow-up is either injected into the
running turn live (Claude) or queued and delivered coalesced at the next turn boundary (the per-turn
agents), and the queue is persisted so it survives a crash. Three in-composer triggers: **`/`** opens
slash-command autocomplete, **`@`** attaches files (from the checkout, an absolute path, drag-and-drop,
or a file dialog), and **`#`** references an open pull request by number — as of 0.7.10 all three
triggers also work in the new-agent draft composer (one shared input core). Alongside the field are
optional chips: a **thinking-effort** cycle (levels are now dynamic per model — e.g. Codex models can
expose tiers above high — scoped and validated against the selected provider), an **auto-edit**
toggle, and a **context-usage** meter (a donut showing context-window use with a cost breakdown on
hover) — each shown or hidden in Settings.
- **Docs status:** `partially documented` — `reference/slash-commands.md` covers `/`, `@`, `#` (incl. the draft composer) and the chips with per-model effort levels; the Send/Stop toggle and mid-turn follow-up injection/queueing are undocumented.
- **Source:** `src/components/Composer/{index,ModelPicker,UsageMeter,AttachmentList}.tsx`, `src/components/Composer/autocomplete/`, `src/data/slashCommands/`, `src-tauri/src/message_queue.rs`

### Right side panels (Code / Git / Run / Terminal)
Each cockpit has a right rail with up to four tabbed panels, each toggled on in Settings (Git and Code
are on by default). Fletch remembers the last-used tab per agent. The Git tab shows a changed-file
count; the Run tab shows a pulsing "live" dot when the dev server is up.
- **Docs status:** `documented` — `getting-started/first-agent.md` and `guides/tips.md` explain enabling panels; individual panels have their own pages.
- **Source:** `src/components/RightPanel/index.tsx`

#### Code panel — file browser/editor (Explore) and live diff (Live)
The Code panel has two modes. **Explore** is a VS Code-style file tree (with search and a "Changed"
filter) and an editor: you can browse and edit any file in the checkout yourself, with syntax
highlighting, line numbers, a git-change gutter, autosave-after-pause (⌘S flushes now), a Revert to the
agent's version, a read-only unified-diff toggle, a syntax-theme selector, and a right-click context
menu (new file/folder, rename, duplicate, copy path, delete). **Live** is an activity feed of the
agent's edits rendered as unified diffs, auto-following the file the agent is currently editing
(Following/Paused), with per-file tabs and fresh-line highlighting. Both share one diff renderer with
syntax-highlighted add/remove/context lines and old/new line numbers.
- **Docs status:** `partially documented` — `guides/tips.md` names the Live and Explore views; the full file-editor/context-menu capabilities are undocumented and there is no dedicated Code-panel page.
- **Source:** `src/components/RightPanel/Code/{index,CodeLivePanel,DiffView}.tsx`, `src/components/RightPanel/FilePanel/`, `src/components/RightPanel/{FileIcon,FileContextMenu}.tsx`

#### Run panel — run your project in the workspace
Start your project's dev server, build, or script right inside an agent's checkout without leaving
Fletch. Press Start to launch the detected command (shown as `$ <command>`), watch output stream as a
live log, and Stop to end it. When the process serves on a port, a `:<port>` link opens it in your
browser. Each agent's Run panel operates on its own checkout, so you can run several versions
side-by-side (mind the ports). Run processes execute under a broader sandbox profile than agents (it
grants toolchain directories like `~/.cargo`, `~/.rustup`, `~/go`, and the full `~/.config`/`~/.local`
so real builds succeed) — a deliberately weaker boundary because it runs project code you chose to run.
- **Docs status:** `documented` — `guides/running-your-project.md` covers Start/Stop, the port link, and per-agent isolation.
- **Source:** `src/components/RightPanel/RunPanel.tsx`, `src-tauri/src/run_session.rs`, `src-tauri/src/sandbox/seatbelt.rs` (build_run_profile)

#### Terminal panel — a shell in the workspace
An interactive shell (xterm.js) with its working directory set to the agent's checkout — for one-off
commands and inspecting state. The session persists across tab switches so long-running commands keep
going, URLs in the output are clickable, the theme follows the app's light/dark setting, and ⌘F opens
an in-terminal find (Enter/Shift+Enter next/previous) that's intercepted before keystrokes reach the
shell. It's a plain shell for you, separate from the agent's native view.
- **Docs status:** `documented` — `reference/terminal.md` covers it thoroughly.
- **Source:** `src/components/RightPanel/TermPanel.tsx`, `src-tauri/src/exec_session.rs`

### Run configuration & auto-detection
Most projects run with no setup: Fletch auto-detects a sensible run configuration per ecosystem
(Node, Python, Ruby, Rust, Go) by reading files at the checkout root, grouped into Environment,
Scripts (version/install/dev/test/build), and Server (port/env), each with a confidence score. When a
detected value is wrong, open the gear on the Run panel to override any field, then Apply & restart,
Revert to detected (one field), or Reset all. Overrides are saved per project (in Fletch's local
database) so every agent for that repo inherits them, while individual agents can still override from
their own Run panel.
- **Docs status:** `documented` — `guides/run-configuration.md` and `guides/running-your-project.md` cover detection, overrides, and per-project storage.
- **Source:** `src-tauri/src/run_detect/`, `src/components/RunConfig/`, `src/components/RightPanel/RunSettingsSheet.tsx`

### Per-project environment variables (Run env sharing) *(new in 0.7.10)*
The sandbox deliberately withholds the project's `.env` from a run, so the **Environment variables**
section in project settings is the opt-in membrane: it lists the variables found in the source repo's
`.env`, each with a per-variable "share with sandbox" toggle (a cube that lights up when shared).
Nothing is shared by default. A shared variable's value is **mirrored live** from `.env` at spawn —
one source of truth, nothing to drift — or **overridden** by editing the value in place; overrides
support `{{agent_id}}` interpolation for per-agent values (e.g. a disposable per-agent database) and
clearing the field reverts to mirroring. Override values never touch the database: they live in the
macOS Keychain on release builds (in-memory on dev builds, where they don't survive a restart). The
resolved pairs are injected into the sandboxed Run-panel process; only the canonical `.env` file is
read for now.
- **Docs status:** `documented` — `guides/run-configuration.md` ("Environment variables") with a pointer note in `guides/running-your-project.md`.
- **Source:** `src-tauri/src/run_env.rs`, `src/components/ProjectSettings/EnvVarsSection/`, `src/storage/runEnv.ts`, `src-tauri/src/workspace.rs` (run_env)

### Open in editor / terminal
A titlebar launcher opens the active agent's checkout in your editor of choice. It only ever lists
tools actually installed on your machine (detected by both CLI and installed `.app` bundle), grouped
into Editors and Terminals — VS Code and variants, Cursor, Windsurf, Zed, Sublime, Nova, BBEdit,
TextMate, MacVim, Neovide, Xcode, JetBrains IDEs, and terminals like iTerm, Warp, Ghostty, WezTerm,
kitty (plus the system Terminal, always offered). Your last choice is remembered.
- **Docs status:** `documented` — `guides/tips.md` ("Drop into your own tools when it's faster").
- **Source:** `src-tauri/src/editors.rs`, `src/components/TitleBar/OpenInEditor/`

### Workspace status capsule (titlebar)
The center of the titlebar is a context-adaptive status capsule: for the active agent it shows a live
status dot, project/agent name, sandbox badge, and a git/CI badge, with a hover popover giving branch
vs base, ahead/behind, diffstat, PR and check breakdown, and quick actions that jump to the Git tab or
open the PR. At the home screen it's a quiet fleet summary ("N working / N waiting"); in Settings it's
a breadcrumb.
- **Docs status:** `undocumented` (minor).
- **Source:** `src/components/TitleBar/WorkspaceStatus/`

### Fork a workspace
Fork branches an existing agent into a brand-new workspace, letting you explore an alternative without
losing the original. You choose along two axes. **Code:** a *clean* worktree from the base branch, or
*with current code* — carrying the parent's current working tree including its **uncommitted work**, so
the fork builds directly on what the agent has done so far. **Context:** a *fresh chat*, the *full
history*, or history *up to a chosen message*. Carried context both copies the prior transcript into
the new chat (so the history renders) and appends a plain-text digest of that history to the new
agent's standing instructions (so the agent actually knows it, portably across providers). You trigger
it from a "Fork from here" button under any finished turn (forks up to that point) or from the fork menu
in the workspace header (full-history options). A fork is otherwise an ordinary agent — same sandbox,
model, skills, and tools as its parent. (Active area: "carry current code" is recently shipped; a
"summarized context" option is planned but not yet available.)
- **Docs status:** `documented` — `guides/forking.md` (both axes, all five menu entries, context digest, combination playbook); linked from tips, FAQ, and first-agent.
- **Source:** `src-tauri/src/supervisor/fork.rs`, `src/components/Workspace/{ForkButton,ForkMenu,WorkspaceHeader}.tsx`, `src/store/{workspace,forkDigest}.ts`

---

## 4. Git & Shipping

### Git panel (commit, push, PR, CI)
The Git tab is where you ship an agent's work. It shows the branch and its base, status pills
(uncommitted / pushed / conflicts / PR number / merged), the changed files, a commit-message composer,
and a state-aware split-button whose primary action adapts to the situation. By default commits are
**delegated to the agent** so it writes a sensible message: Commit, Commit & push, or Commit & open PR;
if you type your own message you get a Direct commit with your exact text. Pull-request actions: Open PR
(reusing an existing one for the branch), Merge PR (requests auto-merge so GitHub completes it once
required checks pass; falls back to a direct merge when the repo has auto-merge disabled or the PR is
already clean, but never bypasses a merge queue — 0.7.10), and View on GitHub. When a branch has a PR
the panel shows its CI check rollup (pending /
passing / failing) with per-check detail and the real merge-gate state (clean, blocked, behind, dirty,
draft), plus **Fix checks with agent** to hand a failure back to the agent. Review comments are shown
(bot comments flagged) and can be pushed into the composer with "→ chat." Other actions: Pull, Rebase
onto base, Update branch (agent), Resolve conflicts (agent), Stash, Discard all, Abort merge, Delete
branch, and Archive workspace. Fletch doesn't read branch-protection rules, so it surfaces all failing
checks rather than only the required ones. Many "judgment" actions (messages, PR descriptions, conflict
edits) run through the agent while local git mutations run natively in the sandbox and credentialed
remote actions go through the host broker — so your GitHub credentials never enter the sandbox.
- **Docs status:** `documented` — `guides/pull-requests-and-ci.md` covers the panel, delegated commits, PR/merge, CI rollup, comments, and the full action list.
- **Source:** `src/components/RightPanel/GitPanel/`, `src/components/RightPanel/{primaryActions,mergeGate,delegation,prComments}.ts`, `src-tauri/src/{git,git_state}.rs`, `src-tauri/src/rpc/git.rs`

### GitHub integration & connection
Cloning repos, creating repos, and opening/merging pull requests all talk to GitHub directly. You
connect GitHub once (an OAuth device flow — Fletch shows a code and verification URL); Fletch stores the
token in your macOS Keychain and uses it for GitHub REST/GraphQL calls and as the credential for git
push/fetch over HTTPS. Read operations degrade gracefully when you're not connected or the origin isn't
GitHub; mutating operations tell you to connect. Signing in with GitHub (or Google) is also offered at
onboarding for account identity and is optional. (Verified against code: `oauth.rs` states the token
"replac[es] the `gh` CLI dependency"; no `gh` invocation exists in the backend. The README's Quick
start documents the device-code flow and states no GitHub CLI is involved.)
- **Docs status:** `documented` — `guides/connecting-github.md` rewritten to the native device-flow/Keychain/broker story; `getting-started/installation.md`, `getting-started/first-repository.md`, `reference/faq.md`, and `reference/privacy.md` updated to match.
- **Source:** `src-tauri/src/github/{mod,client}.rs`, `src-tauri/src/oauth.rs`, `src-tauri/src/secrets.rs`, `src/components/GithubConnect/index.tsx`, `src/util/useGithubConnect.ts`

### Project lifecycle — open, clone, create (clone-to-PR)
Everything in Fletch happens against a project (a local git repo), added from the sidebar's "Add
project" button, which offers three ways to start: **Open a folder** (point at a local repo via a native
picker; a plain folder is initialized as a git repo for you, a git-naive on-ramp), **Clone from GitHub**
(search your repos or paste `owner/repo`/URL/SSH, pick a destination), or **Create new project** (name +
private/public + description; created locally and, when connected, published to GitHub with an `origin`
remote — otherwise publishable later). From there the normal spawn → work → PR loop takes over, so you
can go from clone to pull request without ever opening a shell.
- **Docs status:** `documented` — `getting-started/first-repository.md` covers all three paths.
- **Source:** `src-tauri/src/new_project.rs`, `src-tauri/src/commands.rs`, `src/components/NewProject/`, `src/components/Sidebar/NewProjectPopover.tsx`

---

## 5. Orchestration — Workflows

### Workflows engine
For a repeatable process — plan, implement in parallel, review in a loop, ship — Fletch has an engine
that runs a fleet of agents through a defined pipeline on a single git branch, where the checkout itself
is the shared context. You define a workflow once and launch it on any task. A workflow is a tree of
blocks:
- **Step** — one agent with a **goal** and a **gate** (the condition that means it's done).
- **Parallel** — fan several steps out from the same point, with a join policy (`all`/`any`), an
  optional merge of their results, and a concurrency cap.
- **Loop** — repeat a body (e.g. review → fix) until a step's verdict says done or a max iteration
  count is hit.
- **Orchestrate** — a lead agent supervises child agents, answers their questions, spawns children
  dynamically, and can compose bounded sub-workflows.

**Gates** are deterministic completion predicates the engine (not an LLM) evaluates: the agent wrote a
`verdict.json` marked done (the default), HEAD moved past the fork point (a commit), a named file
exists (an artifact), the project's tests pass, or you approved it. A blocked gate is re-prompted once
with the reason, then pauses the run.

**Budgets** cap spend: run-level turns and wall-clock (active driving time, not calendar time), plus
per-attempt turns, retry attempts, optional token budgets (enforced only where the provider reports
usage), and various timeouts. Exceeding a budget pauses the run — it never silently overspends or dies.

Under the hood the engine is robust and observable: each run gets a per-run **blackboard** directory
mounted read-write into every step's sandbox (`task.md`, per-step `handoff.md` and `verdict.json`,
shared scratch), steps talk to the engine only through host-brokered messaging (`report` / `ask` /
orchestrator `notify`), every engine decision is an append-only journal event so runs are fully
**resumable** across app restarts (each attempt's chat is preserved and replayable), and git handoffs
are explicit via a host-owned run repository that ferries each step's boundary commit forward.

You define workflows in **Settings › Workflows** (a visual block-tree builder with per-step agent/gate/
budget settings and a "when the run finishes" section) and can **import/export** them as portable YAML.
You launch one from a new workspace's **Workflow** tab (pick a workflow, type a task, launch); as of
0.7.10 `@` file attachments on the launch prompt are delivered to the run's entry step. **Finalize**
pushes the run branch (in a `wf/…` namespace) and optionally opens a PR against a chosen base.
- **Docs status:** `documented` — `concepts/workflow.md` rewritten as the engine concepts page ("Workflows": blocks, gates, budgets, blackboard, messaging, resumability, finalize) and `guides/building-workflows.md` covers the builder, YAML import/export, launching, and the monitor.
- **Source:** `src-tauri/src/workflow/` (spec, gates, budget, blackboard, journal, comms, gitops, scheduler, attempt, definition, yaml, tests_gate), `src/workflows/` (builder, run, spec.ts), `src-tauri/migrations/0018–0020`

### Workflow run monitor
Watching a running workflow, the monitor shows a header (task, name, `wf/…` branch, status), a **budget
meter** (turns / tokens / minutes against the frozen budgets, styled "near" at ≥90%), a **paused banner**
when the run needs you, an attempt rail listing each step and attempt, the selected attempt's preserved
chat, and a live **timeline** of plain-language events (raw JSON only behind a per-row expand). Runs also
appear as rows in the sidebar under their project.
- **Docs status:** `documented` — `guides/building-workflows.md` ("Read the run monitor").
- **Source:** `src/workflows/run/RunView/`, `src/workflows/run/{eventSummary,RunRow,status}.ts(x)`

### Workflow pauses & how you resolve them
Every wait has a named cause and its own action in the monitor — no dead buttons. **Approval:** a step
is waiting for your go-ahead → Approve. **Question:** a step asked you something (with optional choices)
→ answer inline, which resumes the run. **Blocked gate:** a step finished but its gate isn't satisfied →
Retry. **Stalled:** a step stopped making progress and didn't recover after a nudge → Retry/Resume.
**Budget exceeded:** the run hit a configured cap → raise it inline (additive +turns/+tokens/+minutes)
to continue. **Conflict:** merging parallel work hit a conflict → resolve it with an agent, or resolve it
yourself in the run's integration worktree and continue.
- **Docs status:** `documented` — `guides/building-workflows.md` ("When a run pauses" table).
- **Source:** `src/workflows/run/RunView/PausedBanner.tsx`, `src-tauri/src/workflow/{types.rs (PausedReason), comms.rs, gitops.rs, scheduler.rs}`

---

## 6. Persistence & History

### Session persistence & transcript replay
Every agent conversation is durably captured to a local SQLite database (`fletch.db` in Fletch's app
data directory), so re-opening an agent replays its full transcript. Fletch tails each provider's
on-disk transcript by byte offset (rather than re-parsing every turn) and stores a verbatim record
stream that the provider adapters normalize on read; your outgoing messages, per-turn timing, token/cost
totals, PR state, and the pending follow-up queue are all persisted too. The store carries its own schema
migrations, and a fatal database-init failure surfaces a native recovery dialog rather than crashing
silently.
- **Docs status:** `documented` — `reference/privacy.md` and `reference/settings-and-storage.md` cover local-only storage, the SQLite DB, transcript locations, and "nothing on a remote server."
- **Source:** `src-tauri/src/{database,transcripts,message_queue}.rs`, `src-tauri/migrations/`

### History (restore archived sessions)
The titlebar's History button opens a sheet of past/archived sessions: a search box, sessions grouped by
date (Today / Yesterday / …), each row showing project, task, branch, diffstat, and relative time, with a
**Restore** action to bring one back. Keyboard: ↑/↓ to navigate, Enter to restore, Esc to close.
- **Docs status:** `documented` — `guides/tips.md` (History sheet, search, Enter to restore) and `reference/faq.md` (archive vs discard).
- **Source:** `src/components/History/index.tsx`

### Project pulse & usage stats
Each project's settings modal opens with a "Project Pulse": a year-long activity heatmap (on the accent
color ramp), a day-streak, and lifetime totals — agents run, PRs opened/merged, lines added/removed, and
tokens (≈ cost). Per-agent stats (runtime, context %, tokens, cost) also appear in the sidebar's hover
popover. Usage is aggregated per day in the local database.
- **Docs status:** `partially documented` — the project pulse (heatmap + lifetime totals) is described in `guides/run-configuration.md`; the per-agent sidebar stats popover remains undocumented (minor).
- **Source:** `src/components/ProjectSettings/{ProjectPulse,pulseData}.ts(x)`, `src/components/Stats/`, `src-tauri/migrations/0014` (usage_daily), `src/components/Sidebar/AgentStatsPopover.tsx`

---

## 7. Settings & Platform

### Settings screens
Settings come in two surfaces: a quick popover (⌘,) for appearance, panel/composer toggles, and provider
enable switches, and a full-screen Settings with sections **Account** (profile, GitHub connection,
developer-tools readiness), **General** (theme/accent, side-panel and composer toggles,
notifications, sandbox engine + container auth, diagnostics/telemetry, reveal logs), **Providers**
(enable/version/path/override/re-scan), **Custom agents**, **Workflows**, **Skills**, **Tools (MCP)**,
**Experimental** (native terminal view; advanced Docker image/memory/CPU), and, in development builds
only, **Developer** (replay onboarding, fake update toast). Almost everything is app-wide; run
configuration and project name/location are the per-project exceptions.
- **Docs status:** `documented` — `reference/settings-and-storage.md` now lists all sections including Workflows, Skills, Tools, the Notifications/Sandbox/Diagnostics groups, and the Experimental Docker knobs (the dev-only Developer pane is intentionally omitted from user docs).
- **Source:** `src/components/Settings/index.tsx`, `src/components/SettingsScreen/`

### Project settings (rename, relocate & delete)
Beyond run configuration and environment variables, each project's settings modal lets you set a custom
display **name** (independent of the folder, shown in the sidebar), **relocate** the project (repoint
Fletch at a moved repo folder — running agents keep their existing checkouts; only new agents use the
new path), and — *new in 0.7.10* — **delete** the project: a confirm-gated action that permanently
removes the project with all of its agents, workspaces, and history (the repo on disk is untouched).
- **Docs status:** `documented` — `guides/run-configuration.md` covers rename, relocate, delete, env vars, and the project pulse.
- **Source:** `src/components/ProjectSettings/{index,GeneralSection,DeleteSection}.tsx`, `src-tauri/src/commands.rs` (rename_project, relocate_repo, delete_project)

### Theming (theme & accent)
Fletch supports a Dark/Light theme (flip with ⌘⇧L or the sidebar footer toggle) and six accent colors
(Copper, Rust, Olive, Sage, Ocean, Plum) — applied live across the UI, including the terminal and syntax
highlighting. A code-syntax theme can be chosen separately in the file editor. (The Density setting was
removed in 0.7.10 as non-functional.)
- **Docs status:** `documented` — `reference/settings-and-storage.md` lists theme and the six accents (Density reference removed); ⌘⇧L is in `reference/keyboard-shortcuts.md`.
- **Source:** `src/store/appearance.ts`, `src/data/providers.ts` (ACCENTS), `src/components/SettingsScreen/GeneralPane.tsx`

### Notifications (chime, native, badge)
When an agent finishes a turn or needs your input while Fletch isn't focused, it can play a chime and
post a native macOS notification, and the app's dock icon badges the number of agents with unseen
results. The sound and native-notification toggles live in **Settings › General › Notifications**; both
are best-effort (permission is requested lazily and failures are silent).
- **Docs status:** `partially documented` — the Sound and Native notifications toggles are covered in `reference/settings-and-storage.md`; the dock-badge count remains undocumented (minor).
- **Source:** `src/util/{notify,sound,window}.ts`, `src/components/SettingsScreen/GeneralPane.tsx`, `src/store/appearance.ts`

### Onboarding
On first run (and re-openable from Settings › General) Fletch walks you through a short flow: a welcome/
sign-in step (Continue with GitHub or Google via device code, optional), a Git check (with an Install Git
button), a GitHub connect step (skippable), an Agents step (CLI detection, skippable), and a Ready
summary. A data-sharing disclosure gates the first analytics event.
- **Docs status:** `documented` — `getting-started/installation.md` now describes the flow (optional sign-in, git check, GitHub connect, agent detection, skippable steps).
- **Source:** `src/components/Onboarding/`, `src/components/ProviderReadiness/index.tsx`

### Slash commands *(reworked in 0.7.10)*
The composer's `/` autocomplete now draws on four sources (Claude Code only; other agents expose none).
**Passthrough built-ins** (`/help`, `/compact`, `/init`) forward to the agent verbatim. **Local
built-ins** are handled by Fletch itself because they don't resolve over the stream: `/doctor` and
`/mcp` shell out to the real `claude` subcommand and render its output as a readable block, `/cost`
shows session token usage, `/config` opens Settings, `/clear` starts a fresh session, `/resume` opens
history. **Discovered commands** are scanned from disk with symlink-cycle protection: custom commands
(`~/.claude/commands` + `<project>/.claude/commands`), Claude skills (`~/.claude/skills` + project),
and installed plugins' commands (namespaced `<plugin>:<cmd>`); precedence on a bare-name clash is
project command > project skill > user command > user skill > plugin, and nothing shadows a built-in.
**Fletch library skills** (Settings › Skills) are invocable as `/<skill-slug>`: on send the skill
snapshot joins the spawn payload and the message is rewritten into a follow-it-now prompt with trailing
text as arguments. Still absent (TUI-only): `/model`, `/usage`, `/agents`. There is no separate command
palette; `/`, `@`, and `#` are the action surface, and all three now also work in the new-agent draft
composer.
- **Docs status:** `documented` — `reference/slash-commands.md` rewritten for the four-source model.
- **Source:** `src/data/slashCommands/{index,claude,types}.ts`, `src-tauri/src/slash_commands.rs`, `src/store/localCommands.ts`, `src/helpers/index.ts` (skill invocation), `src/components/Composer/autocomplete/`

### Keyboard shortcuts
Global shortcuts (ignored while typing in a field): ⌘B toggle sidebar, ⌘/ toggle right panel, ⌘, quick
settings, ⌘⇧L flip theme, ⌘N new agent, ⌘K focus sidebar search, Esc close popovers. Plus per-context
keys: ⌘F find (chat and terminal), Alt+↑/↓ step chat turns, number keys to answer a question card, ⌘S
flush editor save, ⌘Enter submit a commit, and ↑/↓/Enter/Esc in the History and onboarding overlays.
There is no user-editable keybinding registry.
- **Docs status:** `documented` — `reference/keyboard-shortcuts.md` lists them by area; matches the code.
- **Source:** `src/util/shortcuts.ts`, plus per-component handlers (ChatView, ChatNav, History, Onboarding, QuestionCard, FileEditor, TermPanel, CommitComposer)

### Auto-updates
Fletch updates itself in the background: on launch it checks a signed release feed, silently downloads
and stages a newer build, and shows an "Update ready" toast with Restart now (or defer — applied next
launch). Updates are cryptographically verified before installing, and Fletch never restarts on its own.
A "Check for Updates…" item is also added to the app menu. Development builds skip the update check.
- **Docs status:** `documented` — `reference/updates.md` covers staging, verification, and the toast.
- **Source:** `src-tauri/tauri.conf.json` (updater), `src-tauri/src/lib.rs`, `src/components/UpdateToast.tsx`, `src/util/{autoUpdate,appMenu}.ts`

### Crash reporting & usage analytics
Fletch can send anonymous product-usage analytics (which features are used, identified only by a random
per-install ID — never your code, repo names, branches, or prompts), toggled off with **Usage analytics**
in Settings › General. Separately, crash and error reports carry diagnostic detail (like an operation name
and exit code) but no code; reporting scrubs payloads to an allowlist of categorical fields and redacts
private paths. Both are disabled entirely in development builds and when the relevant build-time keys are
absent.
- **Docs status:** `documented` — `reference/privacy.md` covers analytics (with the opt-out), crash reports, and "no code upload."
- **Source:** `src-tauri/src/{telemetry,sentry_scrub}.rs`, `src-tauri/src/lib.rs`

### Portable git bootstrap
If your machine has no usable system `git`, Fletch can download a pinned, checksum-verified portable git
build and use it internally, so git-dependent features work even on a bare machine. Offered from the
developer-tools readiness UI.
- **Docs status:** `documented` — `getting-started/installation.md` (git requirement note).
- **Source:** `src-tauri/src/git_dist.rs`, `src/components/SettingsScreen/DevToolsStatus.tsx`, `src/util/useGitInstall.ts`

---

## Documentation gap summary

**Status (2026-07-17): the website pass against this list is done.** How each item was closed:

1. **Workflows engine** — closed. `concepts/workflow.md` rewritten as the engine concepts page
   ("Workflows"), new `guides/building-workflows.md` for the builder, YAML import/export, launch,
   monitor, and pause resolution. The site also gained a **Why Fletch** sidebar section
   (Security & Sandboxing + Working in Parallel) positioned between getting-started and the mechanics.

2. **"No Docker / git worktree" framing** — closed. New `concepts/sandboxing.md` covers both engines,
   the engine choice and recommendation, container auth, and the clone-vs-worktree design; `index.md`,
   `installation.md`, `first-agent.md`, `isolated-workspaces.md`, `parallel-agents.md`, `faq.md`,
   `settings-and-storage.md`, and wording across guides/reference were corrected.
   `getting-started/first-worktree` was renamed to `getting-started/first-agent` with a 301 redirect
   in `astro.config.mjs`.

3. **GitHub via native OAuth** — verified in code (`oauth.rs`: the token "replac[es] the `gh` CLI
   dependency"; no `gh` invocation anywhere in the backend) and closed: `connecting-github.md`
   rewritten; `installation.md`, `first-repository.md`, `pull-requests-and-ci.md`, `faq.md`, and
   `privacy.md` updated. The app README now documents the device-code flow too (no GitHub CLI).

4. **Fork** — closed with `guides/forking.md`.

5. **Settings coverage** — closed in `reference/settings-and-storage.md` and
   `guides/run-configuration.md`.

6. **Skills & MCP servers** — closed with one combined page, `reference/skills-and-tools.md`.

7. **Long tail** — History/restore and open-in-editor folded into `guides/tips.md`; project pulse
   into `guides/run-configuration.md`; notifications into `reference/settings-and-storage.md`;
   one-click install and portable git into `getting-started/installation.md`; model catalog into
   `concepts/agents.md`.

### Remaining (deliberately not done, low priority)

- **Composer mid-turn follow-ups** (live injection vs turn-boundary queueing) and the **Code panel's
  full editor/context-menu surface** are still only partially documented — candidates for a composer
  page and a `guides/reviewing-and-editing-code.md`.
- Minor undocumented UI: the titlebar **workspace status capsule**, sidebar **row status cues /
  per-agent stats popover**, and the **dock badge** count.
- ~~The app README~~ — resolved: rewritten upstream (workflows/isolation/control framing) and
  polished here (onboarding wording, custom-agent skills/tools). One inconsistency left flagged:
  the README's harness table lists **Pi as fully supported** while the app's own provider detail
  copy (`src/data/providerDetail.ts`) and the site docs call Pi *experimental* — maintainer call.
