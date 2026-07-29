// The app's single owner of delegation lifecycles.
//
// While the agent holds a delegation, something has to watch for the transition
// that marks it done — and decide, when the agent settles without one, whether
// that's a finished job or an abandoned one. The per-delegation step decision is
// pure (`delegationStep`); `planDelegationPass` below is the pure sweep across
// every in-flight delegation, and the hook is a thin applier over it.
//
// This used to live in `GitRepoSection`, which meant a delegation only advanced
// while its repo's panel section happened to be mounted — i.e. only for the
// selected agent, on the Git tab. Everything else stalled:
//
//   - A delegation dispatched from Mission Control (`approveAgent`, `updateAll`)
//     targets an agent that is usually NOT the selected one, so nothing watched
//     it at all. For a *running* agent that was outright broken: the trigger is
//     held pending `dequeue`, and with no watcher the dequeue never fired, so
//     the agent was never actually asked to do the work.
//   - Switching agents mid-delegation dropped the watcher, so the delegation
//     neither resolved nor gave up — it just sat in the store.
//
// Mounted once at the app root, this watches every in-flight delegation
// regardless of what's on screen. `gitSync` feeds it by polling delegated
// checkouts alongside the focused one — a watcher without fresh state would read
// every unfocused delegation as abandoned.

import { useEffect } from "react";
import type { AgentStatus, GitState, PrChecks, PrState } from "@/api";
import { type Delegation, delegationDone, delegationResolved, delegationStep } from "@/delegation";
import { EMPTY_AGENTS, useAppStore } from "@/store";
import { splitCheckoutKey } from "./git";

/** What one pass decided to do about a single delegation. Mirrors
 *  `DelegationStep`, resolved into the store call it implies (and carrying the
 *  copy, so the applier holds no policy of its own). `wait` isn't represented —
 *  a pass emits only the delegations that need something done. */
export type DelegationEffect =
  | { do: "resolve"; key: string; agentId: string; subdir?: string; notice: string }
  | { do: "dequeue"; key: string }
  | { do: "mark-running"; key: string }
  | { do: "give-up"; key: string; notice: string }
  | { do: "drop-orphan"; key: string };

export interface DelegationPassInput {
  delegations: Record<string, Delegation>;
  /** Live agent status by agent id. A missing entry means the agent is gone
   *  (archived / discarded), which orphans its delegations. */
  statuses: Record<string, AgentStatus>;
  /** Per-checkout state, keyed by `checkoutKey` — the same maps the store holds. */
  gitStates: Record<string, GitState>;
  prStates: Record<string, PrState | null>;
  prChecks: Record<string, PrChecks | null>;
  now: number;
}

/** Decide what to do about every in-flight delegation this tick. Pure, so the
 *  interesting cases — a delegation on an unfocused agent, two checkouts of one
 *  agent, a settled turn — are testable without a rendered panel. */
export function planDelegationPass(input: DelegationPassInput): DelegationEffect[] {
  const { delegations, statuses, gitStates, prStates, prChecks, now } = input;
  const effects: DelegationEffect[] = [];
  // At most one dequeue per agent per pass. Two delegations on one agent (two
  // checkouts) can both be queued behind the same turn; delivering both would
  // coalesce the second into the first's turn, which is the exact failure
  // `queued` exists to prevent. The loser stays queued and, now that the agent
  // is running again, waits out the turn it just triggered.
  const dequeued = new Set<string>();

  for (const [key, delegation] of Object.entries(delegations)) {
    const { agentId } = splitCheckoutKey(key);
    const status = statuses[agentId];
    // The agent was archived or discarded under us — drop the orphan rather
    // than watching a lifecycle whose subject is gone.
    if (!status) {
      effects.push({ do: "drop-orphan", key });
      continue;
    }
    // Judged against THIS checkout's state, which is why delegations are keyed
    // per checkout: a multi-repo agent's two delegations reach their targets
    // independently.
    const resolved = delegationResolved(
      delegation.kind,
      gitStates[key] ?? null,
      prStates[key] ?? null,
      prChecks[key] ?? null,
    );
    switch (delegationStep(delegation, status, resolved, now)) {
      case "resolve":
        effects.push({
          do: "resolve",
          key,
          agentId,
          subdir: delegation.subdir,
          notice: delegationDone(delegation.kind),
        });
        break;
      case "dequeue":
        if (dequeued.has(agentId)) break;
        dequeued.add(agentId);
        effects.push({ do: "dequeue", key });
        break;
      case "mark-running":
        effects.push({ do: "mark-running", key });
        break;
      case "give-up":
        effects.push({
          do: "give-up",
          key,
          // `fix-checks` never resolves from state (CI takes minutes), so a
          // settled agent is its NORMAL ending, not an abandonment — say so.
          notice:
            delegation.kind === "fix-checks"
              ? delegationDone("fix-checks")
              : "Agent finished — review the chat for details",
        });
        break;
      case "wait":
        break;
    }
  }
  return effects;
}

/** Mount once, at the app root. */
export function useDelegationSync() {
  const delegations = useAppStore((s) => s.delegations);
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);
  const gitStates = useAppStore((s) => s.gitStates);
  const prStates = useAppStore((s) => s.prStates);
  const prChecks = useAppStore((s) => s.prChecks);

  const markDelegationRunning = useAppStore((s) => s.markDelegationRunning);
  const markDelegationDequeued = useAppStore((s) => s.markDelegationDequeued);
  const clearDelegation = useAppStore((s) => s.clearDelegation);
  const noteDelegationOutcome = useAppStore((s) => s.noteDelegationOutcome);
  const fetchPrChecks = useAppStore((s) => s.fetchPrChecks);

  useEffect(() => {
    // Re-evaluated on every store tick that touches these maps, which is what
    // gives the give-up clock its pulse: `gitSync` polls delegated checkouts, so
    // `gitStates` changes at least once a second while any delegation is live.
    // No timer of its own — with no delegations this does nothing at all.
    if (Object.keys(delegations).length === 0) return;
    const statuses: Record<string, AgentStatus> = {};
    for (const a of agents) statuses[a.id] = a.status;

    for (const effect of planDelegationPass({
      delegations,
      statuses,
      gitStates,
      prStates,
      prChecks,
      now: Date.now(),
    })) {
      switch (effect.do) {
        case "resolve":
          clearDelegation(effect.key);
          noteDelegationOutcome(effect.key, effect.notice);
          // A fresh PR (or branch update) changes the merge gate — refresh now
          // rather than waiting out the slow poll.
          void fetchPrChecks(effect.agentId, effect.subdir);
          break;
        case "give-up":
          clearDelegation(effect.key);
          noteDelegationOutcome(effect.key, effect.notice);
          break;
        case "drop-orphan":
          clearDelegation(effect.key);
          break;
        case "dequeue":
          markDelegationDequeued(effect.key);
          break;
        case "mark-running":
          markDelegationRunning(effect.key);
          break;
      }
    }
  }, [
    delegations,
    agents,
    gitStates,
    prStates,
    prChecks,
    markDelegationRunning,
    markDelegationDequeued,
    clearDelegation,
    noteDelegationOutcome,
    fetchPrChecks,
  ]);
}
