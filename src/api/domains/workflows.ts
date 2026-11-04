import type { Definition, ImportReport, Spec } from "@/workflows/spec";
import { invoke } from "../invoke";
import type { AgentRecord } from "../types/agent";
import type { WfEvent, WfRun, WfRunDetail } from "../types/workflow";

export const workflowsApi = {
  // ── Workflows v1 (read-only surface; scheduler slices populate the data) ──
  /** Runs newest-updated first, optionally scoped to one project. */
  wfListRuns: (projectId?: string) => invoke<WfRun[]>("wf_list_runs", { projectId }),
  /** A run plus its attempts; null if the run doesn't exist. */
  wfGetRun: (runId: string) => invoke<WfRunDetail | null>("wf_get_run", { runId }),
  /** A page of a run's journal: events strictly after `afterSeq`, oldest first. */
  wfEvents: (runId: string, afterSeq: number, limit: number) =>
    invoke<WfEvent[]>("wf_events", { runId, afterSeq, limit }),
  /** A run's step agents (live + archived). Run-owned agents are hidden from
   *  `get_workspace`, so the monitor fetches them here to render attempt chats. */
  wfRunAgents: (runId: string) => invoke<AgentRecord[]>("wf_run_agents", { runId }),

  // ── Workflows v1: run control (spec §13; registered by the scheduler, S4) ──
  /** Launch a run from a launch-time `spec` snapshot; returns the new run id.
   *  Pass `definitionId` when launching a stored definition (bumps run_count);
   *  `baseBranch` overrides the branch step 1 forks from. */
  wfLaunch: (
    spec: Spec,
    task: string,
    projectId: string,
    repoPath: string,
    definitionId?: string,
    baseBranch?: string,
    /** Absolute paths of files to attach to the run's first prompt, like a chat
     *  message's attachments. Empty by default. */
    attachments: string[] = [],
    /** GitHub issue number (as text) this run was started from. The finalized
     *  PR closes it (backend appends `Closes #N`). Undefined for a normal
     *  launch. */
    issueRef?: string,
  ) =>
    invoke<string>("wf_launch", {
      spec,
      task,
      projectId,
      repoPath,
      definitionId,
      baseBranch,
      attachments,
      issueRef,
    }),
  /** Cancel a run: stops the live attempt's agent and marks the run canceled. */
  wfCancel: (runId: string) => invoke<void>("wf_cancel", { runId }),
  /** Delete a terminal run and everything it owns (§13): its run-owned step
   *  agents (and their chats), `~/.fletch/runs/<id>/`, and its rows. Rejected
   *  while the run is active. */
  wfDeleteRun: (runId: string) => invoke<void>("wf_delete_run", { runId }),

  // ── Workflows v1: definition storage (spec §13, `wf_def_*`) ──
  /** Validate and persist a workflow definition. Omit `id` to create; pass an
   *  existing id to edit in place (run_count/created_at are preserved). Rejects
   *  with the joined §5.2 validation errors if the spec is invalid. */
  wfDefSave: (spec: Spec, id?: string, hue?: number) =>
    invoke<Definition>("wf_def_save", { spec, id, hue }),
  /** Every stored definition, newest-edited first. */
  wfDefList: () => invoke<Definition[]>("wf_def_list"),
  /** Delete a definition; in-flight runs keep their own launch snapshot. */
  wfDefDelete: (id: string) => invoke<void>("wf_def_delete", { id }),
  /** Serialize a definition to portable YAML (custom-agent specs embedded). */
  wfDefExportYaml: (id: string) => invoke<string>("wf_def_export_yaml", { id }),
  /** Parse + validate a YAML file and resolve it against the local library.
   *  Missing skills / unknown providers come back as warnings, not errors. */
  wfDefImportYaml: (yamlText: string) => invoke<ImportReport>("wf_def_import_yaml", { yamlText }),
};
