// Moving the UI from one engine to another: park what the environment being
// left was showing, point everything at the new one, put back what it was
// showing, then ask it for the truth.
//
// It lives beside ./environments rather than in it because it needs @/api, and
// api/transport reads the active environment out of that module — importing the
// other way would close the cycle (see the note at the bottom of
// ./environments).
//
// Why a stash rather than a namespace per map. Agent ids are recycled place
// names, so two hosts will both have an agent called `fuji`
// (docs/multi-host-plan.md §1.3), and nearly every per-agent map in the store
// would cross-wire on that. Namespacing each one means finding and changing
// every key-construction site across a dozen slices; parking the whole set on
// the entry is one list, in one place, and it is also what makes switching back
// restore the previous view exactly rather than re-fetch it.
//
// What is NOT stashed, and why:
//   - PTY ring buffers and the live-terminal cache already key on the
//     environment themselves (src/pty/buffers, src/pty/terminals), so they
//     survive a switch untouched.
//   - `drafts` are stashed but need no namespacing: a draft id is a generated
//     `draft-<ts>-<rand>`, and nothing persists them.
//   - The persisted per-agent and per-project autopilot opt-outs
//     (`autopilotPausedAgents`, `autopilotDisabledProjects`) live in THIS Mac's
//     database and describe its own engine. They are neither stashed nor reset,
//     because autopilot never runs against a remote environment: its rungs need
//     `run_verification`, `fork_agent` and the local `project_settings` table,
//     none of which are on the wire (docs/remote-protocol.md's op table).
//     What autopilot builds *per checkout* — `autopilot`, `autopilotVerdicts`,
//     `autopilotLog` — IS stashed, for the reason above: those maps are keyed by
//     `agentId::subdir`, so a local checkout's enrolment would otherwise answer
//     for the same-named agent on a host (and `publishPreAuthorized` reads it to
//     auto-approve a push).
//   - Provider versions, model catalogs, installs, container builds, the custom
//     agent / skill / MCP libraries, the account and the appearance settings
//     are properties of this desktop, keyed by provider or runtime.
//   - `prWriteOrder`'s high-water marks are keyed by checkout, but its ticket
//     counter is globally monotonic: every write issued after a switch outranks
//     every one issued before it, so a shared key cannot reject a fresh write.

import type { EnvironmentId } from "./environments";
import { detachEventListeners, registerEventListeners } from "./eventListeners";
import { interruptedAgents } from "./interrupted";
import { clearPendingHides } from "./pendingHides";
import { refreshWorkspace } from "./refreshWorkspace";
import type { AppState, SliceCreator } from "./types";

/** What one environment is showing, as store keys. Everything here is keyed by
 *  something that belongs to a single engine — an agent id, a checkout
 *  (`agentId::subdir`), a repo path, or the workspace snapshot itself. */
const STASH_KEYS = [
  // The snapshot, and what is selected in it. `selectedAgentId` is not here:
  // it is `lastSelectedAgentId` on the entry, so the switcher can read it.
  "workspace",
  "selectedRunId",
  "focusedStepAgentId",
  "activeDraftId",
  "historyOpen",
  "selectedHistoryAgentId",
  "projectScreenRepoPath",
  "attendedChatId",
  // Per-agent chat state.
  "offSidebarAgents",
  "managedLogs",
  "pendingToolUse",
  "backgroundTasks",
  "transcriptLoading",
  "transcriptLoaded",
  "managedBusy",
  "managedBusyLabel",
  "turnStartedAt",
  "switchInFlight",
  "unseenResults",
  "syncHealth",
  "usage",
  "runPhases",
  "runPorts",
  // Per-checkout git / PR / delegation state. All of it is polled, so a stale
  // stash is corrected within one cadence of switching back.
  "gitStates",
  "gitShortstats",
  "gitMeta",
  "prStates",
  "prChecks",
  "prComments",
  "delegations",
  "delegationNotices",
  "verificationReports",
  // Per-checkout autopilot: what it is tracking, the verdict it is judging the
  // open cycle by, and the history it wrote.
  "autopilot",
  "autopilotVerdicts",
  "autopilotLog",
  // What the user typed but did not send, per agent or per draft, and the
  // drafts themselves (grouped by repo path, which is the engine's).
  "composerSeeds",
  "composerDrafts",
  "drafts",
  // Remembered right-rail tab per agent.
  "rightPanelTabs",
] as const;

type StashKey = (typeof STASH_KEYS)[number];

/** The state an environment the user has never visited starts in — the values
 *  the slices themselves initialise with. A restore spreads the stash over
 *  this, so a key the stash happens not to carry lands empty rather than
 *  leaking from the environment being left.
 *
 *  Typed as `Pick<AppState, StashKey>`, so adding a key to `STASH_KEYS`
 *  without giving it an empty value here is a compile error. */
