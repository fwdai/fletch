# Workflow kernel: the layering ledger

The kernel (`crates/fletch-core/src/workflow/runner/`) is the one workflow
engine: the sequential success path for a run. One workspace (the run
repository's own working tree), one agent per step, one line of commits. It
provisions the run repo, detaches it at the run base, and then for each step
creates the `wf_step_exec` row, spawns an agent that *adopts* that workspace
(`SpawnReq::existing_workspace`), waits for idle, sends the step prompt, waits
for the turn to end, evaluates the gate, boundary-commits and pins
`refs/wf/steps/<exec>`, and archives the agent. Every phase is journaled, so the
run monitor derives everything it shows from the journal and the exec rows.

A spec is a flat list of steps, each gated `commit` or `verdict`. Anything else
is not part of the spec types (`workflow/spec.rs`) and fails to parse.

Layers get added to the kernel one at a time. The rule for each: ship the
mechanism **and** the user-visible narrative together. A retry the timeline
doesn't show, or a test gate whose progress never reaches the UI, is not a
layer — it is a regression with extra code.

## The ledger

Every layer is not yet present. The plan is its intended shape; when a layer
lands, its row turns from plan into fact.

| Layer | Planned shape |
| --- | --- |
| Retries | `reset --hard` to the last pinned ref + `clean -fd` (I2), then rerun the step in the same workspace with the previous cause in its prompt. Narrative: a "retry N — workspace reset" marker and the journaled cause. |
| Tests / artifact gates | New gate kinds in `spec::Gate` and `gates::evaluate`. The tests gate runs the project's checks with `verify::Verifier` in the run workspace after the turn, streams progress into the timeline, and makes any re-prompt visible rather than a silent second turn. |
| Ask / answer + approval | A mailbox op for run-owned agents, a paused run state the UI can explain (a new `RunStatus` with its migration), and the commands that lift it; the step resumes in its workspace. |
| Resume | Relaunch the sandbox, `reset --hard` the workspace to the last pinned step ref, continue from the first step without a `done` exec. |
| Loops | A loop block in the spec; iterate its body in the same workspace until a verdict says `done` or the max is reached. |
| Parallel / orchestrate | Git worktrees off the run workspace, so children fork cheaply and integrate by local merges of pinned refs. |
| Budgets | Turn / token / wall caps in the spec, charged per step and checked between turns, with a stop the timeline names. |
| Box placement | Provision the run workspace inside the project's box and adopt it there, so a boxed project's steps run as the box's tenants like any other adopting agent. Until then a launch for such a project is refused up front (`service::lands_in_a_box`). |
| Stall / nudge watchdogs | Covered by the single per-step wall timeout. Add nuance only if real hangs demand it. |
| Latency polish | Straight-line pre-spawn of the next step's CLI, session reuse across loop iterations, asynchronous archive. |

## v0 limitations

Each of these is a deliberate omission, not an oversight. The kernel fails loudly
rather than pretending.

- **No resume.** A run found non-terminal at startup is failed with a reason
  saying runs do not resume yet — including a run the deleted clone-per-step
  engine left `running`. Its stale execs are abandoned and their agents
  stopped. Nothing is re-run, because the workspace already contains the
  finished steps' commits. Runs that engine left `paused` were failed by
  migration 0056.
- **No retries.** A step that errors, times out, or fails its gate fails the run,
  with the cause journaled and on `wf_run.error`.
- **`commit` and `verdict` gates only.** `commit` is satisfied by the turn
  ending (the boundary commit captures whatever the agent did); `verdict`
  requires `result: "done"`. Anything else — `revise`, `blocked`, missing,
  unparsable — fails the run. Any other gate kind, or any block other than a
  step, does not parse: a YAML import naming it is refused, a stored definition
  holding it is left out of the list with a warning, and a run whose frozen spec
  holds it fails when driven.
- **Sequential only.** No parallel stages, no loops, no orchestrate blocks, no
  sub-runs.
- **No budgets.** Turn / token / wall caps are not part of the spec. A YAML file
  declaring them is refused; a stored JSON spec carrying old `budgets` fields
  reads with them ignored.
- **No comms.** A step agent has no `wf_*` mailbox ops: it cannot ask, report or
  be answered mid-run.
- **No boxes.** The run workspace is a host tree, so every step adopts it on
  this Mac's sandbox. A project whose agents would land in a box (its stored
  placement, or the automatic choice with a box ready, or any host off macOS)
  is refused at launch with that reason, never run on the host behind the
  project's isolation.
- **One guard.** A single per-step wall timeout (30 minutes) covers spawn through
  turn end. There is no turn-start deadline, stall clock, or nudge.
