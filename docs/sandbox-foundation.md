# Sandbox foundation

Fletch's differentiator is that every agent runs sandboxed by default, and
that the sandbox is invisible to the user: no images to manage, no containers
to babysit. Two engines ship: **seatbelt** (the macOS sandbox, the agent's
checkout on the host filesystem) and the **box** (a micro-VM per agent, the
checkout inside the guest). Docker and Podman still exist and are on their way
out (see "Retiring the container engines").

This page is the charter for how the engines are integrated into the rest of
the app: the rules every feature follows so that a feature written once works
under either engine, and so that engine knowledge lives in one place. It is the
upper half of the story; `docs/isolation.md` is the lower half (what each
engine enforces) and `docs/box.md` is the box runtime.

## The rules

1. **One engine fact table.** Every "does this engine …" question outside
   `sandbox/` is answered by `EngineKind::caps()` (`sandbox/engine.rs`), a
   plain struct of booleans matched exhaustively per engine, or by a function
   in `sandbox/mod.rs` whose body does the engine match. No code outside
   `sandbox/` matches on `EngineKind` variants or names `sandbox::boxes`,
   except the engine's own settings surface (`commands/boxes.rs`,
   `commands/settings.rs`), boot (`host/boot.rs`), and the event list
   (`remote/events.rs`). The guard test `sandbox::tests::engine_knowledge_stays_in_sandbox`
   pins the allowlist; it may only shrink.

2. **The checkout is reached through the executor.** Host code that reads,
   writes, lists or runs git in an agent's checkout asks
   `exec::for_path(path)` and uses the `Executor` it gets. A host `std::fs`
   call on a checkout path is a bug: under the box the host path is an empty
   placeholder. Git goes through `git_dist`, which asks the executor.
   Timer-driven reads are wrapped in `exec::passive` so a poll never boots a
   stopped VM; a user action may boot it.

3. **Provisioning is one pipeline.** A checkout comes into existence in two
   steps: *base delivery* (host: `git clone --shared`; box: `git init` in the
   guest and a push of the base over the channel) and *fixups*
   (`sandbox::provision::fixups`: origin rewrite to the HTTPS remote, frozen
   identity, `commit.gpgsign=false`, delegation and attribution hooks, HEAD
   and tracking refs). Fixups are written once over the executor and run for
   every agent checkout Fletch makes: an agent's, a secondary repo's, a
   restored one's. The one exception is a workflow run's repository
   (`workflow/gitops.rs::provision_run_repo`), which takes only the origin
   rewrite, the identity seeding and the attribution hook. It is a host-only
   internal repository that step agents adopt, not an agent checkout: the
   delegation hooks and the `commit.gpgsign` freeze exist for checkouts whose
   commits an agent makes inside its sandbox, whereas the run repo's own
   merge commits are made by the host and keep the user's signing config, no
   agent mailbox listens there, and its refs stay as cloned.

4. **Transport is asked of the sandbox.** Host git with the user's credentials
   never runs *in* an agent checkout. `git::remote::Transport` asks
   `sandbox::transport_repo(checkout)` for the repository to fetch and push
   over: the checkout itself on the host, the refreshed mirror for a box.
   A fork carries a checkpoint out of a box through the mirror too: the one
   ref it needs is fetched into the mirror under a name of its own
   (`sandbox::carry_checkpoint`), never the box's every checkpoint.

5. **Provider knowledge is one adapter.** What a provider CLI keeps outside
   the checkout, how its credential reaches a launch, where its transcripts
   land, and which engines can run it are answered by `sandbox::provider`
   (the `ProviderHome` adapter, one per provider; the credential resolution
   every engine shares, `sandbox::provider::auth`; and the single
   `supports(engine, provider)` gate). An engine applies the adapter's
   answers (the box: `sandbox::boxes::home`) and names no provider. Provider modules under `agent/providers/` never name an engine;
   they ask `sandbox::transcript_roots` and `sandbox::create_session_file`.
   The engine-independent *host write policy* (`sandbox/policy.rs`) stays the
   authority for which host directories seatbelt may grant; it is security
   reviewed and is not derived from the adapter.

