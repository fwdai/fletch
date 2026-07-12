# Workflows v1 — Implementation Slices

Companion to [`TECH_SPEC.md`](./TECH_SPEC.md). Each slice is scoped to be
handed to an implementation agent together with the spec: the agent should
implement **strictly that slice**, matching the spec sections it cites.

## Guiding constraints (apply to every slice)

- **Robust, not over-engineered.** Build exactly what the spec section
  says; no speculative config, no extra abstraction layers, no features
  from later slices "while you're in there". The only sanctioned
  abstraction seam is `AgentDriver` (it exists for testability).
- **Solid, not clunky.** Every user-visible state must render as something
  intentional: no raw JSON in the UI, no dead buttons, no flashes of
  unstyled/empty state. Every paused/failed state names its cause in plain
  language and offers its action.
- Follow existing codebase idioms (supervisor lock discipline — never hold
  a map lock across I/O; biome/clippy clean; existing CSS patterns).
- Each slice lands with its tests (spec §16 lists the required coverage
  per area) and keeps `cargo test` + `bun run test` green.
- Base branch for all slices: `feat/workflows-rebase`.

## Slice graph

```
Wave 0 (parallel):        S1 ──────┐        S2 ─────┐   S3 ──┐
                                   │                │        │
Wave 1:                            └──► S4 ◄────────┴────────┘   F1(←S2)
                                        │
Wave 2 (parallel):   S5   S6   S7   S8   S10   F2(←S1,S4)   F3(←S2,F1)
                          (all ← S4)
Wave 3 (parallel):        S9(←S8)        S11(←S8,S10)
Wave 4:                   S12(←S11,S2)   S13(← everything shipped)
```

Waves are the parallelism plan: slices within a wave touch disjoint files
and can be assigned to separate agents concurrently. A slice may start as
soon as its listed dependencies are merged (waves are a simplification;
dependencies are the truth).

---

## Wave 0 — foundations (S1 ∥ S2 ∥ S3)

### S1 — Schema, domain types, journal

- **Spec:** §4, §7.1–7.2, §3.1 (types/journal only).
- **Scope:** Add `0019_workflows_v1.sql`: drop the v0 tables, create the
  five new tables, and add the nullable `owner_run_id` column to the
  persisted agent/worktree record (0018 stays as shipped — it is already
  applied to dev databases and `user_version` tracking would skip a
  rewrite). Create
  `workflow/types.rs` (run/attempt/event/message types, status enums,
  serde), `workflow/journal.rs` (append within caller's transaction,
  paged reads, `wf:event` + `wf:run` tauri emission). Commands:
  `wf_get_run`, `wf_events`, `wf_list_runs` (read-only, returning empty
  data until S4 populates it).
- **Out of scope:** any scheduling logic; spec/block types (S2).
- **Files:** `src-tauri/migrations/0019_workflows_v1.sql`,
  `src-tauri/src/workflow/{mod,types,journal}.rs`, `lib.rs` registration,
  `src/api.ts` wrappers.
- **Acceptance:** migration applies on a fresh and an existing DB; journal
  append/read round-trips with per-run monotonic `seq` under concurrent
  appenders; events emit to the frontend; unit tests for seq allocation
  and paging.

### S2 — Definition spec, validation, YAML

