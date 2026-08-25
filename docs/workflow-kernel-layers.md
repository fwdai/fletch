# Workflow kernel: the layering ledger

The kernel (`src-tauri/src/workflow/runner/`) is the simple sequential success
path for a run: one workspace (the run repository's own working tree), one agent
per step, one line of commits. It provisions the run repo, detaches it at the run
base, and then for each step creates the `wf_step_exec` row, spawns an agent that
*adopts* that workspace (`SpawnReq::existing_workspace`), waits for idle, sends
the step prompt, waits for the turn to end, evaluates the gate, boundary-commits
and pins `refs/wf/steps/<exec>`, and archives the agent. It journals the same
event types and writes the same `wf_step_exec` statuses as the old engine, so the
run monitor is unchanged.

The clone-per-step engine (`src-tauri/src/workflow/scheduler/`) is untouched and
keeps serving every spec the kernel does not claim. Routing happens in one place
— the drive-task spawn site in `scheduler/drive.rs` — and is derived from the
run's launch-frozen spec, so every drive of a given run reaches the same runner:

> A run is a kernel run iff every top-level block is a `step` and every gate is
> `commit` or `verdict`.

Layers get added to the kernel one at a time. The rule for each: ship the
mechanism **and** the user-visible narrative together. A retry the timeline
doesn't show, or a test gate whose progress never reaches the UI, is not a
layer — it is a regression with extra code.

## The ledger

| Layer | Current home (old engine) | Plan |
| --- | --- | --- |
| Retries | attempt loop in `scheduler/steps.rs::execute_step` | Rebuild thin: `reset --hard` to the last pinned ref + `clean -fd`, rerun the step in the same workspace, reuse `prompts::retry_prompt`. |
| Tests / artifact gates | `attempt.rs` gate eval + `workflow/tests_gate.rs` | Reuse the runner mechanics as-is; stream test progress into the timeline and make the re-prompt visible instead of a silent second turn. |
| Ask / answer + approval | `workflow/comms/` | Reuse mostly as-is — the mailbox, routing and pause reasons are independent of how steps are executed. |
| Loops | `scheduler/steps.rs::run_loop` | Rebuild thin on the kernel (iterate the body in the same workspace), reusing the spec types (`Loop`, `Until`) unchanged. |
| Budgets | `workflow/budget.rs` | Defer. When it lands, reuse the accounting (`Ledger`, `EffectiveBudgets`) rather than re-deriving caps. |
| Resume | run cursor + run-repo step refs | Rebuild simpler: relaunch the sandbox, `reset --hard` the workspace to the last pinned step ref, continue from the next step. |
| Parallel / orchestrate | `scheduler/parallel.rs`, `scheduler/orchestrate.rs` | Git worktrees off the run workspace, so children fork cheaply and integrate locally. The old engine serves these specs until then. |
| Stall / nudge watchdogs | `attempt.rs` (`drive_turn`) | Replaced by the single per-step wall timeout. Add nuance only if real hangs demand it. |
| Latency polish | — | Straight-line pre-spawn of the next step's CLI, session reuse across loop iterations, asynchronous archive. |

## v0 limitations

Each of these is a deliberate omission, not an oversight. The kernel fails loudly
rather than pretending.

- **No resume.** A kernel run found non-terminal at startup is failed with
  "kernel runs do not resume yet"; its stale execs are abandoned and their agents
  stopped. Nothing is re-run, because the workspace already contains the finished
  steps' commits.
- **No retries.** A step that errors, times out, or fails its gate fails the run,
  with the cause journaled and on `wf_run.error`.
- **`commit` and `verdict` gates only.** `commit` is satisfied by the turn
  ending (the boundary commit captures whatever the agent did); `verdict`
  requires `result: "done"`. Anything else — `revise`, `blocked`, missing,
  unparsable — fails the run. Any other gate kind makes a spec ineligible.
- **Sequential only.** No parallel stages, no loops, no orchestrate blocks, no
  sub-runs.
- **No budget enforcement.** Turn/token/wall caps are not charged or checked; a
  step's declared `turns_per_attempt` still reaches the prompt as guidance.
- **Comms are not acted on.** A step may declare `report`/`ask` caps and the
  prompt will describe them, but the kernel does not pause on a human ask.
- **One guard.** A single per-step wall timeout (30 minutes) covers spawn through
  turn end. There is no turn-start deadline, stall clock, or nudge.