6. **The launch plan's hooks are required capabilities.** Env delivery, the
   before-turn credential step and the after-exit write-back are part of
   every engine's `LaunchPlan`, with no-op values for an engine whose launcher
   runs where the agent runs. Sessions never inspect the engine.

7. **Gaps are declared, not discovered.** A feature an engine cannot serve
   yet is a `false` in `caps()`, read by the feature's entry point. `caps()`
   holds facts only; the user-facing reason lives at the feature's refusal
   site, written beside the cap read: `supervisor/run.rs` for the Run panel
   (`run_panel`), the step provisioning in `supervisor/lifecycle.rs` for
   workflow steps (`workflow_steps`). What the UI shows comes from a refusal
   site too, never from the struct: `sandbox::workspace_types` carries the
   sentence a box spawn would fail with. Nothing refuses by probing a path.

8. **Workspace type is a field.** Every spawn carries the engine it is stamped
   with, resolved from the host's choice, the project's placement and an
   optional per-session override (`SpawnRequest::workspace_type`, a
   `sandbox::WorkspaceType`), by one function (`sandbox::engine_for_spawn`).
   The override stands over both for that spawn. The composer's choice is
   `Host` or `Box` (`WorkspaceType::parse`): `Host` is seatbelt; `Box` is a
   box that waits for its image as a box-pinned project does, and a spawn
   refused with the user-facing reason when there can be no box (switch off,
   runtime unusable, provider unsupported), never a quiet fallback. A fork
   passes its parent's exact engine (`WorkspaceType::stamped`, the
   `Stamped(kind)` variant only `sandbox` builds): the child gets that engine
   whatever Settings say now, a stamped box refused like an explicit `Box`
   when it cannot be had, a stamped container engine kept as it is (its
   launch fails closed when the daemon is down). The project's placement
   (`sandbox::placement::Placement`) is a project setting only and never a
   spawn's override. An adopting agent ignores the override and runs where
   its tree lives. The UI reads one host-computed list of
   available workspace types with each one's reason when unavailable and the
   default marked (`sandbox::workspace_types`, served by the
   `available_workspace_types` command in `commands/boxes.rs`, on the desktop
   and the remote wire).

## What `caps()` says today

| capability | seatbelt | box | docker/podman |
| --- | --- | --- | --- |
| `checkout_on_host` | yes | no | yes |
| `shares_host_fs` | yes | no | no |
| `runs_codegraph` | yes | yes | no |
| `token_only_launch` | no | yes | yes |
| `added_repos_self_contained` | no | no | yes |
| `share_roots` | any | `/Users`, `/Volumes` | any |
| `run_panel` | yes | no | yes |
| `workflow_steps` | yes | no | yes |

The struct holds only these facts. A `no` has its user-facing reason at the
feature's refusal site, beside the cap read (rule 7), not in `caps()`.

The verify checks are not a row: they take a checkout path, not an agent
record, so `verify.rs` skips them for a checkout its executor says is in a
box. The handoff summary is not a row either: nothing reads such a fact yet.

## Retiring the container engines

Docker and Podman are retired when the box covers their use. The retirement
is a deletion: nothing depends on `sandbox/container/`, `sandbox/docker/` and
`sandbox/podman/` but the few files the guard test
`sandbox::tests::container_engines_are_a_leaf` pins (`sandbox/mod.rs`,
`sandbox/guarantees.rs`, `commands/settings.rs`, `host/boot.rs`). Provider
knowledge the box shares with them, credential resolution and the claude
setup-token capture included, lives under `sandbox::provider`. What the
deletion touches:

**Rust (`crates/fletch-core/src/` unless named otherwise)**

