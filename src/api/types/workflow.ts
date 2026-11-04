// ───────────────────────────── Workflows v1 types ───────────────────────────
// Mirror the serialized Rust rows in src-tauri/src/workflow/types.rs.
// JSON-typed columns arrive as parsed objects (`unknown`), not strings.

export type WfRunStatus = "pending" | "running" | "done" | "failed" | "canceled";

export type WfAttemptStatus =
  | "pending"
  | "spawning"
  | "running"
  | "gating"
  | "done"
  | "blocked"
  | "awaiting_approval"
  | "error"
  | "abandoned";

export interface WfRun {
  id: string;
  definition_id: string | null;
  name: string;
  spec: unknown;
  task: string;
  project_id: string;
  repo_path: string;
  run_dir: string;
  branch: string;
  base_sha: string;
  status: WfRunStatus;
  error: string | null;
  /** The PR this run's finalize opened, when it opened one. Null for a
   *  `push`-only spec, a `pr create` that failed, and every run that never
   *  reached finalize. Recorded on the row (migration 0029) so the host-side
   *  roadmap merge sweep can poll it with the window closed. */
  pr_number: number | null;
  pr_url: string | null;
  /** The roadmap item this run was dispatched for (migration 0028), or null for
   *  a run launched by hand. The monitor reads it to show which item the run is
   *  building, and to link back to the board. */
  roadmap_item_id: string | null;
  created_at: number;
  updated_at: number;
}

export interface WfStepExec {
  id: string;
  run_id: string;
  step_id: string;
  attempt: number;
  iteration: number;
  agent_id: string | null;
  status: WfAttemptStatus;
  gate_mode: string;
  head_start: string | null;
  head_end: string | null;
  verdict: unknown | null;
  error: string | null;
  started_at: number | null;
  ended_at: number | null;
}

export interface WfEvent {
  run_id: string;
  seq: number;
  ts: number;
  step_exec_id: string | null;
  type: string;
  payload: unknown;
}

export interface WfRunDetail {
  run: WfRun;
  attempts: WfStepExec[];
}

/** `wf:event` envelope (§7.2): the addressing fields only — fetch the payload
 *  on demand via `api.wfEvents`. */
export interface WfEventEnvelope {
  run_id: string;
  seq: number;
  type: string;
  ts: number;
  step_exec_id: string | null;
}
