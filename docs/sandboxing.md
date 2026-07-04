# Sandbox Engine Layer — Implementation Plan

Plan for decoupling Fletch's isolation layer from `sandbox-exec` into a swappable
**sandbox engine** abstraction with two adapters: **seatbelt** (`sandbox-exec`,
current behavior) and **Docker**. Written for implementing agents: each slice is
self-contained, states its dependencies, and has acceptance criteria. Line numbers
are approximate — anchor on function/type names.

## Slice graph

```
A1 (engine seam, refactor-only)
 ├─→ A2 (workspace provisioning: worktree | clone)   ─┐
 ├─→ B1 (docker primitives: probe, image, sweep)      ├─→ B2 (docker engine launch path)
 ├─→ B3 (RPC watcher poll fallback)                   │        ├─→ C2 (UI surfacing)
 └─→ C1 (settings + engine selection)  ───────────────┘        └─→ D1 (container auth chain)
E (crate extraction) — optional, last
```

A2, B1, B3, C1 are parallelizable once A1 lands. B2 needs A2 + B1 + C1.
C2 and D1 need B2 and are parallelizable with each other.

## Hard invariants (every slice must preserve these)

1. **Path identity.** Anything mounted into a container is mounted at its exact
   host path, and the container gets `HOME=<host home>`. Transcripts
   (`~/.claude/projects/<munged-cwd>/…`), RPC payloads, and diff paths all embed
   absolute host paths; the transcript reader derives its lookup dir from the
   agent's cwd (`agent.rs` `transcript_path`, honors `CLAUDE_CONFIG_DIR`). No
   path translation anywhere in the app.
2. **The user's real repo never enters a container.** Container workspaces are
   self-contained clones (see A2). Never bind-mount the origin repo or its
   `.git` — a writable `.git/hooks` inside the container is a host-execution
   escape when the user next runs git.
3. **Secrets never in argv.** Tokens go into containers via `-e VAR` is argv —
   so NO: pass secret env via `--env-file` on a 0600 tempfile or stdin-safe
   mechanism. `docker run -e VAR` (name only, no `=value`) forwards from the
   docker CLI's own environment without appearing in `ps` — use that form:
   set the var on the spawned docker CLI process env, pass bare `-e VAR`.
4. **No orphaned containers.** Every container carries labels
   `fletch.instance=<host-instance-id>` and `fletch.agent-id=<id>`; startup
   sweep removes containers whose instance is dead (mirror of
   `sandbox::cleanup_nested_rpc_roots`).
5. **Seatbelt behavior is bit-identical** until the user flips the setting.
   Default engine is `sandbox-exec`. The Run panel stays seatbelt-only in v1.
6. **GitHub tokens never enter any sandbox.** Push/PR/merge stay host-side via
   the file-mailbox RPC (`rpc/git.rs`); the mailbox dir is bind-mounted.

## Current architecture map (read these before starting)

All paths relative to `src-tauri/src/`.

- `sandbox.rs` — SBPL profile builders. `SANDBOX_EXEC` const, `build_profile`
  (agent), `build_run_profile` (Run panel), `profile_tempfile`, `nested_*_root`
  redirects + `cleanup_nested_*` sweeps, `sbpl_string`/`canonical`/
  `resolve_existing_prefix` helpers.
- `agent.rs` — `enum Agent { Pty, Managed, PerTurn }`; `SpawnSpec` (cwd,
  sandbox_root, rpc_dir, session/model/effort); `rpc_env` (FLETCH_RPC_DIR);
  spawn fns `spawn_pty`, `spawn_pty_native`, `spawn_managed`, `spawn_per_turn`
  → `spawn_exec`; helpers `prepare_sandbox`, `prepare_pty_args`,
  `prepare_managed_args`; `resolve_agent_bin`/`resolve_claude`;
  `PER_TURN_AGENTS` descriptor table. Every spawn builds argv
  `sandbox-exec -f <profile> <bin> <args…>` and parks the profile
  `NamedTempFile` on the session struct.
- `pty_session.rs` / `managed_session.rs` / `exec_session.rs` — transport
  shapes. `PtySpawn{program,args,cwd,env,cols,rows}`,
  `ManagedSpawn{program,args,cwd,env}`,
  `ExecSpawn{program,prefix_args,profile,cwd,session_id,model,stdout_is_json,env}`.
  All inherit host env + `login_shell_env()` + `git_dist::child_env()`, then
  caller env last. Kill = process-group HUP→TERM→KILL escalation
  (`pty_session.rs kill_process_group`).
