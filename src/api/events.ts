import { activeTransport, localTransport, type UnlistenFn } from "./transport";
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
import type {
  DictationLevelEvent,
  DictationModelProgressEvent,
  DictationStateEvent,
  DictationTranscriptEvent,
} from "./types/dictation";
import type { PrStateChangedEvent } from "./types/pr";
import type {
  AgentInstallEvent,
  ProviderLoginExitEvent,
  ProviderLoginOutputEvent,
} from "./types/providers";
import type {
  RoadmapBrief,
  RoadmapBriefProposal,
  RoadmapItem,
  RoadmapItemEvent,
  RoadmapOrderProposal,
  RoadmapProjectHold,
  RoadmapProposal,
  RoadmapQueueNote,
} from "./types/roadmap";
import type { RunOutputEvent, RunPortEvent, RunStateEvent } from "./types/run";
import type { DockerBuildEvent, PublishApproval, PublishApprovalResolved } from "./types/sandbox";
import type {
  SessionRecordsAppendedEvent,
  SessionSyncHealthEvent,
  TurnSentEvent,
  TurnStartedEvent,
} from "./types/session";
import type { VerificationReportEvent } from "./types/verify";
import type { WfEventEnvelope, WfRun } from "./types/workflow";

/** Subscribe to one engine event on the environment the UI is driving, with the
 *  transport's envelope already unwrapped, and get back the unsubscribe. */
function on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return activeTransport().on<T>(event, cb);
}

/** Subscribe on this desktop whatever environment is active — for events no
 *  remote engine has any business emitting: the mic, the provider CLIs
 *  installed here, a sign-in PTY running in this window. The mirror image of
 *  `invokeLocal` in ./invoke, and every wrapper below says which one it uses. */
function onLocal<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return localTransport.on<T>(event, cb);
}

/** Fires on every journal append for any run. */
export function onWfEvent(cb: (e: WfEventEnvelope) => void): Promise<UnlistenFn> {
  return on<WfEventEnvelope>("wf:event", cb);
}

/** Fires whenever a run row changes; carries the full row. */
export function onWfRun(cb: (e: WfRun) => void): Promise<UnlistenFn> {
  return on<WfRun>("wf:run", cb);
}

/** `wf:run-deleted` fires the deleted run's id after `wf_delete_run` removes its
 *  rows, so the sidebar drops the row instead of upserting it. */
export function onWfRunDeleted(cb: (runId: string) => void): Promise<UnlistenFn> {
  return on<string>("wf:run-deleted", cb);
}

/** Fires whenever a roadmap item is created or changed; carries the full row so
 *  the board upserts by id without a refetch. Fires for every project — a
 *  listener scoped to one board filters on `project_id`. */
export function onRoadmapItem(cb: (item: RoadmapItem) => void): Promise<UnlistenFn> {
  return on<RoadmapItem>("roadmap:item", cb);
}

/** `roadmap:item-deleted` fires the deleted item's id, so the board drops the
 *  row instead of upserting it. */
export function onRoadmapItemDeleted(cb: (id: string) => void): Promise<UnlistenFn> {
  return on<string>("roadmap:item-deleted", cb);
}

/** `roadmap:item-event` fires when a durable history row lands — one per status
 *  transition, carrying the full event. The board appends it to an expanded
 *  card's trail; anything missed is refetched on the next expand
 *  (`roadmap_list_item_events`), so a listener that wasn't mounted loses
 *  nothing. */
export function onRoadmapItemEvent(cb: (e: RoadmapItemEvent) => void): Promise<UnlistenFn> {
  return on<RoadmapItemEvent>("roadmap:item-event", cb);
}

/** `roadmap:proposal` fires when the PM parks (or revises — same id, new
 *  contents) a pending ask against an existing item; carries the full row so
 *  the card grows its proposal bar without a refetch. Fires for every project —
 *  a listener scoped to one board filters on `project_id`. */
export function onRoadmapProposal(cb: (proposal: RoadmapProposal) => void): Promise<UnlistenFn> {
  return on<RoadmapProposal>("roadmap:proposal", cb);
}

