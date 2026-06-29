// run/storage.ts — typed access to the workflow run-persistence backend.

import { invoke } from "@tauri-apps/api/core";
import type { RunWithSteps, WorkflowRun, WorkflowRunStep } from "./types";

/** Upsert the whole run row. The engine holds live state and writes it on each
 *  transition (created_at is preserved server-side). */
export const saveRun = (run: WorkflowRun) => invoke<{ id: string }>("workflow_save_run", { run });

/** A run plus its step executions, or null if unknown. */
export const getRun = (id: string) => invoke<RunWithSteps | null>("workflow_get_run", { id });

/** All runs, newest-updated first — used by the resume scan and history view. */
export const listRuns = () => invoke<WorkflowRun[]>("workflow_list_runs");

/** Upsert one step execution. */
export const saveRunStep = (step: WorkflowRunStep) =>
  invoke<{ id: string }>("workflow_save_run_step", { step });

export const deleteRun = (id: string) => invoke<{ ok: boolean }>("workflow_delete_run", { id });
