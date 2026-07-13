# Workflows v1 — Full-Pipeline Manual QA

A hands-on runbook that exercises the whole workflows feature end-to-end by
running the canonical [`TECH_SPEC.md`](./TECH_SPEC.md) §5.3 `feature-pipeline`
example **for real** against a throwaway repo. It is the acceptance ritual for
S13 and a smoke test to re-run after any workflows change.

The engine narrates every decision as a journal event, so this runbook is
mostly *"do X, then confirm the timeline shows Y."* If an expected state never
appears, that is the bug — capture the run id and the last journal event.

> Convention: ✅ = expected observable state. ⏱ = a deadline/watchdog is
> running; the run must not hang silently past it.

---

## 0. Prerequisites

- A debug build: `bun install && bun tauri dev`.
- At least two agent CLIs authenticated on `PATH` — ideally `claude` (used by
  `planner`/`reviewer`; supports mid-turn injection) and `codex` (the `coder`).
  If you only have one, map every agent alias to it in the builder; the flow is
  identical, only concurrency realism drops.
- A **scratch git repo** with a real (small) test command, cloned as a Fletch
  project. A repo where `bun test`/`cargo test`/`npm test` resolves via
  `run_detect` exercises the tests gate; any trivial repo still exercises the
  rest.
- Start from a clean slate: no paused runs from a previous session lingering in
  the sidebar.

---

## 1. Build the definition

