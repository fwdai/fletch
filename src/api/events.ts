import { localTransport, type UnlistenFn } from "./transport";
import type {
  AgentBranchEvent,
  AgentEffortEvent,
  AgentFolderGrantsEvent,
  AgentManagedEvent,
  AgentModelEvent,
  AgentNotifyEvent,
  AgentOutputEvent,
  AgentRepoAddedEvent,
  AgentSpawnProgressEvent,
  AgentStatusEvent,
  AgentTaskEvent,
  AgentTitleEvent,
  AgentViewEvent,
  ShellOutputEvent,
  WorkspaceAutoArchivedEvent,
} from "./types/agent";
import type { ContextChangedEvent } from "./types/context";
import type {
  DictationLevelEvent,
  DictationModelProgressEvent,
  DictationStateEvent,
  DictationTranscriptEvent,
} from "./types/dictation";
import type {
  AutopilotCheckout,
  AutopilotLogEntry,
  AutopilotSwitches,
  DelegationEvent,
} from "./types/git";
import type {
  PrChecksChangedEvent,
  PrSetEntryChangedEvent,
  PrStateChangedEvent,
  PrThreadsChangedEvent,
} from "./types/pr";
import type {
  AgentInstallEvent,
  ProviderLoginExitEvent,
  ProviderLoginOutputEvent,
} from "./types/providers";
import type { InviteOffer, PairRequest } from "./types/remote";
import type { ReviewState } from "./types/review";
import type {
  RoadmapItem,
  RoadmapItemEvent,
  RoadmapOrderProposal,
  RoadmapProjectHold,
  RoadmapProposal,
  RoadmapQueueNote,
} from "./types/roadmap";
import type { RunOutputEvent, RunPortEvent, RunStateEvent } from "./types/run";
import type {
  BoxEgressEvent,
  BoxImageStatus,
  BoxPolicyEvent,
  BoxStateEvent,
  BoxToolsStatus,
  EgressPrompt,
  EgressPromptResolved,
  PublishApproval,
  PublishApprovalResolved,
} from "./types/sandbox";
import type {
  SessionRecordsAppendedEvent,
  SessionSyncHealthEvent,
  TurnSentEvent,
  TurnStartedEvent,
} from "./types/session";
import type {
  ProjectFileChangedEvent,
  ProjectSettingsChangedEvent,
  SettingsChangedEvent,
} from "./types/settings";
import type { TourChangedEvent } from "./types/tour";
import type { WfEventEnvelope, WfRun } from "./types/workflow";

/** Subscribe to one engine event, with the transport's envelope already
 *  unwrapped, and get back the unsubscribe. */
export type Subscribe = <T>(event: string, cb: (payload: T) => void) => Promise<UnlistenFn>;

/** Every event wrapper, over two ways of subscribing. `on` reaches the engine
 *  these events belong to. `onLocal` reaches this desktop whatever engine that
 *  is — for events no remote engine has any business emitting: the mic, the
 *  provider CLIs installed here, a sign-in PTY running in this window. The
 *  mirror image of `invokeLocal` in ./invoke, and every wrapper below says
 *  which one it uses. */