- [ ] Delete `sandbox/container/`, `sandbox/docker/`, `sandbox/podman/` and
  their `mod` lines and re-exports in `sandbox/mod.rs` (`docker_availability`,
  `DockerAvailability`, `podman_availability`, `podman_launch_blocker`,
  `PodmanAvailability`).
- [ ] Drop `EngineKind::Docker` and `EngineKind::Podman` (`sandbox/engine.rs`)
  and every exhaustive match on them:
  - `sandbox/engine.rs`: `ALL`, `as_setting`, `caps()` (the `Docker | Podman`
    row), `from_setting`;
  - `sandbox/mod.rs`: `engine_for`, `transcript_root`, `create_session_file`,
    `provision_checkout`, `restore_checkout`, `before_launch`,
    `setup_allowance`, `agent_bin`, `codegraph_bin`,
    `reap_agent_containers` (and with it `runs_in_container`), and the
    `WorkspaceType::Stamped(Docker | Podman)` arm of `engine_for_spawn`;
  - `sandbox/provider/mod.rs`: the `Docker | Podman` arm of `supports`;
  - `sandbox/guarantees.rs`: every `Docker` / `Podman` coverage row;
  - `commands/settings.rs`: the `Docker` and `Podman` arms of
    `set_sandbox_engine_impl` (and its "pick Docker or Podman on this host" copy);
  - `host/boot.rs`: `headless_container_engine`.
- [ ] Delete the `caps()` table's docker/podman column above.
- [ ] `supervisor/disposition.rs` calls `sandbox::reap_agent_containers` at
  archive and discard; the calls go with the function.
- [ ] `transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME` (`.fletch-claude-projects`,
  bound over the container's claude `projects` dir) stays for as long as
  container-era workspaces are readable: `transcript_root` and the claude
  session locators read old transcripts from it.
- [ ] Settings keys: `docker_image`, `docker_memory`, `docker_cpus`,
  `docker_version_refresh_guard` (`sandbox/docker/engine/settings.rs`),
  `podman_image`, `podman_memory`, `podman_cpus`,
  `podman_version_refresh_guard` (`sandbox/podman/settings.rs`). Drop them
  from `HOST_SETTING_KEYS` and the `set_docker_launch_settings_impl` /
  `set_podman_launch_settings_impl` setters in `commands/settings.rs`, and
  from boot's seeding in `host/boot.rs` (`set_launch_settings`,
  `init_version_refresh_guard`, `set_build_sink`, `sweep_orphans_at_startup`).
  Stored rows are left in the database, unread.
- [ ] `claude_container_token` (`sandbox::provider::auth::TOKEN_SETTING`) is
  not a container key: the claude auth chain offers it to a box launch too.
  Its fate goes with its Settings UI (`ContainerAuth.tsx` below).
- [ ] The desktop commands in `src-tauri/src/lib.rs`: `probe_docker_engine`,
  `probe_podman_engine`, `set_docker_launch_settings`,
  `set_podman_launch_settings`, `start_docker_desktop`
  (`src-tauri/src/commands/app.rs`), and the container-auth commands if the
  setup-token UI goes.

**Records stamped `docker` or `podman`**

- [ ] `sandbox_engine` on an agent row keeps its stored value. Once the
  variants are gone `EngineKind::from_setting` no longer recognises it and
  `sandbox::stamped_engine` would fall back to seatbelt, a silent change of
  engine. Instead the record fails closed: a spawn, resume or view switch
  refuses with a message naming the workspace and the retired engine, and a
  re-stamp to seatbelt or box happens only on an explicit user action.
- [ ] The host's own `sandbox_engine` setting at `docker` or `podman` would
  read as automatic (`parse_engine_setting` returns `None`; `host/boot.rs`
  then selects automatic). It only picks the engine for new agents; existing
  records are covered by the item above.

**The headless host**

- [ ] `host/boot.rs::headless_container_engine` is how `fletch-host` off
  macOS picks an engine today, and the box runs on Apple silicon only.
  Retiring containers either ends Linux host support or waits for a box
  runtime there. This is decided before the deletion, not during it.

