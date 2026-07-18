// ───────────────────────────── Workflows v1 types ───────────────────────────
// Mirror the serialized Rust rows in src-tauri/src/workflow/types.rs.
// JSON-typed columns arrive as parsed objects (`unknown`), not strings.

export type WfRunStatus = "pending" | "running" | "paused" | "done" | "failed" | "canceled";

export type WfPausedReason =
  | "approval"
  | "question"
  | "blocked_gate"
  | "budget_exceeded"
  | "conflict"
  | "stalled";

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

export type WfMessageKind = "report" | "ask" | "answer" | "notify" | "decision";

export type WfMessageStatus = "queued" | "delivered" | "answered" | "expired";

export interface WfRun {
  id: string;
  definition_id: string | null;
  parent_run_id: string | null;
  name: string;
  spec: unknown;
  task: string;
  project_id: string;
  repo_path: string;
  run_dir: string;
  branch: string;
  base_sha: string;
  status: WfRunStatus;
  paused_reason: WfPausedReason | null;
  cursor: unknown | null;
  budgets: unknown;
  spent: unknown;
  error: string | null;
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

export interface WfMessage {
  id: string;
  run_id: string;
  from_step_exec_id: string | null;
  to_step_exec_id: string | null;
  kind: WfMessageKind;
  body: unknown;
  status: WfMessageStatus;
  created_at: number;
  delivered_at: number | null;
}

export interface WfRunDetail {
  run: WfRun;
  attempts: WfStepExec[];
  messages: WfMessage[];
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
