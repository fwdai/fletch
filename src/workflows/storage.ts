// storage.ts — typed access to the workflow-definition backend commands
// (workflow_list / workflow_save / workflow_delete in src-tauri/src/workflows.rs).

import { invoke } from "@tauri-apps/api/core";

/** How a step decides it's done and hands off to the next. */
export const ADVANCE_IDS = ["signal", "commit", "tests", "artifact", "approval"] as const;
export type AdvanceMode = (typeof ADVANCE_IDS)[number];

/** A loop-back edge: this step can return to an earlier one until `when` clears
 *  (or `max` iterations are reached). */
export interface WorkflowStepLoop {
  to: string;
  when: string;
  max: number;
}

export interface WorkflowStep {
  id: string;
  /** A custom-agent id OR a base-provider id (e.g. "claude"). null = unassigned. */
  agent: string | null;
  goal: string;
  advance: AdvanceMode;
  /** Named file for the `artifact` advance mode (e.g. "PLAN.md"). */
  artifact?: string;
  loop?: WorkflowStepLoop | null;
}

export interface Workflow {
  id: string;
  name: string;
  description: string;
  hue: number;
  steps: WorkflowStep[];
  run_count: number;
  created_at: number;
  updated_at: number;
}

/** A workflow as edited in the builder, before persistence stamps run_count and
 *  timestamps. */
export type WorkflowDraft = Pick<Workflow, "id" | "name" | "description" | "hue" | "steps">;

export const listWorkflows = () => invoke<Workflow[]>("workflow_list");
export const saveWorkflow = (workflow: WorkflowDraft) =>
  invoke<{ id: string }>("workflow_save", { workflow });
export const deleteWorkflow = (id: string) => invoke<{ ok: boolean }>("workflow_delete", { id });

/** Strip persistence-managed fields and deep-clone steps for safe editing. */
export function toDraft(w: Workflow): WorkflowDraft {
  return {
    id: w.id,
    name: w.name,
    description: w.description,
    hue: w.hue,
    steps: JSON.parse(JSON.stringify(w.steps)),
  };
}