- `supervisor/lifecycle.rs` — `spawn_session`: computes
  `cwd = workspace::repo_worktree_path(agent_id, subdir)`,
  `sandbox_root = workspace::agent_parent_dir(agent_id)`,
  `rpc_dir = rpc::mailbox_dir(agent_id)` + `ensure_mailbox`; creates worktrees
  via `git::worktree_add_detached`; spawns RPC watcher.
  `supervisor/disposition.rs` — archive/restore (`worktree_add_branch`).
- `supervisor/run.rs` — `sandboxed_run_command`: Run-panel sandbox-exec argv
  (seatbelt-only in v1, do not port).
- `rpc.rs` + `rpc/git.rs` — file mailbox (`FLETCH_RPC_ROOT`/`FLETCH_RPC_DIR`,
  `requests/`+`responses/`, atomic tmp+rename), `RpcDispatcher` trait, git ops
  `git_commit`/`git_push`/`open_pr`/`git_update_branch` executed **host-side**
  with the GitHub token from `github/client.rs git_auth_env()`.
- `workspace.rs` — `WORKTREES_ROOT_ENV`, `worktrees_root()`,
  `agent_parent_dir`, `repo_worktree_path`.
- `bin_resolve.rs` — PATH/login-shell/override binary resolution.
- `database.rs` — key-value `settings` table (`get_setting`/`set_setting`),
  agent records. Frontend settings: `src/storage/settings.ts`,
  `src/storage/preferences.ts`; UI panes under `src/components/SettingsScreen/`.
- Auth today: Fletch injects **no** Anthropic credentials; the child inherits
  the app env (`ANTHROPIC_API_KEY` if the user exports it) or claude uses its
  own login (macOS Keychain, or `~/.claude/.credentials.json`).

---

## Slice A1 — Engine seam (refactor-only)

**Goal:** introduce the `SandboxEngine` trait and move all seatbelt code behind
a `SandboxExecEngine` adapter. Zero behavior change.

**Depends on:** nothing. **Blocks:** everything else.

**Changes:**

1. Convert `sandbox.rs` into a module dir:
   - `sandbox/mod.rs` — public API: trait + types, `pub fn current_engine() -> Arc<dyn SandboxEngine>`
     (hardcoded to `SandboxExecEngine` in this slice; C1 makes it setting-driven).
     Re-export everything `supervisor/run.rs` uses (`build_run_profile`,
     `profile_tempfile`, `SANDBOX_EXEC`, `nested_*`) so the Run panel keeps
     compiling untouched.
   - `sandbox/seatbelt.rs` — the existing file contents (profile builders,
     helpers, nested-root logic, existing tests) plus `SandboxExecEngine`.
   - `sandbox/engine.rs` — types:

```rust
pub enum EngineKind { SandboxExec, Docker }

/// Inputs shared by every agent launch.
pub struct AgentLaunchCtx<'a> {
    pub agent_id: &'a str,
    pub writable_root: &'a Path,   // agent parent dir (sandbox_root)
    pub rpc_dir: &'a Path,
    pub cwd: &'a Path,             // primary worktree/clone
    pub home: &'a Path,
    pub interactive: bool,         // PTY (true) vs stdio (false)
}

/// Everything a session needs to exec + later kill the process.
pub struct LaunchPlan {
    pub program: PathBuf,          // seatbelt: /usr/bin/sandbox-exec; docker: docker
    pub prefix_args: Vec<String>,  // seatbelt: ["-f", profile, agent_bin]; docker: run argv incl. image + agent_bin
    pub env: Vec<(String, String)>,        // set on the spawned CLI process
    pub keepalive: Keepalive,      // parked on the session struct
    pub kill: KillHandle,
}

pub enum Keepalive { None, Profile(tempfile::NamedTempFile) }
pub enum KillPlan { ProcessGroup, Container { name: String } }

/// Teardown bound at launch. Sessions call kill()/is_alive() and never
/// inspect the variant; the Engine variant captures the engine that produced
/// the plan, so teardown never consults current_engine() — a session must be
/// torn down by the engine that launched it even after the setting changes.
pub enum KillHandle {
    ProcessGroup,                                            // session pgid kill is the whole story
    Engine { engine: Arc<dyn SandboxEngine>, plan: KillPlan }, // docker etc.
}

pub trait SandboxEngine: Send + Sync {
    fn kind(&self) -> EngineKind;
    /// Build the launch recipe wrapping `agent_bin` (absolute path or, for
    /// docker, in-image name). Agent CLI args are appended by the caller
    /// after `prefix_args`, i.e. final argv = prefix_args ++ agent_args.
    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan>;
    /// Engine-side teardown/liveness for a plan this engine produced (reached
    /// only via KillHandle::Engine). Defaults: no-op kill, is_alive = true.
    /// Docker overrides both in B2 — containers die independently of the host.
    fn kill(&self, plan: &KillPlan) -> Result<()> { Ok(()) }
    fn is_alive(&self, plan: &KillPlan) -> bool { true }
}
```