- **Spec:** §5 (all), §13 (`wf_def_*` commands).
- **Scope:** `workflow/spec.rs` (Block tree, AgentSpec, Budgets, Gate;
  full §5.2 validation with precise error messages), `workflow/yaml.rs`
  (round-trip serde_yaml; export stripping rules; `ImportReport` with
  agent-resolution proposals + skill warnings). Commands: `wf_def_save/
  list/delete/export_yaml/import_yaml`. Persist to `wf_definition`
  (coordinate with S1's table; if developed in parallel, land after S1).
- **Out of scope:** any execution; builder UI (F1).
- **Acceptance:** the canonical YAML in spec §5.3 round-trips to identical
  Spec; every §5.2 rule has a failing-fixture test; import of a YAML
  referencing an unknown skill produces a warning, not an error.

### S3 — Blackboard + sandbox grants

- **Spec:** §8.1–8.4 (including the §8.3 staleness rule).
- **Scope:** `workflow/blackboard.rs` (provision `~/.fletch/runs/<id>/
  blackboard/`, write `task.md`, defensive `verdict.json` reader with
  typed parse errors, the stale-verdict archival helper — move to
  `history/attempt-<n>.iter-<i>.verdict.json` — and the foreign-write
  mtime scan helper). Seatbelt: run-dir
  write grant plumbed as an optional extra path through `sandbox/policy.rs`
  → `seatbelt.rs`. Docker: bind mount + fixed container path. `WF_BLACKBOARD`
  env var in both engines.
- **Out of scope:** prompts (S4), per-path hard enforcement (deferred).
- **Acceptance:** a sandboxed process can write inside the granted
  blackboard and cannot write beside it (seatbelt test, mirroring existing
  seatbelt tests); verdict reader covers missing/malformed/valid fixtures;
  Docker mount path appears in the container.

---

## Wave 1 — the core engine and builder

### S4 — Scheduler core: linear runs *(the keystone slice)*

- **Deps:** S1, S2, S3.
- **Spec:** §3.2, §6 (all except loop/parallel/orchestrate execution),
  §8.5, §9 (gates: verdict/commit/artifact/approval — **not** tests),
  §12.1–12.2, §13 (run-control commands), §11.3 watchdogs with
  **hardcoded defaults** (budget *ledger* is S5; timeouts land here
  because "no wait without a deadline" is the point of this slice).
- **Scope:** `driver.rs` (trait + `SupervisorDriver` incl. archive,
  last-activity, `owner_run_id` at spawn), `scheduler.rs` (sequence
  walker, cursor, resume with abandon-and-retry, cancel, pause-stops-
  agents, panic containment §6.1), `attempt.rs` (full lifecycle §6.3
  including subscribe-before-send turn detection and the two turn
  deadlines), `gates.rs` (4 gates), `prompts.rs` (protocol §8.5 minus
  comms section), `gitops.rs` (move v0 ops; thread `pr_base`; **the run
  repository**: provision at launch, `refs/wf/steps/*` pinning, ferry
  into the run repo as the `done` precondition, finalize-from-run-repo)
  and the one-line provisioning extension (fetch a ref from the run-repo
  path before detach, `provision.rs`). Commands: `wf_launch`, `wf_cancel`,
  `wf_approve`, `wf_retry`, `wf_resume` (no budget patch yet),
  `resume_active_runs` on startup. Delete `src/workflows/run/engine.ts`
  and point the existing v0 launch/monitor UI at the new commands
  (minimal adaptation only — F2 does it properly); sidebar filters
  run-owned agents by `owner_run_id`.
- **Out of scope:** loops, parallel, comms, tests gate, token ledger.
- **Acceptance:** MockDriver unit tests for every §16 scheduler scenario
  that applies to linear runs (spawn timeout, turn-start timeout, the
  fast-flap subscribe-before-send case, stall→nudge→abandon,
  error→retry→fail, resume abandon semantics, cancel in each phase,
  pause stops agents, driver panic → run failed,
  blocked→re-prompt→pause); the §16 gitops transport tests (cross-
  workspace ferry, ferry-failure blocks `done`, finalize after workspace
  deletion); the stub-agent integration test (2-step linear run on a temp
  repo → `done`, branch pushed, step 2 building on step 1's commit);
  every state transition has a journal event with a cause.

### F1 — Builder v2 (block editor)

- **Deps:** S2. Parallel with S4.
- **Spec:** §14.1.
- **Scope:** Rework `src/workflows/builder/` to edit the block tree:
  step cards (keep v0 card/connector visuals), parallel container, loop
  container, orchestrate container (renders even though execution comes
  later — gated behind the same validation), gate picker incl. tests,
  budgets popover, comms caps toggles. Inline rendering of `spec.rs`
  validation errors. Save through `wf_def_save`.
- **Out of scope:** YAML UI (F3); run monitor (F2).
- **Acceptance:** can build, save, reload, and edit the spec §5.3 example
  end-to-end in the UI; invalid states (loop max 0, unassigned agent)
  surface inline and block save; vitest for the spec↔editor-state mapping.

---

## Wave 2 — parallel tracks on top of S4

### S5 — Budgets ledger + enforcement

- **Spec:** §11.1–11.2, resume-with-budget-patch (§13).
- **Scope:** `budget.rs` (ledger, defaults∪spec merge frozen at launch,
  enforcement at the three points, token extraction from session records
  for providers that expose usage; turn counting for all), journal
  `budget_tick`/`budget_exceeded`, `wf_resume(budget_patch)`.
- **Guardrail:** do not build per-provider usage parsers beyond what
  session records already contain; providers without usage = turn/clock
  budgets only.
- **Acceptance:** ledger math unit tests incl. attempt/step/run rollup;
  a MockDriver run pauses at each enforcement point; resume-with-patch
  continues from the paused position.

### S6 — Tests gate

- **Spec:** §9.4. **Deps:** S4.
- **Scope:** resolve test command (project override > `run_detect`),
  bounded execution in the step worktree under the run-panel profile,
  output-tail capture into `gate_evaluated`, degrade-to-verdict warning.
- **Acceptance:** green/red/timeout/no-command fixtures; re-prompt after a
  red run includes the output tail.

### S7 — Loop blocks

- **Spec:** §6.6 (loop). **Deps:** S4.
- **Scope:** loop execution in the sequence walker: iteration counters in
  the cursor, verdict-driven exit, `loop_max_reached` continue policy,
  blackboard persistence across iterations.
- **Acceptance:** MockDriver: exit-on-done, revise×N-then-max, resume
  mid-loop restores the iteration counter.

### S8 — Parallel blocks (note-producing) + joins

- **Spec:** §6.6 (parallel), §12.3 `integrate: none`. **Deps:** S4.
- **Scope:** concurrent child attempts (bounded by `max_concurrent`),
  join `all|any` (any = first success wins, fails only when all children
  error), loser cancellation + archival, `integrate_skipped` journaling.
  **No merging.**
- **Acceptance:** MockDriver matrix over both join policies × child
  outcomes; resume mid-stage re-drives only unfinished children.

### S10 — Comms router (report / ask / notify / human Q&A)

- **Spec:** §10.1 (minus decide/compose), §10.4. **Deps:** S1, S4.
- **Scope:** `comms.rs` + RPC ops `wf_report`/`wf_ask`/`wf_notify`,
  caps validation, `wf_message` persistence, **engine-owned delivery per
  §10.4** (router appends to the attempt inbox and pokes the driver; the
  scheduler is the only prompter — inject mid-turn where supported, else
  fold pending messages into one engine-composed prompt), pending-ask
  gate deferral, human-question pause + `wf_answer`, prompt protocol
  comms section (§8.5 item 6), auto-forwarded lifecycle events (delivered
  to orchestrator once S11 exists; until then `ask` with no orchestrator
  routes to the human — which is the complete, useful v1 behavior).
- **Acceptance:** routing/caps matrix tests; end-to-end: a step's `wf_ask`
  pauses the run, `wf_answer` delivers and resumes, and the gate is not
  evaluated while the ask is pending; multiple queued messages to an idle
  agent produce exactly one turn; every message journaled.

### F2 — Run Monitor v2

- **Deps:** S1 + S4 (extends incrementally as S5–S10 land). 
- **Spec:** §14.2.
- **Scope:** journal-driven `RunView`: attempt rail with preserved-chat
  links (abandoned attempts dimmed, never hidden), virtualized event
  timeline with live appends, paused-reason banners wired to their
  actions (approve / retry / answer / raise-budget / conflict actions
  appear as their slices land), budget meter, sidebar badge.
- **Guardrail:** the timeline renders event *summaries* in product
  language ("Gate `tests` failed — 3 failing tests"), payload JSON only
  behind an expand affordance.
- **Acceptance:** replaying a recorded journal fixture renders a correct
  timeline; every `paused_reason` has a styled banner + working action;
  chat of an abandoned attempt from a previous app session opens.

### F3 — YAML import/export UI

- **Deps:** S2, F1. **Spec:** §14.3.
- **Scope:** export via save dialog; import flow with the mapping dialog
  (`ImportReport` rendering, per-agent map-vs-embed choice, skill
  warnings).
- **Acceptance:** export→import of the §5.3 example on a machine without
  the custom agents produces a runnable definition via embedded specs.

---

## Wave 3

### S9 — Code-producing parallel: merge + conflicts

- **Spec:** §12.3 `integrate: merge`. **Deps:** S8.
- **Scope:** sequential child merges of ferried refs **in the run repo**,
  `merge_*` journal events, `paused(conflict)` + `wf_resolve_conflict`
  with modes (a) spawn conflict-resolution step (workspace forked from
  the pinned conflicted-merge ref) and (c) human resolves in the run
  repo's integration worktree, then resumes. Mode (b) orchestrator
  routing arrives with S11.
- **Acceptance:** temp-repo integration tests: clean merges in spec order;
  induced conflict pauses with the file list; both resolution modes drive
  the run to `done`.

### S11 — Orchestrator role + decisions

- **Spec:** §10.2, orchestrate execution in §6.6. **Deps:** S8, S10.
- **Scope:** orchestrate-stage execution (orchestrator lives for the
  stage; static `body` children auto-spawn at stage entry; the `children`
  template authorizes dynamic `spawn_child` only), stage completion per
  §6.6 (join met → engine prompts the orchestrator for its concluding
  verdict; `stage_done` ends early), lifecycle auto-forwarding,
  `wf_decide` variants with validation + journaling, `ask` routing to
  orchestrator, conflict decision routing (S9 mode b), orchestrator
  prompt protocol.
- **Acceptance:** MockDriver: orchestrator answers a child's ask; denied
  over-max spawn returns a structured error and journals `…denied`;
  stage does not complete before the orchestrator's concluding verdict;
  orchestrator stall falls back to human escalation.

---

## Wave 4

### S12 — Dynamic composition (`wf_compose`)

- **Spec:** §10.3. **Deps:** S11, S2.
- **Scope:** fragment validation (reusing `spec.rs` wholesale), depth +
  budget-reservation enforcement, sub-run creation/driving with
  `parent_run_id`, integrate-at-join, nested rendering in Run Monitor +
  sidebar, cancel cascade.
- **Acceptance:** composed sub-run runs to done and merges at the join;
  over-budget / over-depth / caps-escalating fragments are rejected with
  structured errors; canceling the parent cancels the sub-run.

### S13 — Cleanup, docs, polish pass

- **Deps:** everything above merged.
- **Scope:** delete all remaining v0 dead code (`ferryNotes`, loop
  markers, v0 run tables/types if any remnants), README + in-app copy,
  a full-pipeline manual QA script (spec §5.3 example run for real), and
  a UI polish pass driven by that QA (empty states, loading states,
  transition jank) — this slice exists so "not clunky" gets dedicated
  attention rather than being everyone's afterthought.

---

## Sizing and assignment notes

- **S4 is the keystone and the largest slice** (roughly the size of S1+S2
  combined); everything in Wave 2 is deliberately cut small so multiple
  agents can fan out the moment S4 merges. If S4 needs splitting, the seam
  is: S4a driver+attempt+gates (MockDriver-tested), S4b scheduler+resume+
  commands+integration test — sequential, same owner.
- Frontend (F1–F3) and backend tracks only synchronize at command
  signatures — freeze those in S1/S2/S4 first (they're specified in §13,
  so later slices must not change them without updating the spec).
- Every slice's PR description should cite the spec sections it
  implements; deviations require editing TECH_SPEC.md in the same PR so
  the spec stays canonical.
