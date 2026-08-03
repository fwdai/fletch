import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentBranchEvent,
  AgentEffortEvent,
  AgentGitActionEvent,
  AgentManagedEvent,
  AgentModelEvent,
  AgentOutputEvent,
  AgentRepoAddedEvent,
  AgentStatusEvent,
  AgentTaskEvent,
  AgentViewEvent,
  ShellOutputEvent,
} from "./types/agent";
import type { PrStateChangedEvent } from "./types/pr";
import type { AgentInstallEvent } from "./types/providers";
import type {
  RoadmapItem,
  RoadmapItemEvent,
  RoadmapOrderProposal,
  RoadmapProposal,
  RoadmapQueueNote,
} from "./types/roadmap";
import type { RunOutputEvent, RunPortEvent, RunStateEvent } from "./types/run";
import type { DockerBuildEvent, PublishApproval } from "./types/sandbox";
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

/** Fires whenever a roadmap item is created or changed; carries the full row so
 *  the board upserts by id without a refetch. Fires for every project — a
 *  listener scoped to one board filters on `project_id`. */
export function onRoadmapItem(cb: (item: RoadmapItem) => void): Promise<UnlistenFn> {
  return listen<RoadmapItem>("roadmap:item", (event) => cb(event.payload));
}

/** `roadmap:item-deleted` fires the deleted item's id, so the board drops the
 *  row instead of upserting it. */
export function onRoadmapItemDeleted(cb: (id: string) => void): Promise<UnlistenFn> {
  return listen<string>("roadmap:item-deleted", (event) => cb(event.payload));
}

/** `roadmap:item-event` fires when a durable history row lands — one per status
 *  transition, carrying the full event. The board appends it to an expanded
 *  card's trail; anything missed is refetched on the next expand
 *  (`roadmap_list_item_events`), so a listener that wasn't mounted loses
 *  nothing. */
export function onRoadmapItemEvent(cb: (e: RoadmapItemEvent) => void): Promise<UnlistenFn> {
  return listen<RoadmapItemEvent>("roadmap:item-event", (event) => cb(event.payload));
}

/** `roadmap:proposal` fires when the PM parks (or revises — same id, new
 *  contents) a pending ask against an existing item; carries the full row so
 *  the card grows its proposal bar without a refetch. Fires for every project —
 *  a listener scoped to one board filters on `project_id`. */
export function onRoadmapProposal(cb: (proposal: RoadmapProposal) => void): Promise<UnlistenFn> {
  return listen<RoadmapProposal>("roadmap:proposal", (event) => cb(event.payload));
}

/** `roadmap:proposal-deleted` fires the proposal's id once it has been ruled on
 *  (accepted, declined, or found stale) — the item's own fate arrives
 *  separately on `roadmap:item` / `roadmap:item-deleted`. */
export function onRoadmapProposalDeleted(cb: (id: string) => void): Promise<UnlistenFn> {
  return listen<string>("roadmap:proposal-deleted", (event) => cb(event.payload));
}

/** `roadmap:order-proposal` fires when the PM parks (or replaces) a whole-board
 *  order ask; carries the full row so the board grows its order bar without a
 *  refetch. Fires for every project — a listener scoped to one board filters on
 *  `project_id`. */
export function onRoadmapOrderProposal(
  cb: (proposal: RoadmapOrderProposal) => void,
): Promise<UnlistenFn> {
  return listen<RoadmapOrderProposal>("roadmap:order-proposal", (event) => cb(event.payload));
}

/** `roadmap:order-proposal-deleted` fires the *project id* once the order ask has
 *  been ruled on (accepted, declined, or found stale) — the ask is keyed by
 *  board, not by row. The reordered rows arrive separately on `roadmap:item`. */
export function onRoadmapOrderProposalDeleted(
  cb: (projectId: string) => void,
): Promise<UnlistenFn> {
  return listen<string>("roadmap:order-proposal-deleted", (event) => cb(event.payload));
}

/** `roadmap:queue-note` explains why an item isn't moving — no workflow to run
 *  it under, a dependency that hasn't landed, a launch that failed, or a PR
 *  that was closed without merging (the merge sweep sends that one alongside
 *  the row's flip back to `open`). The board shows it inline on the row;
 *  nothing persists it, so a listener that wasn't mounted simply didn't hear it
 *  (the drainer repeats itself when the reason changes). */
export function onRoadmapQueueNote(cb: (note: RoadmapQueueNote) => void): Promise<UnlistenFn> {
  return listen<RoadmapQueueNote>("roadmap:queue-note", (event) => cb(event.payload));
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

/** Fires when a session's reasoning effort is changed mid-conversation, so the
 *  composer's effort chip reflects the new value without a full resync. */
export function onAgentEffort(cb: (e: AgentEffortEvent) => void): Promise<UnlistenFn> {
  return listen<AgentEffortEvent>("agent:effort", (event) => cb(event.payload));
}

/** Fires when a session's model is changed mid-conversation, so the composer's
 *  model picker reflects the new value without a full resync. */
export function onAgentModel(cb: (e: AgentModelEvent) => void): Promise<UnlistenFn> {
  return listen<AgentModelEvent>("agent:model", (event) => cb(event.payload));
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

/** An agent is waiting for the user to approve one publish. Only fires when the
 *  `publish_confirmation` setting is on; unanswered requests are denied backend
 *  side after a timeout, so ignoring one is safe. */
export function onPublishApprovalRequested(cb: (e: PublishApproval) => void): Promise<UnlistenFn> {
  return listen<PublishApproval>("publish:approval-requested", (event) => cb(event.payload));
}

/** Fires per line (and at start/finish/failure) while the embedded docker agent
 *  image builds on a cold first spawn — feeds the build progress toast. */
export function onDockerBuildProgress(cb: (e: DockerBuildEvent) => void): Promise<UnlistenFn> {
  return listen<DockerBuildEvent>("docker:build-progress", (event) => cb(event.payload));
}
