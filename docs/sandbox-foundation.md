# Sandbox foundation

Fletch's differentiator is that every agent runs sandboxed by default, and
that the sandbox is invisible to the user: no images to manage, no containers
to babysit. Two engines ship: **seatbelt** (the macOS sandbox, the agent's
checkout on the host filesystem) and the **box** (a micro-VM per agent, the
checkout inside the guest). Off macOS the box is the only engine.

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
   site, written beside the cap read: `supervisor/run.rs` reads `run_panel`
   that way, though every engine says yes today. What the UI shows comes from a refusal
   site too, never from the struct: `sandbox::workspace_types` carries the
   sentence a box spawn would fail with. Nothing refuses by probing a path.

8. **Workspace type is a field.** Every spawn carries the engine it is stamped
   with, resolved by one function (`sandbox::engine_for_spawn`) from the
   project's placement and an optional per-session override
   (`SpawnRequest::workspace_type`, a `sandbox::WorkspaceType`). Every new
   workspace runs in a box; seatbelt is what it gets when no box can be had
   right now (image still downloading, download failed, a machine that cannot
   boot one) or when the project or the workspace is told to stay on the Mac.
   There is no global engine setting and no switch. Off macOS there is no
   seatbelt: every new agent is a box (`boxes::engine_for_new_agent`), and a
   host that cannot boot one says why at boot and refuses each spawn with it.
   The override stands over the placement for that spawn. The composer's choice is
   `Host` or `Box` (`WorkspaceType::parse`): `Host` is seatbelt, refused with
   the user-facing reason off macOS, where there is none (`seatbelt::refusal`,
   the one place that fact lives); `Box` is a box that waits for its image as
   a box-pinned project does, and a spawn refused with the user-facing reason
   when there can be no box (runtime unusable, provider unsupported), never a
   quiet fallback. The same two refusals decide what the composer's list
   offers as available and what `sandbox::engine_for` launches, so a type the
   host does not have is never offered, stamped or launched. A fork
   passes its parent's exact engine (`WorkspaceType::stamped`, the
   `Stamped(kind)` variant only `sandbox` builds): the child gets that engine
   whatever the automatic choice says now, a stamped box refused like an explicit `Box`
   when it cannot be had. A record stamped with a retired engine (`docker`,
   `podman`) is refused at every launch and fork
   (`sandbox::ensure_engine_not_retired`), never moved to another engine. The project's placement
   (`sandbox::placement::Placement`) is a project setting only and never a
   spawn's override. An adopting agent ignores the override and runs where
   its tree lives. The UI reads one host-computed list of
   available workspace types with each one's reason when unavailable and the
   default marked, the default's reason saying why when it is not a box (`sandbox::workspace_types`, served by the
   `available_workspace_types` command in `commands/boxes.rs`, on the desktop
   and the remote wire).

9. **A command in the checkout runs where the checkout is.** The Run panel's
   setup and dev commands run on the host under the Run profile for a host
   checkout, and in the guest for a box's (`sandbox::guest_run`): the same
   `fletch-guest exec --env-file` entry an agent launch takes, a launch of the
   box held against its idle stop for as long as the command runs, and the
   dev server's port forwarded over an ssh connection of its own to a free
   port on the host, which the panel's link opens (`docs/box.md`). The
   supervisor asks `sandbox::checkout_is_remote` and the facade; it names no
   engine. Detection reads the checkout through the executor
   (`run_detect::Tree`), one listing then the files it needs, and the panel's
   look is passive: a stopped box answers with its last detection.

10. **The toolchain cache is one type.** A launch gets a `ToolchainCache`
    (`sandbox/toolchain_cache`): the directory its child may write and the env
    pointing the child's toolchains into it, from the constructor for where
    the child runs. `on_host` serves a seatbelt agent and a host Run command,
    `in_guest` a box. What an engine does with it is its own: seatbelt grants
    the root, a box creates it on its disk, which starts as a clone of the
    project's seed (docs/box.md). The seed's rules are engine-neutral
    (`sandbox/seed/`); an engine supplies, through `seed::AtRest`, how a
    workspace at rest is taken and reduced, and starts a new workspace from
    the seed (`boxes/seed.rs`). Deleting a project and boot's
    sweep call `remove_project_cache` and `reclaim_toolchain_caches`, which
    cover every engine: each one states where it keeps per-project caches on
    this machine (`cache_dir_on_host`, matched exhaustively), so a new store
    is a new answer there, never a new call site.

## What `caps()` says today

| capability | seatbelt | box |
| --- | --- | --- |
| `checkout_on_host` | yes | no |
| `shares_host_fs` | yes | no |
| `share_roots` | any | `/Users`, `/Volumes` |
| `run_panel` | yes | yes |

The struct holds only these facts. A `no` has its user-facing reason at the
feature's refusal site, beside the cap read (rule 7), not in `caps()`.

The verify checks are not a row: they take a checkout path, not an agent
record, so `verify.rs` skips them for a checkout its executor says is in a
box. The handoff summary is not a row either: nothing reads such a fact yet.

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