2. `SandboxExecEngine::launch_agent` = today's `prepare_sandbox` +
   `["-f", profile_path, agent_bin]`, `env = []`, `Keepalive::Profile`,
   `KillHandle::ProcessGroup` (the trait's default kill/is_alive suffice —
   seatbelt overrides neither). Honors `CLAUDE_CONFIG_DIR` exactly as
   `agent.rs prepare_sandbox` does now.
3. Refactor `agent.rs`: `prepare_pty_args`/`prepare_managed_args` return only
   the **agent CLI args** (claude flags); `spawn_pty`, `spawn_pty_native`,
   `spawn_managed`, `spawn_exec` call `sandbox::current_engine().launch_agent(...)`
   and assemble `program = plan.program`,
   `args = plan.prefix_args ++ agent_args`, `env = plan.env ++ rpc_env(...)`.
   Replace the `_profile_file: Option<NamedTempFile>` fields on
   `PtyAgent`/`ManagedAgent` and `ExecSpawn.profile` with `Keepalive`
   (and store the `KillHandle` next to it — used in B2, plumb it now).
4. Sessions: add `kill_plan: KillHandle` to `PtySpawn`/`ManagedSpawn`/`ExecSpawn`;
   kill paths call `self.kill_plan.kill()` before the existing process-group
   escalation and never inspect the variant — adding an engine touches no
   session code. `current_engine()` is resolved once via `OnceLock` and is
   never consulted at kill time.
5. Do **not** touch `supervisor/run.rs` beyond import paths.

**Acceptance:**
- `cargo test` passes; existing `sandbox.rs` tests moved, not rewritten.
- Manual: claude PTY + custom view + one per-turn agent (codex) spawn, run a
  turn, RPC commit works — identical to before.
- `rg 'SANDBOX_EXEC' src-tauri/src` shows uses only inside `sandbox/` and
  `supervisor/run.rs`.

---

## Slice A2 — Workspace provisioning (worktree | clone)

**Goal:** make workspace creation an engine-owned concern and implement the
clone-based provisioner the Docker engine requires — tested **under seatbelt**
via a dev flag, so all git edge cases are flushed out before Docker exists.

**Depends on:** A1. **Parallel with:** B1, B3, C1.

**Why clone:** a linked worktree's `.git` file points at the origin repo's
`.git/worktrees/<name>` (absolute host path holding index/HEAD). Mounting the
origin `.git` into a container is a sandbox escape (invariant 2); mounting it
read-only makes git flaky (lock files). A self-contained clone under
`agent_parent_dir(agent_id)` needs zero extra mounts, and because it still
lives on the host FS at the normal path, all host-side git (diff polling,
`rpc/git.rs` commit/push/merge, archive/restore) operates on it unchanged.

**Changes:**

1. New `sandbox/provision.rs`:

```rust
pub enum WorkspaceMode { Worktree, Clone }

pub struct CheckoutSpec<'a> {
    pub source_repo: &'a Path,   // user's real repo root
    pub base_ref: &'a str,       // commit/branch to check out (detached)
    pub dest: &'a Path,          // workspace::repo_worktree_path(agent_id, subdir)
}

pub fn provision(mode: WorkspaceMode, spec: &CheckoutSpec) -> Result<()>;
pub fn teardown(mode: WorkspaceMode, spec: &CheckoutSpec) -> Result<()>;
```

2. `Worktree` arm: move the existing calls (`git::worktree_add_detached`,
   restore-path `worktree_add_branch`, and the corresponding
   `git worktree remove/prune` on teardown) behind this API. Callers:
   `supervisor/lifecycle.rs spawn_session` (worktree creation sites) and
   `supervisor/disposition.rs` (restore).
