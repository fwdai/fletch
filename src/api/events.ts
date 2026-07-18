import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentBranchEvent,
  AgentGitActionEvent,
  AgentManagedEvent,
  AgentOutputEvent,
  AgentRepoAddedEvent,
  AgentStatusEvent,
  AgentTaskEvent,
  AgentViewEvent,
  ShellOutputEvent,
} from "./types/agent";
import type { PrStateChangedEvent } from "./types/pr";
import type { AgentInstallEvent } from "./types/providers";
import type { RunOutputEvent, RunPortEvent, RunStateEvent } from "./types/run";
import type { DockerBuildEvent } from "./types/sandbox";
import type {
  SessionRecordsAppendedEvent,
  SessionSyncHealthEvent,
  TurnStartedEvent,
} from "./types/session";
import type { VerificationReportEvent } from "./types/verify";
import type { WfEventEnvelope, WfRun } from "./types/workflow";

/** Fires on every journal append for any run. */
export function onWfEvent(cb: (e: WfEventEnvelope) => void): Promise<UnlistenFn> {
  return listen<WfEventEnvelope>("wf:event", (event) => cb(event.payload));
}

/** Fires whenever a run row changes; carries the full row. */
export function onWfRun(cb: (e: WfRun) => void): Promise<UnlistenFn> {
  return listen<WfRun>("wf:run", (event) => cb(event.payload));
}

/** `wf:run-deleted` fires the deleted run's id after `wf_delete_run` removes its
 *  rows, so the sidebar drops the row instead of upserting it. */
export function onWfRunDeleted(cb: (runId: string) => void): Promise<UnlistenFn> {
  return listen<string>("wf:run-deleted", (event) => cb(event.payload));
}

export function onAgentInstallState(cb: (e: AgentInstallEvent) => void): Promise<UnlistenFn> {
  return listen<AgentInstallEvent>("agent-install:state", (event) => cb(event.payload));
}

export function onAgentOutput(cb: (e: AgentOutputEvent) => void): Promise<UnlistenFn> {
  return listen<AgentOutputEvent>("agent:output", (event) => cb(event.payload));
}

export function onShellOutput(cb: (e: ShellOutputEvent) => void): Promise<UnlistenFn> {
  return listen<ShellOutputEvent>("shell:output", (event) => cb(event.payload));
}

export function onAgentEvent(cb: (e: AgentManagedEvent) => void): Promise<UnlistenFn> {
  return listen<AgentManagedEvent>("agent:event", (event) => cb(event.payload));
}

/** Fires when a turn's transcript has been ingested into session_records, so
 *  the canonical render can replace the ephemeral live one. */
export function onSessionRecordsAppended(
  cb: (e: SessionRecordsAppendedEvent) => void,
): Promise<UnlistenFn> {
  return listen<SessionRecordsAppendedEvent>("session:records-appended", (event) =>
    cb(event.payload),
  );
}

/** Fires when an agent's turn-end transcript ingest changes health — drift
 *  detected, or a prior drift cleared. Emitted on change only. */
export function onSessionSyncHealth(cb: (e: SessionSyncHealthEvent) => void): Promise<UnlistenFn> {
  return listen<SessionSyncHealthEvent>("session:sync-health", (event) => cb(event.payload));
}

/** Fires when a turn flips to Running, carrying the backend's own start
 *  timestamp so the live timer shares the persisted duration's clock. */
export function onTurnStarted(cb: (e: TurnStartedEvent) => void): Promise<UnlistenFn> {
  return listen<TurnStartedEvent>("turn:started", (event) => cb(event.payload));
}

export function onAgentStatus(cb: (e: AgentStatusEvent) => void): Promise<UnlistenFn> {
  return listen<AgentStatusEvent>("agent:status", (event) => cb(event.payload));
}

export function onAgentView(cb: (e: AgentViewEvent) => void): Promise<UnlistenFn> {
  return listen<AgentViewEvent>("agent:view", (event) => cb(event.payload));
}

export function onAgentTask(cb: (e: AgentTaskEvent) => void): Promise<UnlistenFn> {
  return listen<AgentTaskEvent>("agent:task", (event) => cb(event.payload));
}

export function onAgentBranch(cb: (e: AgentBranchEvent) => void): Promise<UnlistenFn> {
  return listen<AgentBranchEvent>("agent:branch", (event) => cb(event.payload));
}

export function onAgentRepoAdded(cb: (e: AgentRepoAddedEvent) => void): Promise<UnlistenFn> {
  return listen<AgentRepoAddedEvent>("agent:repo_added", (event) => cb(event.payload));
}

export function onAgentGitAction(cb: (e: AgentGitActionEvent) => void): Promise<UnlistenFn> {
  return listen<AgentGitActionEvent>("agent:git-action", (event) => cb(event.payload));
}

export function onWorkspaceChanged(cb: () => void): Promise<UnlistenFn> {
  return listen<unknown>("workspace:changed", () => cb());
}

export function onPrStateChanged(cb: (e: PrStateChangedEvent) => void): Promise<UnlistenFn> {
  return listen<PrStateChangedEvent>("pr:state_changed", (event) => cb(event.payload));
}

export function onVerificationReport(
  cb: (e: VerificationReportEvent) => void,
): Promise<UnlistenFn> {
  return listen<VerificationReportEvent>("verify:report", (event) => cb(event.payload));
}

export function onRunOutput(cb: (e: RunOutputEvent) => void): Promise<UnlistenFn> {
  return listen<RunOutputEvent>("run:output", (event) => cb(event.payload));
}

export function onRunState(cb: (e: RunStateEvent) => void): Promise<UnlistenFn> {
  return listen<RunStateEvent>("run:state", (event) => cb(event.payload));
}

export function onRunPort(cb: (e: RunPortEvent) => void): Promise<UnlistenFn> {
  return listen<RunPortEvent>("run:port", (event) => cb(event.payload));
}

/** Fires per line (and at start/finish/failure) while the embedded docker agent
 *  image builds on a cold first spawn — feeds the build progress toast. */
export function onDockerBuildProgress(cb: (e: DockerBuildEvent) => void): Promise<UnlistenFn> {
  return listen<DockerBuildEvent>("docker:build-progress", (event) => cb(event.payload));
}