**Frontend (`src/`)**

- [ ] `components/SettingsScreen/Sandbox/ContainerAuth.tsx`,
  `components/SettingsScreen/Sandbox/ContainerLaunchKnobs.tsx`, and the
  container rows of `components/SettingsScreen/Sandbox/index.tsx` (the
  Docker and Podman engine options, their probes and hints, the knobs).
- [ ] `components/DockerBuildToast.tsx`.
- [ ] `store/sandbox.ts`: `applyBuildEvent`, `containerBuilds`, the docker and
  podman probes, `containerAuth`, `startDockerDesktop`, and the
  `DockerBuildProgress` type (re-exported from `store/types.ts`; tests in
  `store/buildEvents.test.ts`).
- [ ] The `docker:build-progress` stream: `onDockerBuildProgress` in
  `api/events.ts`, its listener in `store/eventListeners.ts`, the
  `DockerBuildEvent` type in `api/types/sandbox.ts`; the probe, container-auth
  and `startDockerDesktop` calls in `api/domains/sandbox.ts`.
- [ ] The `dockerCrashKind` banner in `components/Workspace/index.tsx`.
- [ ] `dockerSupported` / `isDockerSupported` in `data/providers.ts` and its
  check in `components/Composer/availability.ts`.
- [ ] `isContainerEngine`, and `docker` / `podman` in `SandboxEngine`, in
  `storage/preferences.ts`; the Docker label in
  `components/TitleBar/WorkspaceStatus/isolation.ts`.
- [ ] The `set_docker_launch_settings` / `set_podman_launch_settings` ops in
  `store/gates.ts`.
- [ ] The `docker` variant of `components/ui/Badge.tsx`.

## Not done on purpose

- The box executor's file operations are async (`Executor::*_async`, one
  `tokio::process` ssh client per call, a stopped box's boot awaited), riding
  an ssh master that lives as long as the box and is ended by its stop. The
  sync methods and `RemoteExecutor::command` still block their thread, on a
  boot too: they serve callers reached through sync code, git's
  `std::process::Command` above all.
- The box reviewer (a tenant in the coder's box) is held off the coder's
  files by the guest kernel, not by file modes (both are one user in the
  guest), and writes nothing it shares with the coder: `fletch-guest exec
  --read-only … --writable … --cow … --cow-file …` gives its launch user,
  mount and pid namespaces in which everything the guest exposes of the
  coder's (its writable root, the home, its mailbox, the folders shared
  writable) is read-only with every mount nested in it; the reviewer's own
  root, mailbox and writable folder grants are real writes; and every shared
  location a CLI writes (the provider's state dirs, the scratch dirs, the
  toolchain cache, `~/.claude.json`) is a private copy-on-write view, an
  overlay whose upper layer (or, for the state file, a private copy) lies
  under the reviewer's own root. It reads the coder's state through it and
  its writes, new files included, never reach it, so no per-provider list of
  executable config is needed; the toolchain cache is no exception (the
  reviewer re-downloads what it needs into its upper). Its sessions are read
  from its upper layer. What the host writes for its launch (the provider's
  seed, the credential files it keeps in step) runs in the same view, so a
  login is shipped to, read back from and removed in the reviewer's copy,
  and none of it reaches the coder's; the provider adapter names what is
  shared at the guest's own paths (`ProviderHome::shared_state`), which hold
  every seeded path and credential file outside the writable root. `/data`
  is hidden, `/proc` is private and every
  capability is dropped (`boxes::confined_view`). Enforced, with no
  fallback: a guest that predates the options or cannot set them up refuses
  the launch. What it does not cover: scratch outside every bind (`/tmp` and
  the other tmpfs mounts), and the network namespace, which is the guest's.
- The Run panel inside a box needs port forwarding over the channel; it is a
  declared gap.