export function makeEvents(on: Subscribe, onLocal: Subscribe) {
  return {
    /** Fires on every journal append for any run. */
    onWfEvent: (cb: (e: WfEventEnvelope) => void): Promise<UnlistenFn> =>
      on<WfEventEnvelope>("wf:event", cb),

    /** Fires whenever a run row changes; carries the full row. */
    onWfRun: (cb: (e: WfRun) => void): Promise<UnlistenFn> => on<WfRun>("wf:run", cb),

    /** `wf:run-deleted` fires the deleted run's id after `wf_delete_run` removes its
     *  rows, so the sidebar drops the row instead of upserting it. */
    onWfRunDeleted: (cb: (runId: string) => void): Promise<UnlistenFn> =>
      on<string>("wf:run-deleted", cb),

    /** Fires whenever a roadmap item is created or changed; carries the full row so
     *  the board upserts by id without a refetch. Fires for every project — a
     *  listener scoped to one board filters on `project_id`. */
    onRoadmapItem: (cb: (item: RoadmapItem) => void): Promise<UnlistenFn> =>
      on<RoadmapItem>("roadmap:item", cb),

    /** `roadmap:item-deleted` fires the deleted item's id, so the board drops the
     *  row instead of upserting it. */
    onRoadmapItemDeleted: (cb: (id: string) => void): Promise<UnlistenFn> =>
      on<string>("roadmap:item-deleted", cb),

    /** `roadmap:item-event` fires when a durable history row lands — one per status
     *  transition, carrying the full event. The board appends it to an expanded
     *  card's trail; anything missed is refetched on the next expand
     *  (`roadmap_list_item_events`), so a listener that wasn't mounted loses
     *  nothing. */
    onRoadmapItemEvent: (cb: (e: RoadmapItemEvent) => void): Promise<UnlistenFn> =>
      on<RoadmapItemEvent>("roadmap:item-event", cb),

    /** `roadmap:proposal` fires when the PM parks (or revises — same id, new
     *  contents) a pending ask against an existing item; carries the full row so
     *  the card grows its proposal bar without a refetch. Fires for every project —
     *  a listener scoped to one board filters on `project_id`. */
    onRoadmapProposal: (cb: (proposal: RoadmapProposal) => void): Promise<UnlistenFn> =>
      on<RoadmapProposal>("roadmap:proposal", cb),

    /** `roadmap:proposal-deleted` fires the proposal's id once it has been ruled on
     *  (accepted, declined, or found stale) — the item's own fate arrives
     *  separately on `roadmap:item` / `roadmap:item-deleted`. */
    onRoadmapProposalDeleted: (cb: (id: string) => void): Promise<UnlistenFn> =>
      on<string>("roadmap:proposal-deleted", cb),

    /** `roadmap:order-proposal` fires when the PM parks (or replaces) a whole-board
     *  order ask; carries the full row so the board grows its order bar without a
     *  refetch. Fires for every project — a listener scoped to one board filters on
     *  `project_id`. */
    onRoadmapOrderProposal: (cb: (proposal: RoadmapOrderProposal) => void): Promise<UnlistenFn> =>
      on<RoadmapOrderProposal>("roadmap:order-proposal", cb),

    /** `roadmap:order-proposal-deleted` fires the *project id* once the order ask has
     *  been ruled on (accepted, declined, or found stale) — the ask is keyed by
     *  board, not by row. The reordered rows arrive separately on `roadmap:item`. */
    onRoadmapOrderProposalDeleted: (cb: (projectId: string) => void): Promise<UnlistenFn> =>
      on<string>("roadmap:order-proposal-deleted", cb),

    /** `roadmap:project-hold` fires when the whole board is stopped, or when the
     *  reason changes (one hold per project, replaced in place); carries the full row
     *  so the banner appears without a refetch. Fires for every project — a listener
     *  scoped to one board filters on `project_id`.
     *
     *  An *item's* hold has no event of its own: it lives on the row, so it arrives
     *  on `roadmap:item` like every other change to that row. */
    onRoadmapProjectHold: (cb: (hold: RoadmapProjectHold) => void): Promise<UnlistenFn> =>
      on<RoadmapProjectHold>("roadmap:project-hold", cb),

    /** `roadmap:project-hold-released` fires the *project id* once the user lets the
     *  board run again — the hold is keyed by board, so there is nothing else to
     *  address it by. Only the user can produce this event: the PM has an op to hold
     *  and none to release. */
    onRoadmapProjectHoldReleased: (cb: (projectId: string) => void): Promise<UnlistenFn> =>
      on<string>("roadmap:project-hold-released", cb),

    /** `roadmap:queue-note` explains why an item isn't moving — no workflow to run
     *  it under, a dependency that hasn't landed, a launch that failed, or a PR
     *  that was closed without merging (the merge sweep sends that one alongside
     *  the row's flip back to `open`). The board shows it inline on the row;
     *  nothing persists it, so a listener that wasn't mounted simply didn't hear it
     *  (the drainer repeats itself when the reason changes). */
    onRoadmapQueueNote: (cb: (note: RoadmapQueueNote) => void): Promise<UnlistenFn> =>
      on<RoadmapQueueNote>("roadmap:queue-note", cb),

    onAgentInstallState: (cb: (e: AgentInstallEvent) => void): Promise<UnlistenFn> =>
      onLocal<AgentInstallEvent>("agent-install:state", cb),

    onAgentOutput: (cb: (e: AgentOutputEvent) => void): Promise<UnlistenFn> =>
      on<AgentOutputEvent>("agent:output", cb),

    onShellOutput: (cb: (e: ShellOutputEvent) => void): Promise<UnlistenFn> =>
      on<ShellOutputEvent>("shell:output", cb),

    /** The running transcript of the active dictation session (see
     *  `DictationTranscriptEvent` — whole text, not a delta). */
    onDictationTranscript: (cb: (e: DictationTranscriptEvent) => void): Promise<UnlistenFn> =>
      onLocal<DictationTranscriptEvent>("dictation:transcript", cb),

    /** Dictation session lifecycle: listening → (transcribing) → stopped, or error
     *  with a reason.
     *  App-wide, like the session itself — every subscriber sees every session's
     *  events, so a consumer that owns one has to ignore the rest (a composer that
     *  unmounted mid-session leaves its terminal event to land on the next one). */
    onDictationState: (cb: (e: DictationStateEvent) => void): Promise<UnlistenFn> =>
      onLocal<DictationStateEvent>("dictation:state", cb),

    /** The microphone's loudness while a dictation session listens, about every
     *  90 ms (see `DictationLevelEvent`). App-wide and session-stamped like the
     *  other dictation events. */
    onDictationLevel: (cb: (e: DictationLevelEvent) => void): Promise<UnlistenFn> =>
      onLocal<DictationLevelEvent>("dictation:level", cb),

    /** Progress of the local engine's model download, ending in `installed` or
     *  `error`. The download outlives the Settings screen that started it, so a
     *  subscriber that mounts late reads `dictationModelStatus` for where it
     *  stands and then follows along here. */
    onDictationModelProgress: (cb: (e: DictationModelProgressEvent) => void): Promise<UnlistenFn> =>
      onLocal<DictationModelProgressEvent>("dictation:model_progress", cb),

    onAgentEvent: (cb: (e: AgentManagedEvent) => void): Promise<UnlistenFn> =>
      on<AgentManagedEvent>("agent:event", cb),

    /** Fires when a turn's transcript has been ingested into session_records, so
     *  the canonical render can replace the ephemeral live one. */
    onSessionRecordsAppended: (cb: (e: SessionRecordsAppendedEvent) => void): Promise<UnlistenFn> =>
      on<SessionRecordsAppendedEvent>("session:records-appended", cb),

    /** Fires when an agent's turn-end transcript ingest changes health — drift
     *  detected, or a prior drift cleared. Emitted on change only. */
    onSessionSyncHealth: (cb: (e: SessionSyncHealthEvent) => void): Promise<UnlistenFn> =>
      on<SessionSyncHealthEvent>("session:sync-health", cb),

    /** Fires when the host accepts a user message for an agent, from any client.
     *  Mirror it into the log unless it is this client's own send (same turn id). */
    onTurnSent: (cb: (e: TurnSentEvent) => void): Promise<UnlistenFn> =>
      on<TurnSentEvent>("turn:sent", cb),

    /** Fires when a turn flips to Running, carrying the backend's own start
     *  timestamp so the live timer shares the persisted duration's clock. */
    onTurnStarted: (cb: (e: TurnStartedEvent) => void): Promise<UnlistenFn> =>
      on<TurnStartedEvent>("turn:started", cb),

    onAgentStatus: (cb: (e: AgentStatusEvent) => void): Promise<UnlistenFn> =>
      on<AgentStatusEvent>("agent:status", cb),

    /** Fires at each stage boundary of a fresh spawn (clone, index, start…), so the
     *  composer and the sidebar spinner can name what the wait is for. Progress
     *  only — `agent:status` stays the authoritative state. */
    onSpawnProgress: (cb: (e: AgentSpawnProgressEvent) => void): Promise<UnlistenFn> =>
      on<AgentSpawnProgressEvent>("agent:spawn-progress", cb),

    onAgentView: (cb: (e: AgentViewEvent) => void): Promise<UnlistenFn> =>
      on<AgentViewEvent>("agent:view", cb),

    /** Fires when a session's reasoning effort is changed mid-conversation, so the
     *  composer's effort chip reflects the new value without a full resync. */
    onAgentEffort: (cb: (e: AgentEffortEvent) => void): Promise<UnlistenFn> =>
      on<AgentEffortEvent>("agent:effort", cb),

    /** Fires when the folders granted to an agent change, from any client, so its
     *  composer shows the same grants everywhere. */
    onAgentFolderGrants: (cb: (e: AgentFolderGrantsEvent) => void): Promise<UnlistenFn> =>
      on<AgentFolderGrantsEvent>("agent:folder-grants", cb),

    /** Fires when a session's model is changed mid-conversation, so the composer's
     *  model picker reflects the new value without a full resync. */
    onAgentModel: (cb: (e: AgentModelEvent) => void): Promise<UnlistenFn> =>
      on<AgentModelEvent>("agent:model", cb),

    onAgentTask: (cb: (e: AgentTaskEvent) => void): Promise<UnlistenFn> =>
      on<AgentTaskEvent>("agent:task", cb),

    /** The agent titled its work via `set_title`; the sidebar subtitle prefers it. */
    onAgentTitle: (cb: (e: AgentTitleEvent) => void): Promise<UnlistenFn> =>
      on<AgentTitleEvent>("agent:title", cb),

    /** The agent wrote a walkthrough of its changes via `tour_set`; reload it with
     *  `api.getTour`. */
    onTourChanged: (cb: (e: TourChangedEvent) => void): Promise<UnlistenFn> =>
      on<TourChangedEvent>("tour:changed", cb),

    /** The agent asked for the user's attention via its `notify` mailbox op. */
    onAgentNotify: (cb: (e: AgentNotifyEvent) => void): Promise<UnlistenFn> =>
      on<AgentNotifyEvent>("agent:notify", cb),

    onAgentBranch: (cb: (e: AgentBranchEvent) => void): Promise<UnlistenFn> =>
      on<AgentBranchEvent>("agent:branch", cb),

    onAgentRepoAdded: (cb: (e: AgentRepoAddedEvent) => void): Promise<UnlistenFn> =>
      on<AgentRepoAddedEvent>("agent:repo_added", cb),

    /** A host delegation moved (`supervisor::delegation`): recorded, delivered,
     *  running, or over with an outcome notice. */
    onDelegationChanged: (cb: (e: DelegationEvent) => void): Promise<UnlistenFn> =>
      on<DelegationEvent>("delegation:changed", cb),

    /** A checkout's autopilot row changed on the host: enrollment, pause, project
     *  switch or cycle. Replaces the row it names. */
    onAutopilotState: (cb: (e: AutopilotCheckout) => void): Promise<UnlistenFn> =>
      on<AutopilotCheckout>("autopilot:state", cb),

    /** An autopilot switch was written on the host — by any client, even for a
     *  project with no agents. Carries both opt-out lists whole. */
    onAutopilotSwitches: (cb: (e: AutopilotSwitches) => void): Promise<UnlistenFn> =>
      on<AutopilotSwitches>("autopilot:switches", cb),

    /** The host's autopilot did something worth recording on a checkout. */
    onAutopilotEvent: (cb: (e: AutopilotLogEntry) => void): Promise<UnlistenFn> =>
      on<AutopilotLogEntry>("autopilot:event", cb),

    onWorkspaceChanged: (cb: () => void): Promise<UnlistenFn> =>
      on<unknown>("workspace:changed", () => cb()),

    /** The idle-workspace sweep archived something; names what, for a notice. */
    onWorkspaceAutoArchived: (cb: (e: WorkspaceAutoArchivedEvent) => void): Promise<UnlistenFn> =>
      on<WorkspaceAutoArchivedEvent>("workspace:auto-archived", cb),

    onPrStateChanged: (cb: (e: PrStateChangedEvent) => void): Promise<UnlistenFn> =>
      on<PrStateChangedEvent>("pr:state_changed", cb),

    /** The host-side PR watcher's CI read moved (rollup or failing set). */
    onPrChecksChanged: (cb: (e: PrChecksChangedEvent) => void): Promise<UnlistenFn> =>
      on<PrChecksChangedEvent>("pr:checks_changed", cb),

    /** The host-side PR watcher found new unresolved review threads. */
    onPrThreadsChanged: (cb: (e: PrThreadsChangedEvent) => void): Promise<UnlistenFn> =>
      on<PrThreadsChangedEvent>("pr:threads_changed", cb),

    /** A checkout's local review moved on the host: a turn started or ended, a
     *  verdict landed, the code went stale. Replaces the state it names. */
    onReviewChanged: (cb: (e: ReviewState) => void): Promise<UnlistenFn> =>
      on<ReviewState>("review:changed", cb),

    /** The host-side PR watcher saw a non-focused PR of a checkout's set change. */
    onPrSetEntryChanged: (cb: (e: PrSetEntryChangedEvent) => void): Promise<UnlistenFn> =>
      on<PrSetEntryChangedEvent>("pr:set_entry_changed", cb),

    onRunOutput: (cb: (e: RunOutputEvent) => void): Promise<UnlistenFn> =>
      on<RunOutputEvent>("run:output", cb),

    onRunState: (cb: (e: RunStateEvent) => void): Promise<UnlistenFn> =>
      on<RunStateEvent>("run:state", cb),

    onRunPort: (cb: (e: RunPortEvent) => void): Promise<UnlistenFn> =>
      on<RunPortEvent>("run:port", cb),

    /** An agent is waiting for the user to approve one publish. Only fires when the
     *  `publish_confirmation` setting is on; unanswered requests are denied backend
     *  side after a timeout, so ignoring one is safe. */
    onPublishApprovalRequested: (cb: (e: PublishApproval) => void): Promise<UnlistenFn> =>
      on<PublishApproval>("publish:approval-requested", cb),

    /** One held publish is no longer held — answered here, answered on another
     *  device, answered from the host's CLI, or refused because nobody answered in
     *  time. The prompt for `id` comes down; the verdict itself has already reached
     *  the agent. A host that never emits it leaves the old behaviour intact: the
     *  prompt stays until it is answered. */
    onPublishApprovalResolved: (cb: (e: PublishApprovalResolved) => void): Promise<UnlistenFn> =>
      on<PublishApprovalResolved>("publish:approval-resolved", cb),

    /** A device on the network asks this Mac to pair, and is showing `code`.
     *  Local: it is this machine's listener being asked, whatever environment the
     *  window is driving. */
    onPairRequest: (cb: (e: PairRequest) => void): Promise<UnlistenFn> =>
      onLocal<PairRequest>("remote:pair-request", cb),

    /** That request is over — answered (here or in another window), withdrawn by
     *  the device, or timed out. */
    onPairRequestEnded: (cb: (e: { id: string }) => void): Promise<UnlistenFn> =>
      onLocal<{ id: string }>("remote:pair-request-ended", cb),

    /** The host closed the pairing window itself — after too many pairing
     *  requests, which anyone on the network can send — so the code on screen no
     *  longer works. */
    onPairingClosed: (cb: (e: { reason: string }) => void): Promise<UnlistenFn> =>
      onLocal<{ reason: string }>("remote:pairing-closed", cb),

    /** A desktop paired with this Mac (with Full access) offered its own pairing
     *  link, so this Mac can control it too. */
    onInviteOffered: (cb: (e: InviteOffer) => void): Promise<UnlistenFn> =>
      onLocal<InviteOffer>("remote:invite-offered", cb),

    /** The box image install moved: progress while downloading, then the outcome.
     *  Feeds the Sandbox page's image panel. */
    onBoxImage: (cb: (e: BoxImageStatus) => void): Promise<UnlistenFn> =>
      on<BoxImageStatus>("box:image", cb),

    /** One agent's box changed phase (preparing / starting / running / failed…). */
    onBoxState: (cb: (e: BoxStateEvent) => void): Promise<UnlistenFn> =>
      on<BoxStateEvent>("box:state", cb),

    /** A provider's tools disk build started, finished or failed. Feeds the
     *  Developer pane's setup line. */
    onBoxTools: (cb: (e: BoxToolsStatus) => void): Promise<UnlistenFn> =>
      on<BoxToolsStatus>("box:tools", cb),

    /** The egress proxy answered a connection from a box; the workspace surfaces
     *  the denied ones. */
    onBoxEgress: (cb: (e: BoxEgressEvent) => void): Promise<UnlistenFn> =>
      on<BoxEgressEvent>("box:egress", cb),

    /** A VM is holding a connection to a host until someone answers (the `ask`
     *  posture). The hold refuses itself after about a minute, so ignoring one is
     *  safe; `onBoxEgressResolved` says when it is over. */
    onBoxEgressRequested: (cb: (e: EgressPrompt) => void): Promise<UnlistenFn> =>
      on<EgressPrompt>("box:egress-requested", cb),

    /** One held host is no longer held: answered here, answered on another
     *  device, or refused because nobody answered. The prompt for `id` comes down. */
    onBoxEgressResolved: (cb: (e: EgressPromptResolved) => void): Promise<UnlistenFn> =>
      on<EgressPromptResolved>("box:egress-resolved", cb),

    /** The set of VMs changed; `boxList` says what it is now. */
    onBoxList: (cb: () => void): Promise<UnlistenFn> => on<unknown>("box:list", () => cb()),

    /** What boxes may reach changed in the app: the global list, or a project's
     *  in-app additions. Reload `boxEgressOverview`. */
    onBoxPolicy: (cb: (e: BoxPolicyEvent) => void): Promise<UnlistenFn> =>
      on<BoxPolicyEvent>("box:policy", cb),

    /** A host-owned global setting was written — by this window, another desktop
     *  or a phone. Fold it over what `get_settings` answered. */
    onSettingsChanged: (cb: (e: SettingsChangedEvent) => void): Promise<UnlistenFn> =>
      on<SettingsChangedEvent>("settings:changed", cb),

    /** A setting of THIS desktop's engine was written, whatever environment is
     *  active — for rows about this Mac, like its provider accounts' limits
     *  (`provider_limits_*`), which a remote host's events must not overwrite. */
    onLocalSettingsChanged: (cb: (e: SettingsChangedEvent) => void): Promise<UnlistenFn> =>
      onLocal<SettingsChangedEvent>("settings:changed", cb),

    /** One project's client-writable setting was written, from any client. */
    onProjectSettingsChanged: (cb: (e: ProjectSettingsChangedEvent) => void): Promise<UnlistenFn> =>
      on<ProjectSettingsChangedEvent>("project_settings:changed", cb),

    /** A project's `.fletch/settings.json` changed on the host, or what of it is
     *  approved did. Reload `projectFileStatus`. */
    onProjectFileChanged: (cb: (e: ProjectFileChangedEvent) => void): Promise<UnlistenFn> =>
      on<ProjectFileChangedEvent>("project_file:changed", cb),

    /** A write landed on one project's context, from any client or the host's own
     *  pipeline. Carries no row: reload `context_overview`. Fires for every
     *  project — a listener scoped to one tab filters on `project_id`. */
    onContextChanged: (cb: (e: ContextChangedEvent) => void): Promise<UnlistenFn> =>
      on<ContextChangedEvent>("context:changed", cb),

    /** Raw PTY bytes from a provider's in-app sign-in (Settings → Providers). */
    onProviderLoginOutput: (cb: (e: ProviderLoginOutputEvent) => void): Promise<UnlistenFn> =>
      onLocal<ProviderLoginOutputEvent>("provider-login:output", cb),

    /** A provider's sign-in process ended — cleanly, with an error, or because it
     *  was closed. */
    onProviderLoginExit: (cb: (e: ProviderLoginExitEvent) => void): Promise<UnlistenFn> =>
      onLocal<ProviderLoginExitEvent>("provider-login:exit", cb),
  };
}

