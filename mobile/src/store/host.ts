// One host's store: everything the phone holds about a host, built over that
// host's link (host/index.ts) and talking to it through nothing else. Agent ids
// recur across hosts, so nothing here is shared between two stores: the state,
// and the order of every read and write, belong to the instance. What belongs
// to the phone itself — navigation, sheets, the paired hosts, which one is on
// screen — is the app store's (store/index.ts).

import type { BackgroundTaskMap } from "@desktop/adapters/shared/backgroundTasks";
import type { AgentManagedEvent, AgentRecord, Workspace } from "@desktop/api/types/agent";
import type { CheckoutFile, DirListing } from "@desktop/api/types/checkout";
import type {
  AutopilotCheckout,
  AutopilotLogEntry,
  AutopilotSwitches,
  DelegationEvent,
  GitState,
  ShortStats,
} from "@desktop/api/types/git";
import type { PrChecks, PrComments, PrState } from "@desktop/api/types/pr";
import type { GhRepoSummary, GhStatus } from "@desktop/api/types/providers";
import type { PublishApproval } from "@desktop/api/types/sandbox";
import type { LiveTurn } from "@desktop/api/types/session";
import type { VerificationReport } from "@desktop/api/types/verify";
import { appActionMessage } from "@desktop/delegation";
import { reconcileSending, whileSending } from "@desktop/helpers/sending";
import { createPrWriteOrder } from "@desktop/store/prWriteOrder";
import { newestWins } from "@desktop/util/newestWins";
import { type ApprovalEvent, replayApprovalEvents } from "@desktop/util/publishApprovals";
import { createStore, type StoreApi, useStore } from "zustand";
import type { ChatItem } from "../adapters";
import { type HostLink, linkOf } from "../host";
import { useHost } from "../host/useHost";
import { isBusy } from "../lib/agents";
import { ignore } from "../lib/ignore";
import { projectById } from "../lib/projects";
import {
  type ConnectionState,
  type HelloResult,
  type HostInfo,
  type HostProtocol,
  hostSupports,
  type PairStep,
  type Via,
} from "../remote";
import { type AutopilotMap, autopilotFromSnapshot, checkoutKey, switchesOf } from "./autopilot";
import { type ChatsSlice, createChatLists, createChatsSlice } from "./chats";
import { registerRemoteEvents } from "./events";
import { type AgentTab, appStore, hostOf } from "./index";
import { replayLiveTurn, runningTurnStart } from "./liveTurn";
import { saveDestParent } from "./persist";
import { createProposalsSlice, type ProposalsSlice } from "./proposals";
import { createProviderAccountsSlice, type ProviderAccountsSlice } from "./providerAccounts";
import { createRoadmapSlice, type RoadmapSlice } from "./roadmap";
import {
  appendActivity,
  askedText,
  autopilotActivity,
  delegationActivityText,
  mergeActivity,
  type ShipActivityMap,
} from "./shipActivity";
import { type LoadedHistory, type LogLoad, pastHistory, renderHistory } from "./transcript";
import { createWorkspaceReads } from "./workspaceReads";

export interface HostState extends ChatsSlice, ProposalsSlice, ProviderAccountsSlice, RoadmapSlice {
  connection: ConnectionState;
  connectionError: string | null;
  /** Mirrors the client: an `error` with a retry scheduled behind it reads as
   *  "reconnecting", one without as something the user has to fix. */
  retrying: boolean;
  /** How far the dial the app started for this host has got (`dial` in
   *  store/index), and null when none is in flight. A retry the client runs on
   *  its own reports nothing here: that is the connection banner's business. */
  dialStep: PairStep | null;
  hostInfo: HostInfo | null;
  /** What the connected host said it answers, or null when it said nothing (a
   *  host from before the field) or nothing is connected. Read through
   *  `hostSupports`, never compared against `hostInfo.appVersion`. */
  protocol: HostProtocol | null;
  /** The host's relay base URL, when it has one; the fallback path. Set on
   *  the Mac only — this mirrors what its last handshake answered. */
  relay: string | null;
  /** Which path the live connection took — mirrored from the client for the
   *  Host sheet, and null when there is no connection. */
  via: Via | null;
  /** Where the last clone landed on this host, so the next one is offered the
   *  same folder. Hydrated from the persist layer when the host is known. */
  lastDestParent: string | null;

  workspace: Workspace | null;
  logs: Record<string, ChatItem[]>;
  /** Per agent, how the last read of its history from the host went — what
   *  tells a chat that is still loading (or failed to) apart from one that
   *  loaded. Absent until a read has been asked for. */
  logLoads: Record<string, LogLoad>;
  /** Per agent, the records its log was last reduced from and the cursor for
   *  older ones (store/transcript). What `loadOlderLog` extends; a set `older`
   *  is what offers the chat's "Load older messages". */
  histories: Record<string, LoadedHistory>;
  /** Agents with a send from this device that the host has not answered with
   *  a status yet. The agent's `status` is the busy signal (`isAgentBusy`);
   *  this only bridges the gap between the tap and the `running` it produces —
   *  or the spawn it triggers first. Discharged by the next `agent:status`
   *  other than that spawn and its resting `idle`, by a failed send, and by a
   *  snapshot that shows the agent at rest (`reconcileSending`). */
  sending: Record<string, boolean>;
  /** tool_use id → held control-protocol request id, per agent. */
  pendingToolUse: Record<string, Record<string, string>>;
  /** Gated publishes waiting on an answer, oldest first. The host is blocked on
   *  each one until it is answered or its wait lapses. */
  pendingPublishApprovals: PublishApproval[];
  turnStartedAt: Record<string, number>;
  /** Claude's background sub-agents per agent, keyed by task id. Live only:
   *  never persisted, empty after every handshake. See store/backgroundTasks. */
  backgroundTasks: Record<string, BackgroundTaskMap>;
  /** Per agent, the `seq` of the first live frame a replayed turn did *not*
   *  hold (see store/liveTurn): frames below it are already in the log and are
   *  dropped on arrival. Empty after every handshake, since a host restart
   *  starts its count over. */
  liveSeq: Record<string, number>;
  gitStates: Record<string, GitState | null>;
  /** Uncommitted working-tree stats for the whole fleet, from the app-wide
   *  poll — the same numbers the desktop sidebar shows. */
  shortstats: Record<string, ShortStats>;
  prStates: Record<string, PrState | null>;
  prChecks: Record<string, PrChecks | null>;
  /** Unresolved review threads per agent: one `get_pr_threads` when the Ship
   *  tab opens with none cached, then `pr:threads_changed`. Never set on a host
   *  without the op, so a missing key means "unknown", not "none". Emptied on
   *  every handshake — an event missed while the socket was down has no other
   *  way back — so an open tab reads again. */
  prComments: Record<string, PrComments | null>;
  /** The latest turn-end verification per agent (`verify:report`). Absent =
   *  never verified since this launch. Not persisted. */
  verificationReports: Record<string, VerificationReport>;
  /** The Ship tab's activity list per agent, newest first (store/shipActivity).
   *  Empty after every handshake, like `backgroundTasks`, then refilled with
   *  the autopilot log the host keeps (`autopilot_log`). */
  shipActivity: ShipActivityMap;
  /** Names of the workspaces the host's idle sweep archived
   *  (`workspace:auto-archived`) since the user last dismissed the notice on
   *  Home. Null when there is nothing to say. Not persisted. */
  autoArchived: string[] | null;
  /** The host's live delegation per agent (`delegation:changed`), primary repo
   *  only — the phone is single-repo for now. A mirror: the host decides when
   *  one starts, runs and ends; the Ship strip reads `working` off it. Re-read
   *  with `get_delegations` on every handshake. */
  delegations: Record<string, DelegationEvent>;
  /** The host's autopilot per checkout (store/autopilot), every checkout of
   *  every live agent. A mirror like `delegations`: `autopilot:state` replaces
   *  a row, a snapshot (`autopilot_state` on every handshake, `autopilot_set`'s
   *  answer) replaces them all. Empty on a host without the ops. */
  autopilot: AutopilotMap;
  /** The host's two autopilot opt-out lists, whole: from every snapshot, and
   *  replaced by `autopilot:switches` — which every `autopilot_set` fires, even
   *  on a project with no agents (so no row changes). `null` until read. */
  autopilotSwitches: AutopilotSwitches | null;
  trees: Record<string, CheckoutFile[]>;