/** `roadmap:proposal-deleted` fires the proposal's id once it has been ruled on
 *  (accepted, declined, or found stale) — the item's own fate arrives
 *  separately on `roadmap:item` / `roadmap:item-deleted`. */
export function onRoadmapProposalDeleted(cb: (id: string) => void): Promise<UnlistenFn> {
  return on<string>("roadmap:proposal-deleted", cb);
}

/** `roadmap:order-proposal` fires when the PM parks (or replaces) a whole-board
 *  order ask; carries the full row so the board grows its order bar without a
 *  refetch. Fires for every project — a listener scoped to one board filters on
 *  `project_id`. */
export function onRoadmapOrderProposal(
  cb: (proposal: RoadmapOrderProposal) => void,
): Promise<UnlistenFn> {
  return on<RoadmapOrderProposal>("roadmap:order-proposal", cb);
}

/** `roadmap:order-proposal-deleted` fires the *project id* once the order ask has
 *  been ruled on (accepted, declined, or found stale) — the ask is keyed by
 *  board, not by row. The reordered rows arrive separately on `roadmap:item`. */
export function onRoadmapOrderProposalDeleted(
  cb: (projectId: string) => void,
): Promise<UnlistenFn> {
  return on<string>("roadmap:order-proposal-deleted", cb);
}

/** `roadmap:project-hold` fires when the whole board is stopped, or when the
 *  reason changes (one hold per project, replaced in place); carries the full row
 *  so the banner appears without a refetch. Fires for every project — a listener
 *  scoped to one board filters on `project_id`.
 *
 *  An *item's* hold has no event of its own: it lives on the row, so it arrives
 *  on `roadmap:item` like every other change to that row. */
export function onRoadmapProjectHold(cb: (hold: RoadmapProjectHold) => void): Promise<UnlistenFn> {
  return on<RoadmapProjectHold>("roadmap:project-hold", cb);
}

/** `roadmap:project-hold-released` fires the *project id* once the user lets the
 *  board run again — the hold is keyed by board, so there is nothing else to
 *  address it by. Only the user can produce this event: the PM has an op to hold
 *  and none to release. */
export function onRoadmapProjectHoldReleased(cb: (projectId: string) => void): Promise<UnlistenFn> {
  return on<string>("roadmap:project-hold-released", cb);
}

/** `roadmap:brief` fires when the project's product brief changes — which only
 *  happens when the user accepts a PM ask, since that ruling is its one writer.
 *  Carries the whole document, so the tab re-renders without a refetch. Fires for
 *  every project — a listener scoped to one board filters on `project_id`. */
export function onRoadmapBrief(cb: (brief: RoadmapBrief) => void): Promise<UnlistenFn> {
  return on<RoadmapBrief>("roadmap:brief", cb);
}

/** `roadmap:brief-proposal` fires when the PM parks (or replaces) an ask to
 *  rewrite the brief; carries the full row so the tab grows its decision bar
 *  mid-conversation. */
export function onRoadmapBriefProposal(
  cb: (proposal: RoadmapBriefProposal) => void,
): Promise<UnlistenFn> {
  return on<RoadmapBriefProposal>("roadmap:brief-proposal", cb);
}

/** `roadmap:brief-proposal-deleted` fires the *project id* once the brief ask has
 *  been ruled on either way — the ask is keyed by board, not by row. An accepted
 *  one is followed by `roadmap:brief` carrying the new document. */
export function onRoadmapBriefProposalDeleted(
  cb: (projectId: string) => void,
): Promise<UnlistenFn> {
  return on<string>("roadmap:brief-proposal-deleted", cb);
}

/** `roadmap:queue-note` explains why an item isn't moving — no workflow to run
 *  it under, a dependency that hasn't landed, a launch that failed, or a PR
 *  that was closed without merging (the merge sweep sends that one alongside
 *  the row's flip back to `open`). The board shows it inline on the row;
 *  nothing persists it, so a listener that wasn't mounted simply didn't hear it
 *  (the drainer repeats itself when the reason changes). */
