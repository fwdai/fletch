// run/projectPipeline.ts — the project's default workflow, persisted in the
// host's `project_settings` (same store as run-config overrides) under
// `workflow.default`, through the host's project-settings ops: the definition
// id the workflow composer preselects, and the one the roadmap queue runs.
// Failures are logged, not thrown: a missing/unreadable setting degrades to the
// built-in default (the first workflow).

import { api } from "@/api";

const DEFAULT_WORKFLOW_KEY = "workflow.default";

export async function loadDefaultWorkflowId(projectId: string): Promise<string | null> {
  if (!projectId) return null;
  try {
    const all = await api.getProjectSettings(projectId);
    return all[DEFAULT_WORKFLOW_KEY] || null;
  } catch (e) {
    console.error("loadDefaultWorkflowId failed", e);
    return null;
  }
}

export function rememberDefaultWorkflow(projectId: string, definitionId: string): void {
  if (!projectId || !definitionId) return;
  api
    .setProjectSetting(projectId, DEFAULT_WORKFLOW_KEY, definitionId)
    .catch((e) => console.error("rememberDefaultWorkflow failed", e));
}