  /** Retry the link the client already holds — the store keeps no copy of the
   *  target, so there is no stale pairing token to replay. */
  reconnect(): Promise<void>;
  /** Whether the connected host answers `op` (docs/remote-protocol.md,
   *  "Compatibility"). Everything on the v1 allowlist is always true, so only a
   *  surface added after v2 was fixed needs to ask. */
  hostSupports(op: string): boolean;
  /** Empty everything held of the host (`hostScopedState`), and refuse every
   *  PR read still out. */
  clear(): void;
  /** Take the snapshot a handshake answered with, then re-read what it does
   *  not carry and what the socket may have missed while it was down — the
   *  screens open on this host included. Every handshake of the link, whichever
   *  host is on screen. */
  applyHandshake(snapshot: HelloResult): void;

  /** Re-read the workspace snapshot. Resolves to whether the host answered:
   *  the foreground probe reads that, everything else ignores it. Never
   *  rejects — a failed read is best effort and the next resync recovers. */
  refreshWorkspace(): Promise<boolean>;
  /** Open an agent's screen, on `tab` when given (an alert about its PR lands
   *  on Ship rather than the chat). */
  openAgent(agentId: string, tab?: AgentTab): void;
  loadAgent(agentId: string): Promise<void>;
  /** Rebuild the log from the host's records. With `liveTurn`, the running
   *  turn is replayed on top from `read_live_turn` (see store/liveTurn).
   *  Reports how the read went in `logLoads`, not `lastError`: the chat that
   *  needed it is where the failure is shown. */
  rebuildLog(agentId: string, opts?: { liveTurn?: boolean }): Promise<void>;
  /** Read the page before the oldest one loaded and put it at the head of the
   *  log. A no-op when there is no older page to read. A failure rejects and
   *  is not put in `lastError`: the control that asked shows it. */
  loadOlderLog(agentId: string): Promise<void>;
  loadGit(agentId: string): Promise<void>;
  /** Read the agent's working-tree state alone — the local git half of
   *  `loadGit`, cheap enough to poll while the Agent screen shows it (see
   *  useGitFollow). The PR half is left to the `pr:*` events. A failed read
   *  keeps the last-known state. */
  loadGitState(agentId: string): Promise<void>;
  /** The Ship tab's one read when it opens with no checks cached: PR state and
   *  CI from one `get_pr_live` pass, so both are from the same moment. The
   *  host's `pr:*` events keep them current after that. A null `checks` means
   *  the CI read didn't resolve (see `PrLive`), so the last tint is kept rather
   *  than blanked. */
  loadPrLive(agentId: string): Promise<void>;
  /** Read the PR's unresolved review threads — once, when the Ship tab opens
   *  with none cached; `pr:threads_changed` follows. A no-op on a host without
   *  `get_pr_threads`. */
  loadPrThreads(agentId: string): Promise<void>;
  /** Refresh the fleet-wide working-tree stats behind the agent rows. */
  loadShortstats(): Promise<void>;
  /** Seed the fleet-wide PR state behind the agent rows from one
   *  `get_all_pr_status` read, on every handshake and return to the
   *  foreground; the host watcher's `pr:*` events keep it current between.
   *  Primary-repo entries only (the phone is single-repo for now); a null
   *  `checks` keeps the last tint, as in `loadPrLive`, and an agent absent from
   *  the map keeps everything — absence means "nothing to say", not "no PR". A
   *  no-op on a host without the op. */
  loadPrStatus(): Promise<void>;
  /** Clear the auto-archive notice on Home. */
  dismissAutoArchived(): void;
  loadTree(agentId: string): Promise<void>;