const BLANK: Pick<AppState, StashKey> = {
  workspace: null,
  selectedRunId: null,
  focusedStepAgentId: null,
  activeDraftId: null,
  historyOpen: false,
  selectedHistoryAgentId: null,
  projectScreenRepoPath: null,
  attendedChatId: null,
  offSidebarAgents: {},
  managedLogs: {},
  pendingToolUse: {},
  backgroundTasks: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  managedBusy: {},
  managedBusyLabel: {},
  turnStartedAt: {},
  switchInFlight: {},
  unseenResults: {},
  syncHealth: {},
  usage: {},
  runPhases: {},
  runPorts: {},
  gitStates: {},
  gitShortstats: {},
  gitMeta: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  delegations: {},
  delegationNotices: {},
  verificationReports: {},
  autopilot: {},
  autopilotVerdicts: {},
  autopilotLog: {},
  composerSeeds: {},
  composerDrafts: {},
  drafts: [],
  rightPanelTabs: {},
};

const stashOf = (s: AppState): Partial<AppState> => {
  const out: Record<string, unknown> = {};
  for (const key of STASH_KEYS) out[key] = s[key];
  return out as Partial<AppState>;
};

export interface EnvironmentSwitchSlice {
  /** Point the UI at another environment. A no-op for the one already active,
   *  and for an id that is not there — a host forgotten between the render and
   *  the click. */
  switchEnvironment: (id: EnvironmentId) => Promise<void>;
  /** A handshake landed for `id`: the first one, or a reconnect — both mean
   *  this client has been out of touch and its live events are behind. Ignored
   *  for an environment that isn't on screen; a background host's snapshot is
   *  nobody's view, and the switch to it will refresh anyway. */
  environmentReconnected: (id: EnvironmentId) => void;
}

export const createEnvironmentSwitchSlice: SliceCreator<EnvironmentSwitchSlice> = (set, get) => {
  /** Re-read the selected agent's conversation from the host, the way the
   *  phone does on every reconnect: a dropped socket silently misses the live
   *  events a log is built from, and the records are the authority.
   *
   *  Mid-turn is the exception — the host only ingests a turn's transcript at
   *  turn end, so rebuilding now would replace what the stream rendered with
   *  the prompt alone. `loadHistoryTranscript` runs the same `sync_session` →
   *  `read_session_records` pair as the phone's `rebuildLog`. */
  const resyncSelectedAgent = async () => {
    const agentId = get().selectedAgentId;
    if (!agentId) return;
    const midTurn = get().managedBusy[agentId] && get().managedLogs[agentId] !== undefined;
    if (midTurn) return;
    await get().loadHistoryTranscript(agentId);
  };

  return {
    switchEnvironment: async (id) => {
      const from = get().activeEnvironmentId;
      if (id === from || !get().environments[id]) return;

      set((s) => {
        const leaving = s.environments[from];
        const target = s.environments[id];
        return {
          environments: {
            ...s.environments,
            ...(leaving
              ? {
                  [from]: {
                    ...leaving,
                    stash: stashOf(s),
                    lastSelectedAgentId: s.selectedAgentId,
                  },
                }
              : {}),
          },
          activeEnvironmentId: id,
          ...BLANK,
          ...target?.stash,
          selectedAgentId: target?.lastSelectedAgentId ?? null,
        };
      });

      // The two module-level sets keyed by a bare agent id. Both are transient
      // and only ever consumed by an event from the environment that set them
      // — an event this client stops hearing the moment it switches away — so
      // clearing them beats namespacing: left in place, a suppressed chime or a
      // hidden row would land on the same-named agent of the other host.
      interruptedAgents.clear();
      clearPendingHides();

      // The subscriptions resolved `activeTransport()` when they were made, so
      // they are still folding the environment we just left. Make them again.
      await detachEventListeners();
      await registerEventListeners(set, get);

      // A second switch while this one awaited the re-attach owns the state
      // now; ours must not write over it.
      if (get().activeEnvironmentId !== id) return;

      // Then the truth. A host that is not connected rejects this at once (the
      // client has no socket to write to), leaving the stash — or an empty view
      // — on screen with the connection state beside it in the switcher.
      // Issued unconditionally so it always claims the newest refresh
      // generation, which is what drops a `get_workspace` still in flight
      // against the environment we left.
      await refreshWorkspace(set).catch(() => {});
    },

    environmentReconnected: (id) => {
      if (get().activeEnvironmentId !== id) return;
      void (async () => {
        await refreshWorkspace(set).catch(() => {});
        await resyncSelectedAgent().catch(() => {});
      })();
    },
  };
};