3. `Clone` arm:
   - `git clone --no-hardlinks <source_repo> <dest>` then
     `git -C <dest> checkout --detach <base_ref>`. `--no-hardlinks` is
     mandatory: hardlinked objects would let a container corrupt origin
     objects through shared inodes.
   - Rewrite `origin` to the real remote:
     `git -C <source_repo> remote get-url origin` → `git -C <dest> remote
     set-url origin <url>` (skip if source has no `origin`; then keep the
     local-path remote and note it in a log line).
   - Copy `user.name`/`user.email` into the clone's local config if the source
     repo has repo-local identity (global gitconfig already applies host-side).
   - Teardown = `rm -rf dest` (it's self-contained).
   - Large-repo cost is accepted in v1. Do NOT add `--filter=blob:none`
     (local promisor remotes are fragile); leave a `// TODO(perf)`.
4. Dev flag: settings key `workspace_mode` (`worktree` default | `clone`),
   read in `spawn_session`. Not exposed in UI — set via sqlite for testing.
   (B2 forces `Clone` whenever the engine is Docker, regardless of this key.)
5. Audit + fix host-side consumers against a clone (they should already work;
   verify each): `git_state.rs` diff/status polling, `rpc/git.rs`
   `git_commit`/`git_push`/`open_pr`/`git_update_branch` (update-branch does
   `fetch` first — fine, remote URL is real), archive/restore in
   `disposition.rs` (restore of a clone workspace: recreate by clone, then
   `git fetch origin <branch>` / checkout).

**Acceptance:**
- Unit tests for both provisioners against fixture repos (tempdir): detached
  checkout at base ref, remote URL rewritten, no hardlinks
  (`find .git/objects -links +1` empty on APFS test), teardown clean.
- Manual with `workspace_mode=clone` under seatbelt: full agent lifecycle —
  spawn, edit, `git_commit` RPC, `git_push`, `open_pr`, `git_update_branch`
  with a conflict, archive, restore. Diff panel renders throughout.

---

## Slice B1 — Docker primitives (no spawn wiring)

**Goal:** standalone Docker plumbing: availability, image, cleanup.

**Depends on:** A1 (module layout only). **Parallel with:** A2, B3, C1.

**Changes:** new `sandbox/docker/` module:

1. `docker/cli.rs` — locate the docker binary via `bin_resolve::resolve_bin("docker", home)`
   (Docker Desktop installs to `/usr/local/bin/docker`; add it to
   `common_bin_paths` if missing). All invocations get explicit timeouts.
2. `docker/probe.rs` — `pub fn availability() -> DockerAvailability`:
   `docker version --format '{{.Server.Version}}'` with 2s timeout →
   `Available { server_version } | NotInstalled | DaemonDown`. Cache for 5s
   (probe is called from UI polling).
3. `docker/image.rs` — embedded Dockerfile as a const:

```dockerfile
FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl ca-certificates ripgrep jq procps \
 && rm -rf /var/lib/apt/lists/*
RUN npm install -g @anthropic-ai/claude-code
# entrypoint.sh: seed a minimal $HOME/.claude.json if absent (see B2), then exec "$@"
COPY entrypoint.sh /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
```

   - `ensure_image(tag) -> Result<()>`: `docker image inspect fletch-agent:<v>`
     → if missing, `docker build` from a tempdir containing the Dockerfile +
     entrypoint. Tag = `fletch-agent:<sha256(dockerfile+entrypoint)[..12]>` so
     content changes rebuild automatically. Serialize builds with a process-wide
     `Mutex`. Emit progress via a callback (C2 wires it to UI events).
   - Settings key `docker_image` overrides the image entirely (skip build;
     user-supplied images must have `claude` on PATH and git installed —
     document in C2 copy).
4. `docker/cleanup.rs` — instance identity + orphan sweep:
   - Host instance id: reuse pid-based liveness like `nested_state_root`:
     label `fletch.host-pid=<pid>`.
   - `sweep_orphans()`: `docker ps -aq --filter label=fletch.host-pid` →
     inspect labels → for pids not alive (`sandbox::pid_alive`),
     `docker rm -f <ids>`. Call from app startup (`lib.rs`, next to
     `cleanup_nested_rpc_roots`) — only when the probe says the daemon is up;
     never block startup on it (spawn a thread).
5. Integration tests behind `#[ignore]` + env `FLETCH_DOCKER_TESTS=1`
   (probe, build tiny image, sweep a labeled container). Unit-test pure parts
   (tag derivation, label parsing) unconditionally.

**Acceptance:** unit tests pass without Docker; `FLETCH_DOCKER_TESTS=1 cargo
test -- --ignored` passes on a machine with Docker Desktop running.

---

## Slice B3 — RPC watcher poll fallback

**Goal:** the host-side mailbox watcher must not depend on FS events —
container-originated writes over VirtioFS don't reliably produce FSEvents.

**Depends on:** A1 (none functionally — keep it after A1 to avoid rebase noise).

**Changes:** in `supervisor/rpc_watch.rs`: alongside the existing event-driven
trigger, add a 500ms interval tick that calls the same `rpc::process_pending`.
Processing must be idempotent/racy-safe — it already is (request files are
consumed, responses written atomically via tmp+rename; verify
`handle_request_file` tolerates a concurrently-deleted request and skips
requests that already have a response file). Add a test: drop a request file
with the event watcher disabled (feature-gate or inject), assert a response
appears within 1s.

**Acceptance:** RPC round-trip works with FS events artificially disabled.

---

## Slice C1 — Settings + engine selection

**Goal:** user-visible engine choice, availability-gated; engine resolved at
spawn time and sticky per agent.

**Depends on:** A1 (types), B1 (probe). **Parallel with:** A2, B3.

**Changes:**

1. Settings key `sandbox_engine`: `"sandbox-exec"` (default) | `"docker"`.
2. Backend (`lib.rs` commands): `get_sandbox_engine`, `set_sandbox_engine`
   (validates against probe before accepting `docker`), `probe_docker_engine`
   returning `{ status: "available"|"not-installed"|"daemon-down", version? }`.
3. `sandbox::current_engine()` (from A1) now reads the setting; falls back to
   seatbelt with a `tracing::warn!` if docker selected but daemon down at
   spawn time — and surfaces that in the spawn error path so the UI can show
   why (C2 polishes the surfacing; here a clear error string suffices).
4. Stickiness: add `sandbox_engine` TEXT column to the agents table (new
   migration file under `src-tauri/migrations/`, following existing naming).
   `spawn_session` stamps the engine at first spawn and reuses the stored value
   on respawn/view-switch/restore, so a settings change never re-engines a
   live agent. When the stored engine is docker, `workspace_mode` is forced to
   `Clone` (A2) — record that too if you find restore paths re-deriving it.
5. Frontend: `src/storage/preferences.ts` typed accessor;
   `src/components/SettingsScreen/GeneralPane.tsx` (or ExperimentalPane if
   feature-flagged — implementer's choice, prefer General) — a select with
   three visual states: seatbelt (always), docker enabled (probe available),
   docker disabled + hint ("Install/start Docker Desktop"). Include the caveat
   copy: "Docker agents run in a Linux container: builds and tests run on
   Linux, not macOS. Only Claude Code is available in containers for now."

**Acceptance:** setting persists; docker option disabled when daemon down;
selecting docker while a seatbelt agent runs does not affect it; per-agent
engine survives app restart.

---

## Slice B2 — Docker engine launch path

**Goal:** `DockerEngine` implementing `SandboxEngine`; claude runs full turns
in containers.

**Depends on:** A1, A2, B1, C1. **Blocks:** C2, D1.

**Design decisions (fixed — do not re-litigate):**
- **One container per process, agent = PID 1** via `docker run --init --rm`.
  No long-lived container + `docker exec` (kill/exit-code semantics are broken
  in that model). PTY sessions → `docker run -it` spawned under portable-pty
  (docker CLI forwards TTY resize). Managed → `docker run -i`. Per-turn claude
  → `docker run --rm -i` per turn (300–500ms startup accepted).
- **Mounts (exactly these, all at identical host paths):**
  `writable_root` (agent parent dir), `rpc_dir`, `<home>/.claude`. Nothing else
  from the host. `-w <cwd>`.
- **Container HOME = host home path** (`-e HOME=<home>`); the image entrypoint
  `mkdir -p "$HOME"` and seeds a minimal `$HOME/.claude.json`
  (`{"hasCompletedOnboarding": true}`) if absent. Do **not** bind-mount
  `~/.claude.json` (single-file bind mounts break on claude's atomic
  rename-replace). `.claude.json` is therefore container-ephemeral; transcripts
  and auth live in the mounted `~/.claude`, so resume works across turns.
- **Run as root inside the container** in v1. Docker Desktop/VirtioFS maps
  ownership so host files appear owned by the user. Leave `// TODO(linux-host)`
  for UID mapping.
- **Per-turn non-claude providers refuse to spawn** under docker with error
  `"<label> isn't available in Docker sandboxes yet"` — check in
  `supervisor/lifecycle.rs` before spawn.

**Changes:**

1. `sandbox/docker/engine.rs` — `DockerEngine::launch_agent` returns:

```text
program: <docker bin>
prefix_args:
  run --rm --init [-t if ctx.interactive] -i
  --name fletch-<agent_id>-<8-char nonce>       # nonce: derive from session_id + monotonic counter
  --label fletch.host-pid=<pid> --label fletch.agent-id=<agent_id>
  -v <writable_root>:<writable_root>
  -v <rpc_dir>:<rpc_dir>
  -v <home>/.claude:<home>/.claude
  -w <cwd>
  -e HOME -e FLETCH_RPC_DIR -e CLAUDE_CONFIG_DIR? -e TERM -e COLORTERM
  -e ANTHROPIC_API_KEY -e CLAUDE_CODE_OAUTH_TOKEN     # bare names: forwarded from CLI env, never in argv (invariant 3)
  [--memory <mem> --cpus <cpus>]                       # settings keys docker_memory/docker_cpus, defaults 4g / 2
  fletch-agent:<tag>
  claude                                               # in-image binary name — skip bin_resolve/resolve_claude
env (on the docker CLI process): HOME, FLETCH_RPC_DIR, TERM=xterm-256color,
  COLORTERM=truecolor, plus auth vars resolved by D1 (until D1: forward
  ANTHROPIC_API_KEY / CLAUDE_CODE_OAUTH_TOKEN from login_shell_env if present)
keepalive: Keepalive::None
kill: KillHandle::Engine { engine: <self>, plan: KillPlan::Container { name } }
```

   `ensure_image` (B1) is called before first launch per app run; surface
   build failure as a spawn error.
2. `agent.rs`: when the plan's kind is docker, pass in-image name `"claude"`
   instead of `resolve_claude()`; `CLAUDE_CONFIG_DIR`: if the host sets a
   non-default one, mount it too (same rules as seatbelt's
   `claude_config_extra`) and forward the var.
3. Kill/liveness: override the trait's `kill` and `is_alive` defaults. `kill`:
   spawn `docker kill -s TERM <name>`; after the existing grace period
   escalate `docker kill <name>`; then the existing local process-group kill
   for the docker CLI itself. `is_alive`: `docker inspect -f '{{.State.Running}}'`
   — containers die independently of the host (daemon stop, OOM), and this is
   the health surface the UI polls. On session exit (normal or killed),
   best-effort `docker rm -f <name>` (usually a no-op with `--rm`).
4. Exit/error mapping: docker CLI exit code 125 (daemon error) / 126/127
   (image problems) → distinct, user-readable `PtyExit`/`ExecExit` messages
   ("Docker daemon stopped", "sandbox image missing claude"), not raw codes.
5. `spawn_session` (lifecycle): when agent engine = docker, force
   `WorkspaceMode::Clone` (A2), skip `login_shell_env` overlay decisions —
   overlay is harmless on the docker CLI, leave sessions untouched.

**Acceptance (manual E2E, Docker Desktop running, engine=docker):**
- Fresh claude agent: native PTY view renders, turn completes, tool use works.
- Custom (managed) view: stream-json turn completes; view-switch respawn
  resumes the same session (transcripts via mounted `~/.claude`).
- RPC: `git_commit` + `git_push` + `open_pr` round-trip from inside the
  container (mailbox over bind mount, B3 poll fallback).
- Diff panel updates as the agent edits (host reads the clone directly).
- Kill mid-turn: process gone in container (`docker ps` empty) within grace.
- Force-quit the app mid-turn → relaunch → `docker ps -a` has no fletch
  containers after startup sweep.
- Stop Docker Desktop mid-turn → agent shows a clear error state, app healthy.
- `ps aux | grep docker` during spawn: no token values visible in argv.

---

## Slice C2 — UI surfacing

**Depends on:** B2, C1. **Parallel with:** D1.

1. Agent header/badge shows the engine (small "Docker" chip when containerized).
2. New-agent flow: when engine=docker, non-claude providers disabled with
   tooltip (matches the backend refusal from B2).
3. Image build progress: first docker spawn triggers a build — show a
   determinate-ish progress toast/panel fed by B1's progress callback; spawn
   proceeds when the build finishes.
4. Docker-down error states rendered distinctly (icon + "Start Docker Desktop"
   action that opens the app via `open -a Docker`).
5. Settings copy from C1 finalized; add `docker_image`, `docker_memory`,
   `docker_cpus` as advanced fields (Developer/Experimental pane).

**Acceptance:** all states reachable and readable: docker not installed,
daemon down, image building, running, daemon died mid-session.

---

## Slice D1 — Container auth chain

**Goal:** containers authenticate to Anthropic robustly. Host Keychain doesn't
exist in a container; the app currently injects nothing.

**Depends on:** B2. **Parallel with:** C2.

**Resolution chain (first hit wins), implemented in `sandbox/docker/auth.rs`:**

1. `CLAUDE_CODE_OAUTH_TOKEN` stored by Fletch (step 3 below) → set on docker
   CLI env, forward with bare `-e`.
2. Login-shell env (`bin_resolve::login_shell_env`) has `ANTHROPIC_API_KEY`
   or `CLAUDE_CODE_OAUTH_TOKEN` (also forward `ANTHROPIC_BASE_URL` /
   `ANTHROPIC_AUTH_TOKEN` if present — proxy setups) → forward those.
3. `<home>/.claude/.credentials.json` exists → nothing to do (the `~/.claude`
   mount carries it; refresh writes land on the host — desired).
4. None of the above → agent spawn under docker fails fast with a typed error
   the UI turns into a "Connect Claude for containers" call-to-action.

**Setup-token flow (settings UI, for Keychain-only users — the common case):**
- Modal: "Run `claude setup-token` in your terminal and paste the token." Text
  field + validate button. (Do not try to drive the interactive OAuth flow
  from inside the app in v1 — `claude setup-token` opens a browser and prints
  the token to the terminal; parsing that from a hidden PTY is brittle. A
  copyable command + paste field is robust.)
- Validate shape (`sk-ant-oat…` prefix; accept unknown prefixes with a
  warning) and store in the `settings` table under `claude_container_token` —
  same plaintext-in-sqlite posture as `github_token` (consistency over
  novelty; keychain migration is a separate future task for both).
- Status row shows which chain step is active ("Using API key from shell
  profile" / "Using pasted token" / "Not connected").
- Token is injected **only** into containers, never into seatbelt agents (they
  keep using the user's own login).

**Acceptance:** on a machine whose claude login is Keychain-only: docker agent
fails with the CTA → paste token → agent completes a turn. With
`ANTHROPIC_API_KEY` exported in `~/.zshrc` instead: works with no setup. Token
absent from argv (`ps`) and from logs.

---

## Slice E — Crate extraction (optional, defer until the layer is proven)

Move `sandbox/` (engine, seatbelt, docker, provision) into a workspace crate
`crates/agent-sandbox` with no tauri/app deps, converting the repo to a cargo
workspace. App-specific bits stay behind (settings reads, UI events; inject
via small traits/callbacks). ⚠️ CI: when promoting transitive deps to direct
deps in the new crate, copy their `default-features`/`features` exactly —
ubuntu CI breaks otherwise while macOS passes (known past failure). Do this
slice only after B2/C2/D1 have shipped and stabilized.

---

## Testing summary

- Every slice: `cargo test` in `src-tauri` (`cargo test --manifest-path
  src-tauri/Cargo.toml`), plus `bun run check` for TS slices (C1/C2).
- Docker integration tests: `#[ignore]`, opt-in via `FLETCH_DOCKER_TESTS=1`.
- The manual E2E checklist in B2 is the release gate for the feature flag.
- Frontend has no test runner for panes — rely on type-check + manual pass.

## Explicit non-goals (v1)

- Run panel under Docker; devcontainer adapter (it will be a config-resolution
  layer over `DockerEngine`: read `.devcontainer/devcontainer.json` for
  image/features — the trait already accommodates it); network confinement;
  Linux/Windows hosts; non-claude agents in containers; partial clones for
  large repos.