  /** `attachments` are host paths staged through the `attachment_*` ops; a
   *  message may be attachments alone. */
  send(agentId: string, text: string, attachments?: string[]): Promise<void>;
  spawn(input: SpawnInput): Promise<void>;
  answerToolUse(
    agentId: string,
    toolUseId: string,
    updatedInput: unknown,
    behavior: "allow" | "deny",
  ): Promise<void>;
  /** A gated publish the host is holding, from `publish:approval-requested`. */
  receivePublishApproval(request: PublishApproval): void;
  /** Replace the cards with what the host says is still waiting. The event
   *  above only reaches the devices connected when it fired, so every handshake
   *  re-reads the queue: that is how a phone opened after the agent asked sees
   *  the prompt, and how one whose card was answered elsewhere drops it. A
   *  no-op on a host too old for `approvals_list`. */
  loadPendingApprovals(): Promise<void>;
  answerPublishApproval(id: string, approved: boolean): Promise<void>;
  /** The host says that request is over (`publish:approval-resolved`) — someone
   *  else answered it, or its wait lapsed. Drop the card; there is nothing to
   *  send back. */
  resolvePublishApproval(id: string): void;
  stop(agentId: string): Promise<void>;
  resume(agentId: string): Promise<void>;
  archive(agentId: string): Promise<void>;
  /** `null` clears the pinned model, leaving the provider CLI's own default. */
  setModel(agentId: string, model: string | null): Promise<void>;
  setEffort(agentId: string, effort: string): Promise<void>;
  /** Hand a git action to the coding agent, as the desktop git panel does: the
   *  agent writes the commit message / PR description itself. `action` is a
   *  playbook name from `instructions/git_actions.md` (`commit-pr`, `open-pr`,
   *  `commit-push`, …); `params` carry only what the playbook can't know. */
  delegateGit(agentId: string, action: string, params?: Record<string, string>): Promise<void>;
  /** Fold one `delegation:changed` into `delegations`, with its activity line. */
  applyDelegation(e: DelegationEvent): void;
  /** Replace `delegations` with the host's table. A no-op on a host without
   *  `get_delegations`. */
  loadDelegations(): Promise<void>;
  /** Fold one `autopilot:state` into `autopilot`. */
  applyAutopilotState(row: AutopilotCheckout): void;
  /** Take one `autopilot:switches`: both lists, replaced whole. */
  applyAutopilotSwitches(switches: AutopilotSwitches): void;
  /** Fold one `autopilot:event` into the agent's activity list. */
  applyAutopilotEvent(entry: AutopilotLogEntry): void;
  /** Re-read autopilot from the host: its state into `autopilot`, its log into
   *  the activity lists. Each half is a no-op on a host without its op. */
  loadAutopilot(): Promise<void>;
  /** Pause (`false`) or resume an agent's autopilot (`autopilot_set`), taking
   *  the host's answer as the new state. The project's switch is the Mac's to
   *  flip. A no-op on a host — or a pairing — without the op. */
  setAgentAutopilot(agentId: string, enabled: boolean): Promise<void>;
  /** Manual path: commit + push + open a PR with the user's own text. */
  publish(agentId: string, title: string, body: string): Promise<void>;
  pushToPr(agentId: string): Promise<void>;
  /** Merge the open PR through the host (`merge_pr`), then re-read the git and
   *  PR state. Gated on `hostSupports("merge_pr")`; the failure, if any, lands
   *  in `lastError`. */
  mergePr(agentId: string): Promise<void>;

  listDir(path: string): Promise<DirListing>;
  addWorkspaceRepo(repoPath: string): Promise<void>;
  cloneRepo(spec: string, destParent: string): Promise<void>;
  ghStatus(): Promise<GhStatus>;
  ghRepoList(): Promise<GhRepoSummary[]>;
}

export interface SpawnInput {
  repoPath: string;
  provider: string;
  model: string | null;
  effort: string | null;
  base: string;
  prompt: string;
  /** Staged host paths to go with the first message, if any. */
  attachments?: string[];
  /** The workspace name the New Agent sheet showed. Empty when the sheet
   *  never got one (its allocation failed), in which case one is allocated
   *  here so the spawn still goes through. */
  name: string;
}

export type HostStore = StoreApi<HostState>;

/** Where an agent record can live: the workspace snapshot, or — for a chat the
 *  snapshot hides behind a purpose tag — the planning-chat registry. */
export type AgentSource = Pick<HostState, "workspace" | "chats">;

/** Any agent the app knows about, wherever its record is kept. Every screen
 *  that takes an agent id goes through here, which is what lets the chat, the
 *  composer and the event handlers treat a planning chat as an ordinary agent. */
export const agentOf = (s: AgentSource, id: string): AgentRecord | undefined =>
  s.workspace?.agents.find((a) => a.id === id) ??
  Object.values(s.chats)
    .flat()
    .find((c) => c.id === id);

export const projectOf = (s: AgentSource, agentId: string) =>
  projectById(s.workspace, agentOf(s, agentId)?.project_id);

/** Every project whose planning chats are loaded re-reads them: a status those
 *  records missed off-socket has no other way back. */
export function refreshChats(s: HostState): void {
  for (const projectId of Object.keys(s.chats)) {
    void s.loadChats(projectId).catch(ignore);
  }
}

/** The log of an open thread is kept current by live events, which a dropped
 *  socket or a backgrounded webview silently misses — so every reconnect and
 *  every return to the foreground re-reads it from the host. The top-most
 *  agent screen open on `hostKey`'s host, if any. */
export function refreshOpenAgent(hostKey: string | null, s: HostState): void {
  const app = appStore.getState();
  const agentId = [...app.nav]
    .reverse()
    .find((n) => n.props.agentId && hostOf(app, n.props) === hostKey)?.props.agentId;
  if (agentId) void s.loadAgent(agentId).catch(ignore);
}

/** The Providers screen of `hostKey`'s host, when it is up: its selection and
 *  meters follow `settings:changed`, which a dropped socket or a suspended app
 *  misses like any other event, and the handshake carries none of it. */
export function refreshProviders(hostKey: string | null, s: HostState): void {
  const app = appStore.getState();
  if (app.nav.some((n) => n.screen === "providers" && hostOf(app, n.props) === hostKey)) {
    void s.loadProviderAccounts().catch(ignore);
  }
}

