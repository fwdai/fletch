// What belongs to one engine, as store keys: the part of the store each host
// has a copy of.
//
// The local environment's copy is the store's own top-level keys, exactly as
// they have always been. Each paired host's is its partition in `hostStates`,
// read and written through that host's scope (./hostScope), so every connected
// host's state is live at once and none of them can write another's.
//
// Why a partition rather than a namespace per map. Agent ids are recycled place
// names, so two hosts will both have an agent called `fuji`
// (docs/multi-host-plan.md §1.3), and nearly every per-agent map in the store
// would cross-wire on that. Namespacing each one means finding and changing
// every key-construction site across a dozen slices; giving each host its own
// copy of the whole set is one list, in one place.
//
// What is NOT here, and why:
//   - The side shell's PTY ring buffers key on the environment themselves
//     (src/pty/buffers).
//   - Provider versions, model catalogs, installs, the custom agent / skill /
//     MCP libraries, the account profile, the Linear connection, the cloud
//     providers and the appearance settings are properties of this desktop,
//     keyed by provider or runtime.
//   - `gitBusy` / `gitNotices` key on the environment already (`gitScopeKey`).
//   - Where the UI is (open screens, panes, the palette, the sub-agent thread)
//     is this window's, whichever host it is showing.

import {
  DEFAULT_AUTO_ARCHIVE_IDLE_DAYS,
  DEFAULT_PUBLISH_APPROVAL_WAIT,
} from "@/storage/preferences";
import type { AppState } from "./types";

/** Everything here is keyed by something that belongs to a single engine — an
 *  agent id, a checkout (`agentId::subdir`), a repo path — or is that engine's
 *  own answer to a question (its snapshot, its settings, its VMs). */
export const HOST_KEYS = [
  // The snapshot, and what is selected in it.
  "workspace",
  "selectedAgentId",
  "selectedRunId",
  "focusedStepAgentId",
  "activeDraftId",
  "historyOpen",
  "projectScreen",
  "attendedChatId",
  // The add-project popover: a way into the host's own repos, so it belongs to
  // the host it was raised over — its own gate decides whether it can be.
  "addProjectOpen",
  // A project waiting to join the sidebar's focus is this host's.
  "focusJoin",
  // Per-agent chat state.
  "offSidebarAgents",
  "managedLogs",
  "pendingToolUse",
  "backgroundTasks",
  "transcriptLoading",
  "transcriptLoaded",
  "sending",
  "busyLabel",
  "compactions",
  "spawnStage",
  "turnStartedAt",
  "switchingAccount",
  "movingWorkspace",
  "folderChangeWaiting",
  "unseenResults",
  "syncHealth",
  "usage",
  "runs",
  // Per-checkout git / PR / delegation state, polled or re-read on every
  // handshake (`loadDelegations`).
  "gitStates",
  "gitBlocked",
  "gitShortstats",
  "prStates",
  "prChecks",
  "prComments",
  "prSets",
  "delegations",
  "delegationNotices",
  // Autopilot as each host runs it: its rows and history per checkout, and its
  // two opt-out lists (a project or agent id means something on one host only).
  "autopilot",
  "autopilotLog",
  "autopilotDisabledProjects",
  "autopilotPausedAgents",
  // The local reviews each host runs, per checkout.
  "reviews",
  // Publishes waiting on an answer: they belong to the engine that is blocked
  // on one.
  "pendingPublishApprovals",
  // The hosts its VMs are held waiting to reach, and the hosts they were
  // refused: both name agents, and a prompt answered on the wrong host would
  // send its id to an engine that never raised it.
  "pendingEgressPrompts",
  "promptedHosts",
  "boxEgressBlocked",
  // Its VMs and their phases, and where its box setup stands.
  "boxVms",
  "boxStates",
  "boxProbe",
  "boxImage",
  "boxTools",
  // Where a new agent may run, as the host answered for the picker on screen.
  "workspaceTypes",
  // Whose GitHub answers for this host's checkouts.
  "github",
  // The host-owned settings (./hostSettings): the host's own rows, read from
  // it and kept live by its `settings:changed`.
  "notifyTurnComplete",
  "notifyPrActivity",
  "providerPathOverrides",
  "agentAttributionRemoved",
  "codeIndexingEnabled",
  "contextLayerEnabled",
  "publishConfirmation",
  "publishApprovalWait",
  "autoArchiveIdleDays",
  "branchPrefix",
  "draftPrs",
  "boxMemoryMib",
  "boxCpus",
  "boxDiskGib",
  "boxImageUrl",
  "boxIdleStopMinutes",
  "boxEgressMode",
  // What the user typed but did not send, per agent or per draft — with the
  // roadmap item picked into an agent's draft — and the drafts themselves
  // (grouped by repo path, which is the engine's).
  "composerSeeds",
  "composerDrafts",
  "composerRoadmapPicks",
  "drafts",
  // Remembered right-rail tab per agent.
  "rightPanelTabs",
  // A rewind's code undo, which only this host's engine can apply.
  "codeUndo",
] as const;

