// run/types.ts — the execution domain model.
//
// A WorkflowRun is one execution of a workflow definition. Its step list is
// *snapshotted* at launch (editing the workflow later never mutates an in-flight
// run). A WorkflowRunStep is one *execution* of a step — loops produce several
// rows for the same `step_id`, distinguished by `iteration`.

import type { AdvanceMode, WorkflowStep } from "../storage";

export type RunStatus =
  | "pending" // created, checkout not yet set up
  | "running" // a step is active
  | "paused" // awaiting manual approval, or a blocked gate
  | "done" // all steps completed, branch pushed
  | "failed" // a step errored and was not recovered
  | "canceled";

export type RunStepStatus =
  | "pending"
  | "running"
  | "blocked" // ran to turn-end but its gate isn't satisfied
  | "awaiting_approval"
  | "done"
  | "error";

/** Terminal run states — no further orchestration happens. */
export const RUN_TERMINAL: ReadonlySet<RunStatus> = new Set<RunStatus>([
  "done",
  "failed",
  "canceled",
]);

/** Run states the resume scan should pick up and continue driving. */
export const RUN_ACTIVE: ReadonlySet<RunStatus> = new Set<RunStatus>([
  "pending",
  "running",
  "paused",
]);

export interface WorkflowRun {
  id: string;
  workflow_id: string;
  /** Snapshot of the workflow's display name at launch. */
  name: string;
  /** Snapshot of the workflow's steps at launch — the source of truth for this run. */
  steps_snapshot: WorkflowStep[];
  /** The task the run was launched on (the user's prompt). */
  task: string;
  project_id: string;
  repo_path: string;
  /** App-owned run directory: ~/.quorum/checkouts/<run-id>/ (sandbox root). */
  run_dir: string;
  /** The single branch this run pushes to (computed once at launch). */
  branch: string;
  /** Commit the run forked from (diff base for the whole run). */
  base_sha: string;
  status: RunStatus;
  /** The step definition id currently executing (null before start / after end). */
  current_step_id: string | null;
  /** Loop iteration of the current step (0 on first pass). */
  current_iter: number;
  created_at: number;
  updated_at: number;
}

export interface WorkflowRunStep {
  id: string;
  run_id: string;
  /** The id of the step in the workflow definition this execution corresponds to. */
  step_id: string;
  iteration: number;
  /** The spawned agent backing this execution (null until spawned). */
  agent_id: string | null;
  status: RunStepStatus;
  advance_mode: AdvanceMode;
  /** Checkout HEAD when the step started — the per-step diff base. */
  head_start: string | null;
  /** Checkout HEAD when the step finished. */
  head_end: string | null;
  /** Short human summary / handoff line captured at completion. */
  summary: string | null;
  started_at: number | null;
  ended_at: number | null;
}

/** A run plus its step executions, as returned by `get_run`. */
export interface RunWithSteps {
  run: WorkflowRun;
  steps: WorkflowRunStep[];
}