const newId = () =>
  globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36)}`;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The `attachments` field for an optimistic bubble — present only when there
 *  are any, matching what the desktop's `mirrorSentTurn` draws for the echo. */
const withAttachments = (attachments: string[]) => (attachments.length > 0 ? { attachments } : {});

/** The store's own setter, which also takes an updater — what any write that
 *  reads the current state has to be handed. */
type StateSetter = (partial: Partial<HostState> | ((s: HostState) => Partial<HostState>)) => void;

/** Record the failure where the error UI can see it — the app's `lastError`,
 *  which the sheets that asked read — then rethrow: an action that swallows
 *  leaves its caller believing it succeeded — which is how a failed spawn used
 *  to clear the prompt the user still needs. */
async function guard<T>(fn: () => Promise<T>): Promise<T> {
  try {
    return await fn();
  } catch (e) {
    appStore.setState({ lastError: message(e) });
    throw e;
  }
}

/** Everything the store holds of its host, emptied. Fresh objects every call:
 *  they are about to be written into. */
const hostScopedState = (): Partial<HostState> => ({
  workspace: null,
  hostInfo: null,
  protocol: null,
  via: null,
  lastDestParent: null,
  // Prompts belong to the host that raised them; it is no longer waiting on
  // this screen for an answer.
  pendingPublishApprovals: [],
  pendingToolUse: {},
  logs: {},
  logLoads: {},
  histories: {},
  sending: {},
  turnStartedAt: {},
  backgroundTasks: {},
  liveSeq: {},
  gitStates: {},
  shortstats: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  verificationReports: {},
  shipActivity: {},
  autoArchived: null,
  delegations: {},
  autopilot: {},
  autopilotSwitches: null,
  trees: {},
  // The chats belong to the host that holds their checkouts, and the ghosts
  // to the boards those chats propose onto — as do the boards themselves.
  chats: {},
  proposals: {},
  boards: {},
  // …and the accounts and their meters are that host's logins.
  providerAccounts: {},
  providerLimits: {},
  providerAccountsLoad: null,
  switchingAccount: {},
});

/** Wait for a freshly spawned agent to leave `spawning` before the first
 *  message is sent — the spawn flow in docs/remote-protocol.md. */
function waitForSpawn(get: () => HostState, agentId: string, timeoutMs = 30_000) {
  return new Promise<void>((resolve, reject) => {
    const started = Date.now();
    const tick = () => {
      const status = agentOf(get(), agentId)?.status;
      if (status && status !== "spawning") return resolve();
      if (Date.now() - started > timeoutMs) return reject(new Error("agent never started"));
      setTimeout(tick, 150);
    };
    tick();
  });
}

/** A store over `link`, holding nothing yet. It folds the link's host events,
 *  connection state and handshakes in from the moment it is built, whichever
 *  host is on screen. */
export function createHostStore(link: HostLink): HostStore {
  const { client, api } = link;

  /** `props` naming this host, for a screen opened on it: it acts on this host
   *  whichever is on screen by then. Read when called — a pairing's link learns
   *  its host on the first answer. */
  const onThisHost = (props: Record<string, string>): Record<string, string> =>
    link.hostKey ? { ...props, hostKey: link.hostKey } : props;

  /** The order of this host's PR writes against its watcher's events
   *  (@desktop/store/prWriteOrder). */
  const prWrites = createPrWriteOrder();
  /** This host's sends still on the wire (see `whileSending` in
   *  helpers/sending): what a snapshot of its records must not settle. */
  const sends = new Set<string>();
  const workspaceReads = createWorkspaceReads();
  const chatLists = createChatLists();

  /** The `started_at` of the last delegation that ended per agent, so a
   *  `delegate_git` reply landing after its own end cannot put it back. */
  const endedDelegations = new Map<string, number>();

  /** Every handshake starts an `approvals_list` without waiting for the last,
   *  so two can be out at once and answer out of order. One order for all of
   *  them, as everywhere else a whole list is replaced (util/newestWins). */
  const approvalLoads = newestWins();

  /** Approval events seen while an `approvals_list` is in flight, replayed
   *  over its answer so the snapshot cannot undo them (see
   *  util/publishApprovals). Belongs to the newest claim — an older load's
   *  snapshot is superseded wholesale, so the events it missed are not its to
   *  replay. Null when no list is outstanding, which is nearly always. */
  let approvalsInFlight: ApprovalEvent[] | null = null;

  /** Per agent, the order of its history reads: opening a chat, a reconnect
   *  and a turn end can each start one, and only the newest says how loading
   *  went. */
  const logReads = new Map<string, ReturnType<typeof newestWins>>();
  const claimLogRead = (agentId: string) => {
    let order = logReads.get(agentId);
    if (!order) {
      order = newestWins();
      logReads.set(agentId, order);
    }
    return order.claim();
  };

  /** Per agent, the order of its git reads and the one out now. Two readers
   *  share a checkout, the opener (`loadGit`) and the follow poll, and over the
   *  relay a read can outlast a tick: a call while one is out joins it rather
   *  than stacking another, and only the newest read writes. */
  const gitReads = new Map<string, ReturnType<typeof newestWins>>();
  const gitReadsInFlight = new Map<string, Promise<void>>();
  const claimGitRead = (agentId: string) => {
    let order = gitReads.get(agentId);
    if (!order) {
      order = newestWins();
      gitReads.set(agentId, order);
    }
    return order.claim();
  };

  /** What a spawn does once the host has answered with a record, whichever
   *  registry that record was filed in: put the first turn on screen, open the
   *  agent, and send the prompt as soon as it leaves `spawning`.
   *
   *  `recover` re-reads that registry when the send fails — the workspace for a
   *  sidebar agent, the project's chat list for a planning chat. */
  async function firstTurn(
    set: StateSetter,
    get: () => HostState,
    {
      record,
      prompt,
      attachments,
      recover,
    }: {
      record: AgentRecord;
      prompt: string;
      attachments: string[];
      recover: () => Promise<void>;
    },
  ): Promise<void> {
    const turnId = newId();
    set((s) => ({
      logs: {
        ...s.logs,
        [record.id]: [
          { kind: "user_message", text: prompt, turnId, ...withAttachments(attachments) },
        ],
      },
      sending: { ...s.sending, [record.id]: true },
    }));
    // The sheet that asked closes, and the agent opens: the app's screens.
    const app = appStore.getState();
    app.closeSheet();
    app.push("agent", onThisHost({ agentId: record.id }));
    try {
      await whileSending(
        record.id,
        async () => {
          await waitForSpawn(get, record.id);
          await api.sendUserMessage(record.id, turnId, prompt, attachments);
        },
        sends,
      );
    } catch (e) {
      // The agent exists on the host but never got the prompt: drop the
      // optimistic turn and the sending flag, and let the re-read record show
      // whatever state it is really in.
      set((s) => {
        const logs = { ...s.logs };
        const sending = { ...s.sending };
        delete logs[record.id];
        delete sending[record.id];
        return { logs, sending };
      });
      await recover();
      throw e;
    }
  }

  const store = createStore<HostState>()((set, get) => ({
    connection: "disconnected",
    connectionError: null,
    retrying: false,
    dialStep: null,
    hostInfo: null,
    protocol: null,
    relay: null,
    via: null,
    lastDestParent: null,

    workspace: null,
    logs: {},
    logLoads: {},
    histories: {},
    sending: {},
    pendingToolUse: {},
    pendingPublishApprovals: [],
    turnStartedAt: {},
    backgroundTasks: {},
    liveSeq: {},
    gitStates: {},
    shortstats: {},
    prStates: {},
    prChecks: {},
    prComments: {},
    verificationReports: {},
    shipActivity: {},
    autoArchived: null,
    delegations: {},
    autopilot: {},
    autopilotSwitches: null,
    trees: {},

    async reconnect() {
      if (!client.target) return;
      set({ connectionError: null });
      // The subscriptions below report the failure and take the handshake.
      await client.reconnect();
    },

    hostSupports(op) {
      return hostSupports(get().protocol, op);
    },

    clear() {
      prWrites.fence();
      set(hostScopedState());
    },

    applyHandshake(snapshot) {
      set({ hostInfo: snapshot.host, protocol: snapshot.protocol ?? null });
      const ws = snapshot.workspace;
      // The handshake's snapshot is as authoritative as `refreshWorkspace`'s
      // read, so it settles the `sending` bridge the same way (helpers/sending)
      // — this is the path that clears a flag stranded by a reconnect.
      if (ws)
        set((s) => ({ workspace: ws, sending: reconcileSending(s.sending, ws.agents, sends) }));
      else void get().refreshWorkspace();
      // A PR read still out was asked of the socket before this one, so none
      // of them may land. The reads below are issued after the fence and
      // outrank it.
      prWrites.fence();
      // Planning chats are absent from that snapshot; their records, and their
      // flags, settle through their own list.
      refreshChats(get());
      // Task events missed while the socket was down are gone for good — a
      // task held as running could never be seen ending — so the maps start
      // over on every handshake and refill from whatever the host emits next.
      // The replay watermarks go with them: a restarted host counts from zero,
      // and so does the Ship tab's activity list. Review threads too: they
      // follow `pr:threads_changed`, so a missed one would linger — emptied,
      // an open Ship tab reads them again.
      set({
        backgroundTasks: {},
        liveSeq: {},
        shipActivity: {},
        delegations: {},
        autopilot: {},
        autopilotSwitches: null,
        prComments: {},
      });
      // The snapshot carries no stats or PR state, so the rows get both on
      // every handshake: stats from the first tick of their poll, PR state
      // from the one seed the watcher's events then keep current.
      void get().loadShortstats();
      void get().loadPrStatus();
      // The publishes this host is blocked on. Every handshake, because the
      // request event only reached the devices connected when it fired — and so
      // did the resolution, whoever gave it.
      void get().loadPendingApprovals().catch(ignore);
      // …and the delegations it is running: the strip's `working` state is
      // the host's, and the events that built it were lost with the socket.
      void get().loadDelegations().catch(ignore);
      // …and its autopilot, for the same reason: state and log are host facts,
      // and the events are only the deltas.
      void get().loadAutopilot().catch(ignore);
      // …and whatever is open on this host's screens.
      refreshOpenAgent(link.hostKey, get());
      refreshProviders(link.hostKey, get());
    },

    async refreshWorkspace() {
      // Only the newest read lands, with the patches made meanwhile replayed
      // over its answer (store/workspaceReads).
      const read = workspaceReads.begin();
      try {
        const workspace = await api.getWorkspace();
        if (workspace && read.current()) {
          const agents = read.apply(workspace.agents);
          const next = agents === workspace.agents ? workspace : { ...workspace, agents };
          set((s) => ({
            workspace: next,
            sending: reconcileSending(s.sending, next.agents, sends),
          }));
        }
        return true;
      } catch {
        // Best effort; the next event or resync recovers.
        return false;
      } finally {
        read.finish();
      }
    },

    openAgent(agentId, tab) {
      appStore.getState().push("agent", onThisHost(tab ? { agentId, tab } : { agentId }));
      void get().loadAgent(agentId).catch(ignore);
    },

    async loadAgent(agentId) {
      // The host's records stop at the last finished turn (the running one is
      // ingested when it ends), so a busy agent's log is rebuilt from the records
      // plus the running turn as the host has it (`read_live_turn`) — whatever
      // the socket missed while the app was in the background, for any provider.
      // A host without the op leaves the log the live events built rather than
      // replacing it with the prompt alone; the turn-end `session:records-
      // appended` rebuild is the authoritative one there.
      const agent = agentOf(get(), agentId);
      const busy = agent !== undefined && isBusy(agent);
      if (busy && get().hostSupports("read_live_turn")) {
        await get().rebuildLog(agentId, { liveTurn: true });
      } else if (!busy || get().logs[agentId] === undefined) {
        await get().rebuildLog(agentId);
      }
      // A purpose-tagged chat has no code of its own to report on — the PM never
      // edits a file, and the host denies it the publish ops — so the git and PR
      // reads are skipped rather than answered with an empty diff.
      if (!agent?.purpose) await get().loadGit(agentId);
    },

    async rebuildLog(agentId, opts = {}) {
      const read = claimLogRead(agentId);
      const report = (load: LogLoad) => {
        if (read.current()) set((s) => ({ logLoads: { ...s.logLoads, [agentId]: load } }));
      };
      // Everything a read writes lands in one update, and only while it is the
      // newest: an older read resolving last would otherwise put its transcript
      // and cursor over the newer one's, under the newer one's `ready`.
      const commit = (patch: (s: HostState) => Partial<HostState>) => {
        if (!read.current()) return;
        set((s) => ({
          ...patch(s),
          logLoads: { ...s.logLoads, [agentId]: { status: "ready" } },
        }));
      };
      const rebuild = async () => {
        // Only the newest page, where the host has pages: a long session's
        // history runs to tens of MB of tool results, and the phone needs the
        // last turns to paint. Every rebuild starts over from it, the turn-end
        // one included — so older pages the user had loaded are dropped then
        // and read again on demand. Simpler than splicing the new records onto
        // pages whose cursors the turn may have outdated, and cheap: it is one
        // page either way.
        const readNewest = async (): Promise<LoadedHistory> =>
          get().hostSupports("read_session_page")
            ? api.readSessionPage(agentId)
            : { records: await api.readSessionRecords(agentId), older: null };
        let [history, turns] = await Promise.all([readNewest(), api.readUserTurns(agentId)]);
        // The host ingests a turn's transcript into session_records only at
        // turn-end, and that ingest can insert nothing (session id not captured
        // yet, transcript not located). Mirror the desktop's readHistory: with no
        // records of the agent's own (a fork may still show inherited ones), ask
        // for a backfill and read again before concluding there is no history.
        // The newest page answers that as well as the whole list: an agent's own
        // records are the newest in its history.
        if (history.records.every((r) => r.inherited)) {
          await api.syncSession(agentId);
          [history, turns] = await Promise.all([readNewest(), api.readUserTurns(agentId)]);
        }
        const { records } = history;
        const histories = (s: HostState) => ({ ...s.histories, [agentId]: history });
        // Still nothing stored and no running turn to replay: keep the log the
        // live events built rather than wiping the conversation the user was
        // just looking at (the desktop's records-appended handler makes the same
        // call). With no log either, the chat says the history is missing —
        // never that there is no conversation (see `LogLoad`).
        if (records.length === 0 && !opts.liveTurn) {
          commit((s) => ({ histories: histories(s) }));
          return;
        }
        const provider = agentOf(get(), agentId)?.provider;
        const items = renderHistory(provider, records, turns);
        if (!opts.liveTurn) {
          commit((s) => ({ histories: histories(s), logs: { ...s.logs, [agentId]: items } }));
          return;
        }
        // The running turn is read last, once the records have settled, so the
        // stream has the shortest possible window to move on in — and every
        // frame that lands in that window is kept, to be folded after the
        // snapshot or skipped as already in it (see replayLiveTurn). The patch
        // below runs in the response's own continuation, before the next frame
        // is dispatched, so nothing arrives between the capture and the fold.
        const late: AgentManagedEvent[] = [];
        const off = client.on("agent:event", (payload) => {
          const e = payload as AgentManagedEvent;
          if (e.agent_id === agentId) late.push(e);
        });
        let live: LiveTurn;
        try {
          live = await api.readLiveTurn(agentId);
        } finally {
          off();
        }
        const startedAt = runningTurnStart(turns);
        commit((s) => ({
          histories: histories(s),
          ...replayLiveTurn(s, agentId, items, live, late),
          ...(startedAt === undefined
            ? {}
            : { turnStartedAt: { ...s.turnStartedAt, [agentId]: startedAt } }),
        }));
      };
      report({ status: "loading" });
      try {
        await rebuild();
      } catch (e) {
        report({ status: "error", error: message(e) });
        throw e;
      }
    },

    async loadOlderLog(agentId) {
      const loaded = get().histories[agentId];
      if (!loaded?.older) return;
      const [page, turns] = await Promise.all([
        api.readSessionPage(agentId, loaded.older),
        api.readUserTurns(agentId),
      ]);
      // A rebuild that landed meanwhile replaced the pages this one extends;
      // its log is the newer, and the cursor it holds is the one to follow.
      if (get().histories[agentId] !== loaded) return;
      const provider = agentOf(get(), agentId)?.provider;
      const records = [...page.records, ...loaded.records];
      // Re-reduced whole rather than prepended, so a call on this page pairs
      // with its result on the next. Everything the log holds past the loaded
      // records (a replayed running turn, live frames, a queued send) is not in
      // them and is carried over as it is.
      const items = renderHistory(provider, records, turns);
      set((s) => ({
        histories: { ...s.histories, [agentId]: { records, older: page.older } },
        logs: { ...s.logs, [agentId]: [...items, ...pastHistory(s.logs[agentId] ?? [], items)] },
      }));
    },

    async loadGit(agentId) {
      // The PR halves are ordered against the watcher's events like every PR
      // read here (see `loadPrLive`); the git state has no event to race.
      const ticket = prWrites.issue();
      try {
        const [, pr] = await Promise.all([get().loadGitState(agentId), api.getPrState(agentId)]);
        if (prWrites.accept("prStates", agentId, ticket)) {
          set((s) => ({ prStates: { ...s.prStates, [agentId]: pr } }));
        }
        if (pr) {
          const checks = await api.getPrChecks(agentId);
          if (!prWrites.accept("prChecks", agentId, ticket)) return;
          set((s) => ({ prChecks: { ...s.prChecks, [agentId]: checks } }));
        }
      } catch {
        // Git/PR reads are advisory; leave the last-known values alone.
      }
    },

    loadGitState(agentId) {
      const out = gitReadsInFlight.get(agentId);
      if (out) return out;
      const claim = claimGitRead(agentId);
      const read = (async () => {
        try {
          const git = await api.getGitState(agentId);
          if (claim.current()) set((s) => ({ gitStates: { ...s.gitStates, [agentId]: git } }));
        } catch {
          // Advisory, like loadGit: keep the last-known state.
        } finally {
          gitReadsInFlight.delete(agentId);
        }
      })();
      gitReadsInFlight.set(agentId, read);
      return read;
    },

    async loadPrLive(agentId) {
      // Claimed before asking: a `pr:*` event that lands while the read is out
      // is newer than what the read saw, and the watcher, having sent it, will
      // not send it again — so the reply must not land on top of it
      // (@desktop/store/prWriteOrder).
      const ticket = prWrites.issue();
      try {
        const live = await api.getPrLive(agentId);
        // Null means the host had nothing to say this round, not "no PR".
        if (!live) return;
        // One ticket, checked per slice: a checks event outranks only the checks.
        const takeState = prWrites.accept("prStates", agentId, ticket);
        const takeChecks = !!live.checks && prWrites.accept("prChecks", agentId, ticket);
        set((s) => ({
          ...(takeState ? { prStates: { ...s.prStates, [agentId]: live.state } } : {}),
          ...(takeChecks ? { prChecks: { ...s.prChecks, [agentId]: live.checks } } : {}),
        }));
      } catch {
        // Advisory, like loadGit: keep the last-known state.
      }
    },

    async loadPrThreads(agentId) {
      if (!get().hostSupports("get_pr_threads")) return;
      // Ordered against `pr:threads_changed`, as `loadPrLive` is against the rest.
      const ticket = prWrites.issue();
      try {
        const comments = await api.getPrThreads(agentId);
        if (!prWrites.accept("prComments", agentId, ticket)) return;
        set((s) => ({ prComments: { ...s.prComments, [agentId]: comments } }));
      } catch {
        // Advisory, like loadGit: keep the last-known threads.
      }
    },

    async loadShortstats() {
      try {
        // Replaced wholesale, mirroring the desktop's `fetchAllShortstats`: the
        // host answers for the whole fleet, so an agent that has gone clean (or
        // been archived) falls out instead of keeping its last number.
        const shortstats = await api.getAllShortstats();
        set({ shortstats });
      } catch {
        // Advisory, like the rest of the git reads: keep the last numbers.
      }
    },

    async loadPrStatus() {
      if (!get().hostSupports("get_all_pr_status")) return;
      // Ordered per agent, row by row: an event for one agent while the sweep is
      // out outranks that agent's row and nobody else's (see `loadPrLive`).
      const ticket = prWrites.issue();
      try {
        const all = await api.getAllPrStatus();
        const states: Record<string, PrState | null> = {};
        const checks: Record<string, PrChecks> = {};
        for (const [key, entry] of Object.entries(all)) {
          // `"{agentId}::{subdir}"` keys are secondary repos, which no screen
          // here shows yet.
          if (key.includes("::")) continue;
          if (prWrites.accept("prStates", key, ticket)) states[key] = entry.state;
          if (entry.checks && prWrites.accept("prChecks", key, ticket)) checks[key] = entry.checks;
        }
        set((s) => ({
          prStates: { ...s.prStates, ...states },
          prChecks: { ...s.prChecks, ...checks },
        }));
      } catch {
        // Advisory, like loadShortstats: keep the last-known state.
      }
    },

    dismissAutoArchived() {
      set({ autoArchived: null });
    },

    async loadTree(agentId) {
      return guard(async () => {
        const tree = await api.listCheckoutTree(agentId);
        set((s) => ({ trees: { ...s.trees, [agentId]: tree } }));
      });
    },

    async send(agentId, text, attachments = []) {
      const trimmed = text.trim();
      if (!trimmed && attachments.length === 0) return;
      // Optimistic bubble, reconciled away when the canonical records land. It
      // carries the turn id so the host's `turn:sent` echo (which every device
      // mirrors, see events.ts) is recognized as ours and not drawn twice.
      const turnId = newId();
      set((s) => ({
        logs: {
          ...s.logs,
          [agentId]: [
            ...(s.logs[agentId] ?? []),
            { kind: "queued_message", text: trimmed, turnId, ...withAttachments(attachments) },
          ],
        },
        sending: { ...s.sending, [agentId]: true },
      }));
      return guard(async () => {
        try {
          await whileSending(
            agentId,
            () => api.sendUserMessage(agentId, turnId, trimmed, attachments),
            sends,
          );
        } catch (e) {
          // The send never reached the host, so no status will discharge it.
          set((s) => ({ sending: { ...s.sending, [agentId]: false } }));
          throw e;
        }
      });
    },

    async spawn({
      repoPath,
      provider,
      model,
      effort,
      base,
      prompt,
      attachments = [],
      name: shown,
    }) {
      return guard(async () => {
        // The user has been looking at (and possibly rerolled) this name; the
        // agent must launch under it, not a fresh draw.
        const name = shown || (await api.allocateDraftName([]));
        const record = await api.spawnAgent(repoPath, provider, name, effort, model, base);
        set((s) => ({
          workspace: s.workspace
            ? { ...s.workspace, agents: [record, ...s.workspace.agents] }
            : s.workspace,
        }));
        await firstTurn(set, get, {
          record,
          prompt,
          attachments,
          recover: async () => {
            await get().refreshWorkspace();
          },
        });
      });
    },

    async answerToolUse(agentId, toolUseId, updatedInput, behavior) {
      const requestId = get().pendingToolUse[agentId]?.[toolUseId];
      if (!requestId) return;
      // Feeding the answer resumes the paused turn, which never left `running`,
      // so the working indicator returns on its own.
      set((s) => {
        const forAgent = { ...(s.pendingToolUse[agentId] ?? {}) };
        delete forAgent[toolUseId];
        return { pendingToolUse: { ...s.pendingToolUse, [agentId]: forAgent } };
      });
      return guard(async () => {
        await api.answerToolUse(agentId, requestId, updatedInput, behavior);
      });
    },

    receivePublishApproval(request) {
      // Nothing to pre-authorize here: the host approves an enrolled checkout's
      // autopilot pushes itself and never raises those, so every prompt that
      // does arrive is one for a person to answer.
      approvalsInFlight?.push({ kind: "requested", request });
      set((s) => ({ pendingPublishApprovals: [...s.pendingPublishApprovals, request] }));
    },

    async loadPendingApprovals() {
      if (!get().hostSupports("approvals_list")) return;
      const claim = approvalLoads.claim();
      const buffer: ApprovalEvent[] = [];
      approvalsInFlight = buffer;
      try {
        // The snapshot is what the host was blocked on when it read its queue,
        // not when it answered; whatever it said in between is folded back in.
        const pending = await api.listPublishApprovals();
        // …unless a newer load has been issued to replace this queue, in which
        // case ours is the older snapshot and writing it would undo the newer.
        if (!claim.current()) return;
        set({ pendingPublishApprovals: replayApprovalEvents(pending, buffer) });
      } finally {
        if (approvalsInFlight === buffer) approvalsInFlight = null;
      }
    },

    async answerPublishApproval(id, approved) {
      // Dropped first, as on the desktop: the card must not linger if the op
      // fails, and a request the host has already timed out is a no-op there.
      set((s) => ({
        pendingPublishApprovals: s.pendingPublishApprovals.filter((r) => r.id !== id),
      }));
      await guard(() => api.answerPublishApproval(id, approved));
    },

    resolvePublishApproval(id) {
      approvalsInFlight?.push({ kind: "resolved", id });
      set((s) => ({
        pendingPublishApprovals: s.pendingPublishApprovals.filter((r) => r.id !== id),
      }));
    },

    async stop(agentId) {
      return guard(async () => {
        await api.stopAgent(agentId);
      });
    },

    async resume(agentId) {
      return guard(async () => {
        await api.resumeAgent(agentId);
      });
    },

    async archive(agentId) {
      const agent = agentOf(get(), agentId);
      return guard(async () => {
        // An archived purpose chat is in neither the snapshot nor
        // `list_project_chats`, so archiving one loses it with no way back.
        // `deleteChat` is the honest gesture for those; refuse here so no other
        // path can strand one.
        if (agent?.purpose) throw new Error("a planning chat is deleted, not archived");
        await api.archiveAgent(agentId);
        // Off its sheet and its screen, both the app's.
        const app = appStore.getState();
        app.closeSheet();
        app.pop();
        await get().refreshWorkspace();
      });
    },

    async setModel(agentId, model) {
      await guard(() => api.setAgentModel(agentId, model));
    },

    async setEffort(agentId, effort) {
      await guard(() => api.setAgentEffort(agentId, effort));
    },

    async delegateGit(agentId, action, params) {
      if (!get().hostSupports("delegate_git")) {
        // A host from before the op: send the trigger as the plain user turn it
        // always was. Nothing holds it while the agent is mid-turn, and nothing
        // tracks it — `agent:git-action` and `pr:state_changed` (events.ts) reload
        // the git/PR state when it lands.
        await get().send(agentId, appActionMessage(action, params));
        set((s) => ({ shipActivity: appendActivity(s.shipActivity, agentId, askedText(action)) }));
        return;
      }
      // The host's: it composes the trigger, holds it while the agent is
      // mid-turn and decides when it is over, so the delegation outlives this
      // phone's connection and reads the same on the Mac.
      await guard(async () => {
        const recorded = await api.delegateGit(agentId, action, params);
        // The `delegation:changed` for this carries the activity line and may
        // land either side of this reply; this only makes sure the strip turns at
        // once — and never revives one whose end already arrived.
        if (endedDelegations.get(agentId) === recorded.started_at) return;
        set((s) =>
          s.delegations[agentId] ? {} : { delegations: { ...s.delegations, [agentId]: recorded } },
        );
      });
    },

    applyDelegation(e) {
      if (e.subdir) return;
      const ended = e.phase === "done" || e.phase === "abandoned";
      if (ended) endedDelegations.set(e.agent_id, e.started_at);
      set((s) => {
        const line = delegationActivityText(s.delegations[e.agent_id], e);
        const { [e.agent_id]: _prev, ...rest } = s.delegations;
        return {
          delegations: ended ? rest : { ...rest, [e.agent_id]: e },
          ...(line ? { shipActivity: appendActivity(s.shipActivity, e.agent_id, line) } : {}),
        };
      });
    },

    async loadDelegations() {
      if (!get().hostSupports("get_delegations")) return;
      const rows = await api.getDelegations();
      set({
        delegations: Object.fromEntries(rows.filter((r) => !r.subdir).map((r) => [r.agent_id, r])),
      });
    },

    applyAutopilotState(row) {
      set((s) => ({
        autopilot: { ...s.autopilot, [checkoutKey(row.agent_id, row.subdir)]: row },
      }));
    },

    applyAutopilotSwitches(switches) {
      set({ autopilotSwitches: switchesOf(switches) });
    },

    applyAutopilotEvent(entry) {
      set((s) => ({
        shipActivity: mergeActivity(s.shipActivity, entry.agent_id, [autopilotActivity(entry)]),
      }));
    },

    async loadAutopilot() {
      await Promise.all([
        get().hostSupports("autopilot_state") &&
          api.getAutopilotState().then((snapshot) => {
            set({
              autopilot: autopilotFromSnapshot(snapshot),
              autopilotSwitches: switchesOf(snapshot),
            });
          }),
        // Merged rather than replaced: a live event may have landed first, and
        // it is the same row by id.
        get().hostSupports("autopilot_log") &&
          api.getAutopilotLog().then((rows) => {
            set((s) => ({
              shipActivity: rows.reduce(
                (map, row) => mergeActivity(map, row.agent_id, [autopilotActivity(row)]),
                s.shipActivity,
              ),
            }));
          }),
      ]);
    },

    async setAgentAutopilot(agentId, enabled) {
      if (!get().hostSupports("autopilot_set")) return;
      await guard(async () => {
        // The whole host's state after the change — the pause, and the cycle it
        // dropped — so the switch settles on the host's word, not the tap's.
        const snapshot = await api.setAutopilot({ agentId }, enabled);
        set({
          autopilot: autopilotFromSnapshot(snapshot),
          autopilotSwitches: switchesOf(snapshot),
        });
      });
    },

    async publish(agentId, title, body) {
      const git = get().gitStates[agentId];
      if (git?.files.length) await api.commitAgent(agentId, title);
      await api.pushAgent(agentId);
      const pr = await api.createPr(agentId, title, body);
      // The host's own answer: a read that went out before the PR existed must
      // not land after it and say there is none.
      prWrites.stamp("prStates", agentId);
      set((s) => ({ prStates: { ...s.prStates, [agentId]: pr } }));
      await get().loadGit(agentId);
    },

    async pushToPr(agentId) {
      const git = get().gitStates[agentId];
      const title = get().prStates[agentId]?.title ?? "Update";
      if (git?.files.length) await api.commitAgent(agentId, title);
      await api.pushAgent(agentId);
      await get().loadGit(agentId);
    },

    async mergePr(agentId) {
      return guard(async () => {
        // Read before the reload: afterwards the record says merged, and the
        // line is about the PR that was open when the tap landed.
        const number = get().prStates[agentId]?.number;
        await api.mergePr(agentId);
        set((s) => ({
          shipActivity: appendActivity(
            s.shipActivity,
            agentId,
            number != null ? `Merged PR #${number}` : "Merged the PR",
          ),
        }));
        await get().loadGit(agentId);
      });
    },

    async listDir(path) {
      return guard(() => api.listDir(path));
    },

    /** Both add-project ops answer with the whole new `Workspace`, which is
     *  applied here rather than waited for as a `workspace:changed`: it replaces
     *  the workspace wholesale, exactly as `refreshWorkspace` does, so an event
     *  landing either side of this leaves the same state. Only the Add Project
     *  sheet is closed on success — a slow clone must not dismiss a sheet the user
     *  opened in the meantime. */
    async addWorkspaceRepo(repoPath) {
      return guard(async () => {
        set({ workspace: await api.addWorkspaceRepo(repoPath) });
        appStore.getState().closeSheetIf("addProject");
      });
    },

    async cloneRepo(spec, destParent) {
      return guard(async () => {
        const workspace = await api.cloneRepo(spec, destParent);
        set({ workspace, lastDestParent: destParent });
        await saveDestParent(link.hostKey, destParent);
        appStore.getState().closeSheetIf("addProject");
      });
    },

    async ghStatus() {
      return guard(() => api.ghStatus());
    },

    async ghRepoList() {
      return guard(() => api.ghRepoList());
    },

    ...createChatsSlice(set, get, {
      link,
      lists: chatLists,
      sends,
      guard,
      firstTurn: (input) => firstTurn(set, get, input),
    }),

    ...createProposalsSlice(set, get, { link, guard }),

    ...createRoadmapSlice(set, get, link),

    ...createProviderAccountsSlice(set, get, { link, chatLists, workspaceReads }),
  }));

  registerRemoteEvents(link, store.setState, store.getState, {
    workspaceReads,
    chatLists,
    prWrites,
  });
  client.onSnapshot((snapshot) => {
    // The client has folded the host's relay into its target.
    store.setState({ relay: client.target?.relay ?? null });
    store.getState().applyHandshake(snapshot);
  });
  client.onState((state, error) => {
    store.setState({
      connection: state,
      connectionError: error ?? null,
      retrying: client.retrying,
      hostInfo: client.host,
      protocol: client.protocol,
      via: client.via,
    });
  });
  return store;
}

const stores = new WeakMap<HostLink, HostStore>();

/** The store over `link`: the same one every time, built the first time it is
 *  asked for. Kept by link rather than by host key, so the store of a pairing
 *  that does not know its host yet (`pairingLink`) is that host's store from
 *  the moment the link is that host's link — and a dropped link's store goes
 *  with it. */
export function storeOfLink(link: HostLink): HostStore {
  let store = stores.get(link);
  if (!store) {
    store = createHostStore(link);
    stores.set(link, store);
  }
  return store;
}

/** The store of `hostKey`'s host. */
export const hostStoreOf = (hostKey: string): HostStore => storeOfLink(linkOf(hostKey));

/** Read the store of the host this component acts on (`useHost`): the one a
 *  `HostContext` above it names, else the one on screen. */
export function useHostStore<T>(selector: (s: HostState) => T): T {
  return useStore(storeOfLink(useHost()), selector);
}
