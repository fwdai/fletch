// The host's autopilot, mirrored (docs/remote-protocol.md, "Autopilot"): one row
// per checkout, replaced wholesale by a snapshot (`autopilot_state`,
// `autopilot_set`) and row by row by `autopilot:state`. The host decides
// everything; the phone renders it and flips an agent's pause. Beside it, the
// host's git delegations, which the Ship strip reads the same way.

import type {
  AutopilotCheckout,
  AutopilotLogEntry,
  AutopilotSnapshot,
  AutopilotSwitches,
  DelegationEvent,
} from "@desktop/api/types/git";
import { appActionMessage } from "@desktop/delegation";
import type { HostLink } from "../host";
import type { HostState } from "./host";
import {
  appendActivity,
  askedText,
  autopilotActivity,
  delegationActivityText,
  mergeActivity,
} from "./shipActivity";

type Set = (partial: Partial<HostState> | ((s: HostState) => Partial<HostState>)) => void;
type Get = () => HostState;

/** Checkout key → its row. */
export type AutopilotMap = Record<string, AutopilotCheckout>;

/** `agentId` for the primary repo, `agentId::subdir` for a secondary — the
 *  desktop's `checkoutKey`, and the keys `get_all_pr_status` answers with. */
export const checkoutKey = (agentId: string, subdir: string | null): string =>
  subdir ? `${agentId}::${subdir}` : agentId;

export const autopilotFromSnapshot = (snapshot: AutopilotSnapshot): AutopilotMap =>
  Object.fromEntries(snapshot.checkouts.map((c) => [checkoutKey(c.agent_id, c.subdir), c]));

/** Just the two opt-out lists of a snapshot or an `autopilot:switches`. */
export const switchesOf = ({ disabled_projects, paused_agents }: AutopilotSwitches) => ({
  disabled_projects,
  paused_agents,
});

/** One agent's checkouts, the primary first and the rest in the host's order. */
export const agentCheckouts = (map: AutopilotMap, agentId: string): AutopilotCheckout[] =>
  Object.values(map)
    .filter((c) => c.agent_id === agentId)
    .sort((a, b) => Number(a.subdir !== null) - Number(b.subdir !== null));

export interface AutopilotSlice {
  /** The host's live delegation per agent (`delegation:changed`), primary repo
   *  only — the phone is single-repo for now. A mirror: the host decides when
   *  one starts, runs and ends; the Ship strip reads `working` off it. Re-read
   *  with `get_delegations` on every handshake. */
  delegations: Record<string, DelegationEvent>;
  /** The host's autopilot per checkout, every checkout of every live agent. A
   *  mirror like `delegations`: `autopilot:state` replaces a row, a snapshot
   *  (`autopilot_state` on every handshake, `autopilot_set`'s answer) replaces
   *  them all. Empty on a host without the ops. */
  autopilot: AutopilotMap;
  /** The host's two autopilot opt-out lists, whole: from every snapshot, and
   *  replaced by `autopilot:switches` — which every `autopilot_set` fires, even
   *  on a project with no agents (so no row changes). `null` until read. */
  autopilotSwitches: AutopilotSwitches | null;
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
}

/** What the slice borrows from the store it composes into, so it can be a module
 *  of its own without importing the store back. */
export interface AutopilotDeps {
  /** The link of the host the slice acts on. */
  link: HostLink;
  /** Record a failure where the error UI can see it, then rethrow. */
  guard<T>(fn: () => Promise<T>): Promise<T>;
}

export function createAutopilotSlice(set: Set, get: Get, deps: AutopilotDeps): AutopilotSlice {
  const { link, guard } = deps;
  const { api } = link;

  /** The `started_at` of the last delegation that ended per agent, so a
   *  `delegate_git` reply landing after its own end cannot put it back. */
  const endedDelegations = new Map<string, number>();

  return {
    delegations: {},
    autopilot: {},
    autopilotSwitches: null,

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
  };
}
