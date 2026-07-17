// run/projectPipeline.ts — per-project composer preferences, persisted in the
// `project_settings` table (same store as run-config overrides). Two keys:
//   composer.mode     — "agent" | "workflow": the kickoff mode the composer
//                        opens in for this project (a remembered suggestion,
//                        never a gate).
//   workflow.default  — the definition id the Pipeline side preselects.
// Failures are logged, not thrown: a missing/unreadable setting degrades to the
// built-in default (quick agent, first workflow).

import { getProjectSettings, setProjectSetting } from "@/storage/projectSettings";

export type ComposerMode = "agent" | "workflow";

const MODE_KEY = "composer.mode";
const DEFAULT_WORKFLOW_KEY = "workflow.default";

export interface PipelinePrefs {
  mode: ComposerMode;
  defaultWorkflowId: string | null;
}

export async function loadPipelinePrefs(projectId: string): Promise<PipelinePrefs> {
  if (!projectId) return { mode: "agent", defaultWorkflowId: null };
  try {
    const all = await getProjectSettings(projectId);
    const mode = all[MODE_KEY] === "workflow" ? "workflow" : "agent";
    return { mode, defaultWorkflowId: all[DEFAULT_WORKFLOW_KEY] || null };
  } catch (e) {
    console.error("loadPipelinePrefs failed", e);
    return { mode: "agent", defaultWorkflowId: null };
  }
}

export function rememberComposerMode(projectId: string, mode: ComposerMode): void {
  if (!projectId) return;
  setProjectSetting(projectId, MODE_KEY, mode).catch((e) =>
    console.error("rememberComposerMode failed", e),
  );
}

export function rememberDefaultWorkflow(projectId: string, definitionId: string): void {
  if (!projectId || !definitionId) return;
  setProjectSetting(projectId, DEFAULT_WORKFLOW_KEY, definitionId).catch((e) =>
    console.error("rememberDefaultWorkflow failed", e),
  );
}