export type HostKey = (typeof HOST_KEYS)[number];

/** One host's copy of the host keys. */
export type HostState = Pick<AppState, HostKey>;

const hostKeys: ReadonlySet<string> = new Set(HOST_KEYS);

export const isHostKey = (key: string): key is HostKey => hostKeys.has(key);

/** The state a host starts in when it is first published — the values the
 *  slices themselves initialise with. Typed as `HostState`, so adding a key to
 *  `HOST_KEYS` without giving it a starting value here is a compile error. */
export const BLANK_HOST_STATE: HostState = {
  workspace: null,
  selectedAgentId: null,
  selectedRunId: null,
  focusedStepAgentId: null,
  activeDraftId: null,
  historyOpen: false,
  projectScreen: null,
  attendedChatId: null,
  addProjectOpen: false,
  focusJoin: null,
  offSidebarAgents: {},
  managedLogs: {},
  pendingToolUse: {},
  backgroundTasks: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  sending: {},
  busyLabel: {},
  compactions: {},
  spawnStage: {},
  turnStartedAt: {},
  switchingAccount: {},
  movingWorkspace: {},
  folderChangeWaiting: {},
  unseenResults: {},
  syncHealth: {},
  usage: {},
  runs: {},
  gitStates: {},
  gitBlocked: {},
  gitShortstats: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  prSets: {},
  delegations: {},
  delegationNotices: {},
  autopilot: {},
  autopilotLog: {},
  autopilotDisabledProjects: null,
  autopilotPausedAgents: [],
  reviews: {},
  pendingPublishApprovals: [],
  pendingEgressPrompts: [],
  promptedHosts: new Set(),
  boxEgressBlocked: [],
  boxVms: null,
  boxStates: {},
  boxProbe: null,
  boxImage: null,
  boxTools: {},
  workspaceTypes: { inputs: null, answer: null, failedKey: null },
  github: null,
  notifyTurnComplete: true,
  notifyPrActivity: true,
  providerPathOverrides: {},
  agentAttributionRemoved: false,
  codeIndexingEnabled: true,
  contextLayerEnabled: false,
  publishConfirmation: false,
  publishApprovalWait: DEFAULT_PUBLISH_APPROVAL_WAIT,
  autoArchiveIdleDays: DEFAULT_AUTO_ARCHIVE_IDLE_DAYS,
  branchPrefix: "",
  draftPrs: false,
  boxMemoryMib: "",
  boxCpus: "",
  boxDiskGib: "",
  boxImageUrl: "",
  boxIdleStopMinutes: "",
  boxEgressMode: "",
  composerSeeds: {},
  composerDrafts: {},
  composerRoadmapPicks: {},
  drafts: [],
  rightPanelTabs: {},
  codeUndo: {},
};
