// Gated publishes: a publish the host holds until a person answers it
// (`publish:approval-requested`), drawn by the agent's chat as an Approve / Deny
// card and answered over the host's link.

import type { PublishApproval } from "@desktop/api/types/sandbox";
import { newestWins } from "@desktop/util/newestWins";
import { type ApprovalEvent, replayApprovalEvents } from "@desktop/util/publishApprovals";
import type { HostLink } from "../host";
import type { HostState } from "./host";

type Set = (partial: Partial<HostState> | ((s: HostState) => Partial<HostState>)) => void;
type Get = () => HostState;

export interface ApprovalsSlice {
  /** Gated publishes waiting on an answer, oldest first. The host is blocked on
   *  each one until it is answered or its wait lapses. */
  pendingPublishApprovals: PublishApproval[];
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
}

/** What the slice borrows from the store it composes into, so it can be a module
 *  of its own without importing the store back. */
export interface ApprovalsDeps {
  /** The link of the host the slice acts on. */
  link: HostLink;
  /** Record a failure where the error UI can see it, then rethrow. */
  guard<T>(fn: () => Promise<T>): Promise<T>;
}

export function createApprovalsSlice(set: Set, get: Get, deps: ApprovalsDeps): ApprovalsSlice {
  const { link, guard } = deps;

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

  return {
    pendingPublishApprovals: [],

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
        const pending = await link.api.listPublishApprovals();
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
      await guard(() => link.api.answerPublishApproval(id, approved));
    },

    resolvePublishApproval(id) {
      approvalsInFlight?.push({ kind: "resolved", id });
      set((s) => ({
        pendingPublishApprovals: s.pendingPublishApprovals.filter((r) => r.id !== id),
      }));
    },
  };
}
