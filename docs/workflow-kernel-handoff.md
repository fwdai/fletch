# Workflow kernel — implementer hand-off

Contract for implementing the layers in [workflow-kernel-layers.md](workflow-kernel-layers.md). Read both before writing code.

## Problem

The original engine (`workflow/scheduler/` + `workflow/attempt.rs`, since deleted) spawned a complete fresh agent per step: new agent record, new git clone, new sandbox container, cold CLI boot, plus a git ferry through an intermediate run repo between steps. Cost: 15–60 s of silent dead air per step boundary, in a UI that showed one step-chat at a time with blank gaps. The sequential kernel replaced it; the features only that engine had return as kernel layers, one at a time.

## End goal

Deterministic multi-agent workflows with the surface feel of a single agent:

- One shared workspace per run. Hand-off via commits + blackboard files. Step boundary costs one CLI boot.
- One continuous thread; marker at each agent hand-off.
- **No silent second**: any moment with nothing streaming shows a named phase + live timer, derived from the journal. A silent gap is a bug, severity = crash.
- Failures are loud, named, visible in the thread. No state the UI cannot explain.

Robustness lives in the journal, gates, and pinned refs — not in per-step isolation.

## Rules for every layer

1. Success path first; edge cases as later layers. Never build the complicated robust version up front.
2. Mechanism and user-visible narrative ship in the same PR. A retry the thread doesn't show is a regression with extra code.
3. One unconditional invariant beats case-by-case handling. Writing a match over failure modes = missing invariant.
4. Reuse/simplify/rewrite decided per layer: reuse what is simple (gate mechanics, prompt composition, the verifier), rewrite what exists only to serve removed complexity. Never reuse because it exists; never rewrite because it is old. The deleted engine's code is in git history as reference, not as something to restore.
5. Exit checks per layer: (a) step-boundary dead time did not regress; (b) no new state the UI cannot explain. Fail either → simplify or revert before merge.
6. Cannot handle a case yet → explicit, journaled, visible refusal (pattern: the kernel's "runs do not resume yet"). Never silent or best-effort.

## Architecture

### Kernel — `crates/fletch-core/src/workflow/runner/`

Per run: `gitops::provision_run_repo` (its working tree IS the workspace) → detach at `base_sha`. Per step: create `wf_step_exec` row → spawn agent adopting the workspace (`SpawnReq::existing_workspace`) → stamp `agent_id` on the row before the first event (mounts the chat) → wait idle → `prompts::step_prompt` → wait turn end → gate (`gates::evaluate`: `commit` = turn ended; `verdict` = `verdict.json` result `done`, else fail run) → `gitops::boundary_commit` + `pin_step_ref` → mark done → archive → next step. Then `finalize_run` pushes the last pinned ref from the same tree. One guard: 30-min per-step wall timeout.

`workflow/service.rs` owns launch, cancel and delete and one drive task per active run; each drive task runs the kernel, and a panicked one fails its run. The spec types (`spec.rs`) admit only what the kernel runs, so there is no routing and no eligibility check: a shape the kernel cannot run does not parse.

### Adoption — supervisor/sandbox

`SpawnReq::existing_workspace` → `TrackedRepo::adopted_checkout` (migration 0038). All checkout-path reads go through `TrackedRepo::checkout_path`; no symlinks. Teardown (archive, discard, failed-spawn) spares adopted trees; run-dir removal is the only deleter. Sandbox grants derive from cwd escaping the writable root (seatbelt: writable subpath + `.git/config` deny; containers: RW bind at host path). `git::hardening::refuse_steerable_config` covers `runs_root()` and `checkouts_root()`.

### Thread UI — `src/workflows/run/RunView/ThreadView/`

Segments = step transcripts in exec-row order — never timestamp-merged. Phase rows derived purely from journal events (`phases.ts`; timers anchored to event `ts`; transient phases withheld 600 ms). `TranscriptRows` and `ChatComposer` are extracted seams from the normal chat — extend them, never duplicate transcript reduction.

Indicator contract: turn in flight → `ChatWorkingStatus` strip owns the signal; between turns / engine work → phase row owns it. Exactly one indicator at a time. Signals feeding this must be instantaneous ("happening now"), never cumulative ("has ever happened") — cumulative signals kill coverage after the first output.

## Invariants

- **I1** Durable work = pinned refs (`refs/wf/steps/<exec>`) + blackboard files. Nothing else in the workspace is durable.
- **I2** Every step attempt starts at exactly the predecessor's pinned ref. Retries: `git reset --hard <last-pinned-ref> && git clean -fd` unconditionally before every attempt — no reasoning about how the previous attempt ended.
- **I3** One live step agent per kernel workspace. Parallelism = git worktrees off the run workspace, never two writers in one tree.
- **I4** A run is driven from its launch-frozen spec (`wf_run.spec_json`) alone; every drive (launch, and resume/retry once they exist) reads that snapshot, never the definition it came from.
- **I5** The journal is the UI's only truth. Mechanisms emit events (reuse existing event types where they fit); UI derives phases/segments from journal + exec rows, never component-local state.
- **I6** Agents never own the run tree. Agent teardown spares adopted checkouts; only run-dir removal deletes the workspace.
- **I7** Host-side git in agent-writable trees goes through `git::hardening` (both roots).
- **I8** The kernel stays readable top-to-bottom. A layer that bloats `runner/` is wrong, not the kernel.

## Layer protocol

Order: retries → tests gate → ask/approval → resume → loops → parallel → budgets → latency polish. One PR each. Before layer 1: dogfood a real plan→code→review run, measure step-boundary dead time, file every unexplained gap; findings outrank ledger order.

Definition of done per layer:

1. Mechanism in its simplest honest form (ledger "Planned shape" column = intended shape).
2. Every new run state has a journaled, visible narrative + tests for the pure derivation logic.
3. A spec shape the layer adds (a block, a gate kind, a field) lands in `spec.rs`, `yaml.rs` and the frontend's spec mirror in the same PR; until then it fails to parse, loudly. A new run status lands with the migration that makes old rows consistent with it.
4. `cargo test`, clippy (0 warnings), `bun run check && bun run lint && bun run test` all green.
5. Ledger updated: layer row plan → fact; new limitations listed honestly.

Layer-specific notes:

- **Retries**: I2 is the mechanism. Narrative: marker "retry N — workspace reset" + journaled cause. Blackboard artifacts survive resets by design.
- **Tests gate**: stream the run into the timeline (label + elapsed + output tail). Red-run re-prompt must be visible — a silent second turn is the anti-pattern.
- **Ask/approval**: the pause is a run state the thread explains, and its question routes to the composer the thread already has. A run-owned agent's RPC dispatcher is today the plain git one; the op lands there.
- **Resume**: relaunch sandbox, `git worktree prune` (once worktrees exist), reset per I2, continue from first step without a `done` exec. Delete the no-resume refusal in the same PR.
- **Loops**: iterate the body in the same workspace; the loop's spec type comes back with it.
- **Parallel**: worktrees share object DB + refs → integration = local merges of pinned refs. Set `gc.auto=0` in the run workspace; `git worktree prune` on resume/cleanup; test with submodules before trusting. Children share the run's single sandbox (worktrees need the main `.git` visible).
- **Latency polish**: only with dogfood numbers. Candidates (independently droppable): pre-spawn next CLI for unambiguous straight-line successors only (no prediction, no orphan tracking), session reuse across loop iterations, async archive.

## Hazards

- `provision_codegraph_index` is the only host-side writer into the run tree (idempotent, commit-excluded). Keep it the only one, or remove it.
- Restore-after-archive clears adoption (restored agent gets its own clone). Preserve when touching restore.
- Thread mode lacks ⌘F / turn navigator (both assume one agent's turn list). Additive; not load-bearing.
- Turn-boundary indicator overlap (~100s ms) is a debounce artifact. Cosmetic. Do not fix with cumulative signals (I8's UI counterpart).
- Global `agent_lifecycle` lock: adoption spawns are light; if parallel children queue visibly on it, narrow the lock — do not pre-spawn around it.
