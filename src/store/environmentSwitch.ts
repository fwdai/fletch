// Which host is on screen, and what a host's handshake re-reads.
//
// Every connected host's state is live in the store at once (./hostState), so
// putting another one on screen is only a change of which one the main pane is
// scoped to: nothing is parked, restored or re-read, and no listener moves.
//
// It lives beside ./environments rather than in it because it needs @/api, and
// api/engine reads the environments out of that module — importing the other
// way would close the cycle (see the note at the bottom of ./environments).

import { isAgentBusy } from "@/helpers";
import type { EnvironmentId } from "./environments";
import { hydrateHostSettings } from "./hostSettings";
import { refreshWorkspace } from "./refreshWorkspace";
import type { SliceCreator } from "./types";

export interface EnvironmentSwitchSlice {
  /** Put another host on screen: the main pane, and what decides what it
   *  shows, move to its scope. The primitive under `onHost`, which every
   *  selection that belongs to a host goes through. A no-op for the host
   *  already there, and for an id that is not there — a host forgotten
   *  between the render and the click. */
  switchEnvironment: (id: EnvironmentId) => void;
  /** A handshake landed for `id`: the first one, or a reconnect — both mean
   *  this client has been out of touch and the host's live events are behind,
   *  so its state is re-read whether or not it is on screen. Run through the
   *  host's own actions (`hostStore(id)`); this desktop's, or another host's,
   *  ignore an id that is not theirs. */
  environmentReconnected: (id: EnvironmentId) => void;
}

export const createEnvironmentSwitchSlice: SliceCreator<EnvironmentSwitchSlice> = (
  set,
  get,
  engine,
) => {
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
    const midTurn = isAgentBusy(get(), agentId) && get().managedLogs[agentId] !== undefined;
    if (midTurn) return;
    await get().loadHistoryTranscript(agentId);
  };

  /** Ask the host for everything it owns the answer to. One list, so nothing
   *  a handshake should refresh is left to chance — hand-listing the reads at
   *  each entry point is how `github` once came to be refreshed at none.
   *
   *  Each is best-effort on its own: a host that is not connected rejects at
   *  once (the client has no socket to write to), which leaves what it last
   *  said on screen with the connection state on its rows' host chip, rather
   *  than stopping the reads that follow. */
  const resyncEnvironment = async () => {
    await refreshWorkspace(engine, set).catch(() => {});
    // What this host is blocked on, which no event will tell a client that
    // was not connected when it was raised.
    await get()
      .loadPendingPublishApprovals()
      .catch(() => {});
    await get()
      .loadPendingEgressPrompts()
      .catch(() => {});
    // The delegations it is running, which it owns: a mirror built from
    // events this client missed is not the truth.
    await get()
      .loadDelegations()
      .catch(() => {});
    // Its autopilot likewise: switches, open cycles and history are all its own.
    await get()
      .loadAutopilot()
      .catch(() => {});
    // …and the local reviews it is running, which it owns the same way.
    await get()
      .loadReviews()
      .catch(() => {});
    // …and whose GitHub login gates its push / PR / clone affordances.
    await get()
      .refreshGithub()
      .catch(() => {});
    // Its PR badges, once that login is known (the read is GitHub-gated). The
    // host watcher's events keep them current from here, but they only reach a
    // client connected when they fired; closed PRs get a live look too, since
    // nothing else re-checks one that reopened.
    await get()
      .loadAllPrStatus(true)
      .catch(() => {});
    // …and the settings it reads (./hostSettings). Not awaited: nothing after
    // it depends on it, and an answer older than a later read of the same host
    // is dropped by `hydrateHostSettings` itself.
    // …and where its Linux VM stands, which its `box:*` events keep current
    // from here.
    void hydrateHostSettings(engine, set)
      .then(() => get().loadBoxSetup())
      .catch(() => {});
  };

  return {
    switchEnvironment: (id) => {
      const s = get();
      if (id === s.activeEnvironmentId || !s.environments[id]) return;
      set({ activeEnvironmentId: id });
    },

    environmentReconnected: (id) => {
      if (id !== engine.hostId) return;
      void (async () => {
        await resyncEnvironment();
        // The live events the open agent's log is built from may have been
        // missed while the socket was down.
        await resyncSelectedAgent().catch(() => {});
      })();
    },
  };
};