export function onRoadmapQueueNote(cb: (note: RoadmapQueueNote) => void): Promise<UnlistenFn> {
  return on<RoadmapQueueNote>("roadmap:queue-note", cb);
}

export function onAgentInstallState(cb: (e: AgentInstallEvent) => void): Promise<UnlistenFn> {
  return onLocal<AgentInstallEvent>("agent-install:state", cb);
}

export function onAgentOutput(cb: (e: AgentOutputEvent) => void): Promise<UnlistenFn> {
  return on<AgentOutputEvent>("agent:output", cb);
}

export function onShellOutput(cb: (e: ShellOutputEvent) => void): Promise<UnlistenFn> {
  return on<ShellOutputEvent>("shell:output", cb);
}

/** The running transcript of the active dictation session (see
 *  `DictationTranscriptEvent` — whole text, not a delta). */
export function onDictationTranscript(
  cb: (e: DictationTranscriptEvent) => void,
): Promise<UnlistenFn> {
  return onLocal<DictationTranscriptEvent>("dictation:transcript", cb);
}

/** Dictation session lifecycle: listening → (transcribing) → stopped, or error
 *  with a reason.
 *  App-wide, like the session itself — every subscriber sees every session's
 *  events, so a consumer that owns one has to ignore the rest (a composer that
 *  unmounted mid-session leaves its terminal event to land on the next one). */
export function onDictationState(cb: (e: DictationStateEvent) => void): Promise<UnlistenFn> {
  return onLocal<DictationStateEvent>("dictation:state", cb);
}

/** The microphone's loudness while a dictation session listens, about every
 *  90 ms (see `DictationLevelEvent`). App-wide and session-stamped like the
 *  other dictation events. */
export function onDictationLevel(cb: (e: DictationLevelEvent) => void): Promise<UnlistenFn> {
  return onLocal<DictationLevelEvent>("dictation:level", cb);
}

/** Progress of the local engine's model download, ending in `installed` or
 *  `error`. The download outlives the Settings screen that started it, so a
 *  subscriber that mounts late reads `dictationModelStatus` for where it
 *  stands and then follows along here. */
export function onDictationModelProgress(
  cb: (e: DictationModelProgressEvent) => void,
): Promise<UnlistenFn> {
  return onLocal<DictationModelProgressEvent>("dictation:model_progress", cb);
}

export function onAgentEvent(cb: (e: AgentManagedEvent) => void): Promise<UnlistenFn> {
  return on<AgentManagedEvent>("agent:event", cb);
}

/** Fires when a turn's transcript has been ingested into session_records, so
 *  the canonical render can replace the ephemeral live one. */
export function onSessionRecordsAppended(
  cb: (e: SessionRecordsAppendedEvent) => void,
): Promise<UnlistenFn> {
  return on<SessionRecordsAppendedEvent>("session:records-appended", cb);
}

/** Fires when an agent's turn-end transcript ingest changes health — drift
 *  detected, or a prior drift cleared. Emitted on change only. */
export function onSessionSyncHealth(cb: (e: SessionSyncHealthEvent) => void): Promise<UnlistenFn> {
  return on<SessionSyncHealthEvent>("session:sync-health", cb);
}

/** Fires when the host accepts a user message for an agent, from any client.
 *  Mirror it into the log unless it is this client's own send (same turn id). */
export function onTurnSent(cb: (e: TurnSentEvent) => void): Promise<UnlistenFn> {
  return on<TurnSentEvent>("turn:sent", cb);
}

/** Fires when a turn flips to Running, carrying the backend's own start
 *  timestamp so the live timer shares the persisted duration's clock. */
export function onTurnStarted(cb: (e: TurnStartedEvent) => void): Promise<UnlistenFn> {
  return on<TurnStartedEvent>("turn:started", cb);
}

export function onAgentStatus(cb: (e: AgentStatusEvent) => void): Promise<UnlistenFn> {
  return on<AgentStatusEvent>("agent:status", cb);
}

export function onAgentView(cb: (e: AgentViewEvent) => void): Promise<UnlistenFn> {
  return on<AgentViewEvent>("agent:view", cb);
}