Open **Settings → Workflows** and create a new definition ("Create your first
workflow"). Reproduce the §5.3 `feature-pipeline` spec:

1. **Agents.** Add `planner` (claude/opus), `coder` (codex), `reviewer`
   (claude, skill `code-review`).
2. **`plan` step** → agent `planner`, gate **artifact** `PLAN.md`, budget
   `turns: 3`.
3. **`orchestrate` block** → agent `planner`; `children: { agent: coder,
   max: 3 }`; join `all`; integrate `merge`; comms `[report, ask]`;
   compose `{ max_sub_runs: 2, max_depth: 2 }`.
4. **`loop` block** → `max: 3`, `until: { step: review, verdict: done }`,
   body `[review (reviewer, verdict gate), fix (coder, commit gate)]`.
5. **`finalize`** → `push: true`, `open_pr: true`, `pr_base: main`.

Checks while building:

- ✅ Each block renders as its own container (step card, parallel/orchestrate
  container, loop bracket) — no raw JSON, no unstyled boxes.
- ✅ Introduce an **invalid** state deliberately (e.g. set `loop.max` to 0, or
  leave a step's agent unassigned) → the inline `spec.rs` validation error
  appears and **Save is blocked**. Fix it → the error clears and Save enables.
- ✅ Save, reload the app, reopen the definition → it round-trips identically.

## 1b. YAML round-trip (F3)

- Export the definition via the save dialog → open the `.yaml`; confirm it
  matches the §5.3 shape and that no local `custom_agent` id leaked (embedded
  base/model/instructions/skill *names* instead).
- Import it back on a machine/profile **without** the custom agents → the
  mapping dialog offers *map-to-local* vs *use-embedded* per agent and warns
  about any missing skill (a warning, never a hard error). Choosing *embedded*
  yields a runnable definition.

---

## 2. Launch

In a project's empty/draft workspace, flip the **Agent / Workflow** toggle to
*Workflow* (it appears once at least one definition exists), pick the
definition, and enter a concrete task, e.g. *"Add a `/health` endpoint
returning `{status:'ok'}` with a test."* Press Enter to launch.

- ✅ Run appears in the sidebar; status `running`; a `run_launched` event is
  first in the timeline with the resolved `base_sha`.
- ✅ `~/.fletch/runs/<run-id>/` now exists with `blackboard/task.md` and a
  `repo/` run repository.
- ✅ The run branch is `wf/<slug>-<suffix>`.
- ✅ The `planner`/`coder` step agents do **not** show in the normal sidebar
  agent list (they live under the run, filtered by `owner_run_id`).

---

## 3. Stage: `plan` (linear step, artifact gate)

- ✅ Timeline: `attempt_spawned` → `attempt_ready` → `prompt_sent {kind: step}`
  → `turn_ended`.
- ✅ When the planner writes `PLAN.md`, `gate_evaluated {mode: artifact}`
  records success and a `boundary_commit` follows.
- ✅ The attempt's chat opens from the attempt rail (via the preserved
  `ChatView`) and stays openable for the rest of the run.
- ⏱ If the agent never writes `PLAN.md`, the gate stays unmet → one re-prompt →
  then `paused(blocked_gate)` with a **Retry** affordance (not a silent hang).

## 4. Stage: `orchestrate` (fan-out, merge, comms, compose)

- ✅ The orchestrator (`planner`) comes up for the stage; it may `spawn_child`
  up to `children.max` (3). Each spawn journals
  `child_spawn_requested`/`approved`; an over-max spawn is **denied** with a
  structured error and a `…denied` event.
- ✅ A child `wf_ask` routes to the orchestrator (a `message_routed` event and
  a delivered message), and gate evaluation for that child **defers** until the
  answer turn completes (`ask_pending`).
- ✅ Optional: verify **compose** — if the orchestrator issues `wf_compose`, a
  `subrun_launched` appears, the sub-run renders nested under the parent in the
  monitor + sidebar, and `subrun_finished` precedes its merge. Over-budget /
  over-depth fragments are rejected with a structured error.
- ✅ `integrate: merge` merges each successful child's ferried ref in spec
  order in the run repo (`merge_started` → `merge_done`).
- ✅ **Conflict path:** to exercise it, seed two children that edit the same
  line. The stage pauses `paused(conflict)` with the conflicting file list.
  Resolve it two ways across separate runs:
  - (a) spawn a conflict-resolution step → its workspace forks from the pinned
    conflicted merge; resolve + commit → run continues.
  - (c) resolve in the run repo's integration worktree (openable in the editor)
    → resume.
- ✅ After join, the engine prompts the orchestrator once for its concluding
  verdict; the stage does not complete before that verdict (or an explicit
  `wf_decide {stage_done}`).

## 5. Stage: `loop` (review/fix, verdict-driven)

- ✅ `loop_iteration {iteration, max}` on each pass.
- ✅ A `revise` review runs `fix`, then re-enters the loop; a `done` review
  **exits immediately**, skipping the trailing `fix` (no needless final fix).
- ✅ **Stale-verdict guard:** across iterations the reviewer's previous
  `verdict.json` is archived to `<step>/history/attempt-*.iter-*.verdict.json`
  *before* the new prompt — a leftover verdict never satisfies the new
  iteration's gate.
- ✅ Exhausting `max` journals `loop_max_reached` and continues (not a
  failure); the reviewer's last verdict rides into the PR body.

## 6. Finalize

- ✅ `finalize_pushed {branch}` pushes the run branch **from the run repo**
  (works even if step workspaces are gone).
- ✅ `finalize_pr {url}` opens a PR targeting `main` (the `pr_base`); on failure
  it's `finalize_pr {error}`, surfaced, not swallowed.
- ✅ Run status → `done`.

---

## 7. Budgets, watchdogs, resilience

- ✅ **Budget meter** tracks turns/tokens/wall-clock vs the ledger. Launch a run
  with a tiny `turns` budget → it pauses `paused(budget_exceeded)` with a
  *"resume with +N turns"* action that patches the budget and re-drives.
- ⏱ **Stall:** if an agent goes quiet past `stall_timeout_secs`,
  `watchdog_stalled` fires one nudge; continued silence → `error(stalled)` →
  retry, then `paused(stalled)` (resumable), never a silent death.
- ✅ **Restart resilience:** with a run `running`, quit and relaunch the app →
  `resume_active_runs` re-drives it; any mid-flight attempt is
  `abandoned("resume")` and a fresh attempt starts from the last committed
  HEAD. Abandoned attempts stay in the rail (dimmed), chats still open.
- ✅ **Pause stops processes:** while paused, the run's live step agents are
  stopped (no idle CLI processes accumulate); workspaces + chats preserved.
- ✅ **Cancel:** `Cancel` on a live run stops the agent, marks the attempt
  abandoned and the run `canceled`; a parent cancel cascades to sub-runs.

## 8. Human Q&A

- ✅ A step's `wf_ask` with no orchestrator pauses the run `paused(question)`;
  the banner shows the question (+ options) and an answer box. `wf_answer`
  delivers the answer and resumes; the gate is not evaluated while the ask is
  pending.

## 9. Cleanup

- ✅ **Delete run** (terminal runs only): the run row's delete button arms on
  the first click and its tooltip states chats will be gone. Click again →
  run-owned workspaces discarded, `~/.fletch/runs/<id>/` removed, rows deleted
  (sub-runs cascade). The run leaves the sidebar; its chats no longer open.

---

## Polish checklist (S13)

Walk the run above and confirm — no raw JSON in any user-facing surface; every
`paused_reason` has a styled banner **and** a working action; empty states
(no definitions, no runs, empty timeline) read as intentional; loading states
never flash unstyled; abandoned attempts are dimmed, never hidden; the timeline
renders product-language summaries with payload JSON only behind an expand.
File anything clunky as a follow-up with the run id + offending event seq.
