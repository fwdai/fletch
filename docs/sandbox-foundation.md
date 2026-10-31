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
   every checkout Fletch makes: an agent's, a secondary repo's, a restored
   one's, and a workflow run's.

4. **Transport is asked of the sandbox.** Host git with the user's credentials
   never runs *in* an agent checkout. `git::remote::Transport` asks
   `sandbox::transport_repo(checkout)` for the repository to fetch and push
   over: the checkout itself on the host, the refreshed mirror for a box.
   Checkpoints (`refs/fletch/*`) are part of what the mirror carries, so a
   fork can carry code out of a box.

5. **Provider knowledge is one adapter.** What a provider CLI keeps outside
   the checkout, how its credential reaches a launch, where its transcripts
   land, and which engines can run it are answered by `sandbox::provider`
   (the `ProviderHome` adapter and the single `supports(engine, provider)`
   gate). Provider modules under `agent/providers/` never name an engine;
   they ask `sandbox::transcript_roots` and `sandbox::create_session_file`.
   The engine-independent *host write policy* (`sandbox/policy.rs`) stays the
   authority for which host directories seatbelt may grant; it is security
   reviewed and is not derived from the adapter.

6. **The launch plan's hooks are required capabilities.** Env delivery, the
   before-turn credential step and the after-exit write-back are part of
   every engine's `LaunchPlan`, with no-op values for an engine whose launcher
   runs where the agent runs. Sessions never inspect the engine.

7. **Gaps are declared, not discovered.** A feature an engine cannot serve
   yet is a `false` in `caps()` with the user-facing reason beside it, read by
   the feature's entry point and by the UI. Nothing refuses by probing a path.

8. **Workspace type is a field.** Every spawn carries the engine it is stamped
   with, resolved from the host's choice, the project's placement and an
   optional per-session override, by one function
   (`sandbox::engine_for_spawn`). The UI reads one host-computed list of
   available workspace types with each one's reason when unavailable.

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
| `verify` | yes | no | yes |
| `workflow_steps` | yes | no | yes |
| `handoff_summary` | yes | no | no |

A `no` carries its reason in the struct, which is what the UI shows.

## Retiring the container engines

Docker and Podman are retired when the box covers their use. The retirement
is a deletion only if, by then, nothing depends on `sandbox/container/`,
`sandbox/docker/` and `sandbox/podman/` except the engine registry
(`sandbox::engine_for`), the settings surface and the caps table. Three
things are prepared ahead of it:

- The box's provider adapters depend on nothing in the container module; the
  credential resolution they share lives under `sandbox::provider`.
- The fate of records stamped `docker` or `podman` is decided: they fail
  closed with a message naming the workspace, and a re-stamp to seatbelt or
  box happens only on an explicit user action.
- The headless host (`fletch-host` off macOS) boots on a container engine
  today and the box runs on Apple silicon only. Retiring containers either
  ends Linux host support or waits for a box runtime there. This is decided
  before the deletion, not during it.

## Not done on purpose

- The box executor is a synchronous ssh client per command, and obtaining its
  channel can block a runtime worker while a VM boots. A persistent async
  channel is a later optimisation; the seam that would change is `exec.rs`
  and `sandbox/boxes/executor.rs` only.
- The box reviewer (a tenant in the coder's box) has no read-only grant on
  the coder's tree: both are one user in the guest. Its brief and capability
  grant hold it.
- The Run panel inside a box needs port forwarding over the channel; it is a
  declared gap.