/** Fires when a session's reasoning effort is changed mid-conversation, so the
 *  composer's effort chip reflects the new value without a full resync. */
export function onAgentEffort(cb: (e: AgentEffortEvent) => void): Promise<UnlistenFn> {
  return on<AgentEffortEvent>("agent:effort", cb);
}

/** Fires when a session's model is changed mid-conversation, so the composer's
 *  model picker reflects the new value without a full resync. */
export function onAgentModel(cb: (e: AgentModelEvent) => void): Promise<UnlistenFn> {
  return on<AgentModelEvent>("agent:model", cb);
}

export function onAgentTask(cb: (e: AgentTaskEvent) => void): Promise<UnlistenFn> {
  return on<AgentTaskEvent>("agent:task", cb);
}

export function onAgentBranch(cb: (e: AgentBranchEvent) => void): Promise<UnlistenFn> {
  return on<AgentBranchEvent>("agent:branch", cb);
}

export function onAgentRepoAdded(cb: (e: AgentRepoAddedEvent) => void): Promise<UnlistenFn> {
  return on<AgentRepoAddedEvent>("agent:repo_added", cb);
}

export function onAgentGitAction(cb: (e: AgentGitActionEvent) => void): Promise<UnlistenFn> {
  return on<AgentGitActionEvent>("agent:git-action", cb);
}

export function onWorkspaceChanged(cb: () => void): Promise<UnlistenFn> {
  return on<unknown>("workspace:changed", () => cb());
}

export function onPrStateChanged(cb: (e: PrStateChangedEvent) => void): Promise<UnlistenFn> {
  return on<PrStateChangedEvent>("pr:state_changed", cb);
}

export function onVerificationReport(
  cb: (e: VerificationReportEvent) => void,
): Promise<UnlistenFn> {
  return on<VerificationReportEvent>("verify:report", cb);
}

export function onRunOutput(cb: (e: RunOutputEvent) => void): Promise<UnlistenFn> {
  return on<RunOutputEvent>("run:output", cb);
}

export function onRunState(cb: (e: RunStateEvent) => void): Promise<UnlistenFn> {
  return on<RunStateEvent>("run:state", cb);
}

export function onRunPort(cb: (e: RunPortEvent) => void): Promise<UnlistenFn> {
  return on<RunPortEvent>("run:port", cb);
}

/** An agent is waiting for the user to approve one publish. Only fires when the
 *  `publish_confirmation` setting is on; unanswered requests are denied backend
 *  side after a timeout, so ignoring one is safe. */
export function onPublishApprovalRequested(cb: (e: PublishApproval) => void): Promise<UnlistenFn> {
  return on<PublishApproval>("publish:approval-requested", cb);
}

/** One held publish is no longer held — answered here, answered on another
 *  device, answered from the host's CLI, or refused because nobody answered in
 *  time. The prompt for `id` comes down; the verdict itself has already reached
 *  the agent. A host that never emits it leaves the old behaviour intact: the
 *  prompt stays until it is answered. */
export function onPublishApprovalResolved(
  cb: (e: PublishApprovalResolved) => void,
): Promise<UnlistenFn> {
  return on<PublishApprovalResolved>("publish:approval-resolved", cb);
}

/** Fires per line (and at start/finish/failure) while the embedded docker agent
 *  image builds on a cold first spawn — feeds the build progress toast. */
export function onDockerBuildProgress(cb: (e: DockerBuildEvent) => void): Promise<UnlistenFn> {
  return on<DockerBuildEvent>("docker:build-progress", cb);
}

/** Raw PTY bytes from a provider's in-app sign-in (Settings → Providers). */
export function onProviderLoginOutput(
  cb: (e: ProviderLoginOutputEvent) => void,
): Promise<UnlistenFn> {
  return onLocal<ProviderLoginOutputEvent>("provider-login:output", cb);
}

/** A provider's sign-in process ended — cleanly, with an error, or because it
 *  was closed. */
export function onProviderLoginExit(cb: (e: ProviderLoginExitEvent) => void): Promise<UnlistenFn> {
  return onLocal<ProviderLoginExitEvent>("provider-login:exit", cb);
}