export type Events = ReturnType<typeof makeEvents>;

/** The events of this desktop's own engine: routed and local-pinned alike, they
 *  subscribe through the Tauri singleton. */
const local = makeEvents(
  (event, cb) => localTransport.on(event, cb),
  (event, cb) => localTransport.on(event, cb),
);

export const {
  onWfEvent,
  onWfRun,
  onWfRunDeleted,
  onRoadmapItem,
  onRoadmapItemDeleted,
  onRoadmapItemEvent,
  onRoadmapProposal,
  onRoadmapProposalDeleted,
  onRoadmapOrderProposal,
  onRoadmapOrderProposalDeleted,
  onRoadmapProjectHold,
  onRoadmapProjectHoldReleased,
  onRoadmapQueueNote,
  onAgentInstallState,
  onAgentOutput,
  onShellOutput,
  onDictationTranscript,
  onDictationState,
  onDictationLevel,
  onDictationModelProgress,
  onAgentEvent,
  onSessionRecordsAppended,
  onSessionSyncHealth,
  onTurnSent,
  onTurnStarted,
  onAgentStatus,
  onSpawnProgress,
  onAgentView,
  onAgentEffort,
  onAgentFolderGrants,
  onAgentModel,
  onAgentTask,
  onAgentTitle,
  onTourChanged,
  onAgentNotify,
  onAgentBranch,
  onAgentRepoAdded,
  onDelegationChanged,
  onAutopilotState,
  onAutopilotSwitches,
  onAutopilotEvent,
  onWorkspaceChanged,
  onWorkspaceAutoArchived,
  onPrStateChanged,
  onPrChecksChanged,
  onPrThreadsChanged,
  onReviewChanged,
  onPrSetEntryChanged,
  onRunOutput,
  onRunState,
  onRunPort,
  onPublishApprovalRequested,
  onPublishApprovalResolved,
  onPairRequest,
  onPairRequestEnded,
  onPairingClosed,
  onInviteOffered,
  onBoxImage,
  onBoxState,
  onBoxTools,
  onBoxEgress,
  onBoxEgressRequested,
  onBoxEgressResolved,
  onBoxList,
  onBoxPolicy,
  onSettingsChanged,
  onLocalSettingsChanged,
  onProjectSettingsChanged,
  onProjectFileChanged,
  onContextChanged,
  onProviderLoginOutput,
  onProviderLoginExit,
} = local;
