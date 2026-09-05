// The autopilot driver: one tick, every live checkout of every project that has
// autopilot on (the default — see `autopilotDisabledProjects`).
//
// Mounted once at the app root, like `useGitSync` and `useDelegationSync`. The
// decision for each checkout is pure (`autopilotStep`); this hook is the applier
// — it turns effects into store transitions, delegations and verification calls.
//
// ── Known limitation, stated plainly ────────────────────────────────────────
// This runs in the webview, and `usePoll` stops when `document.hidden` (see
// util/hooks.ts). Closing the Fletch window hides it — the app and its agents
// keep running, but this loop does not. So autopilot progresses only while the
// window is open. Moving the loop into the supervisor is what fixes that;
// `autopilot.ts` and `readiness.ts` are written to be portable for exactly that
// reason, and their tests enforce it.

import { useCallback, useRef } from "react";
import { useShallow } from "zustand/react/shallow";
import { type AgentRecord, api } from "@/api";
import { type AutopilotEffect, type AutopilotState, autopilotStep } from "@/autopilot";
import { appActionMessage } from "@/delegation";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";
import { autopilotProjectOn } from "./autopilot";
import type { AutopilotLogEntry } from "./autopilotLog";
import { checkoutKey, splitCheckoutKey } from "./git";

/** How often to evaluate enrolled checkouts. Slower than the git poll on
 *  purpose: every action this can take costs an agent turn, so there is nothing
 *  to gain from reacting inside a second, and a slow tick makes accidental
 *  double-dispatch structurally unlikely. */
const AUTOPILOT_TICK_MS = 10_000;

/** The checkouts one pass should look at: every checkout of every live agent
 *  whose project has autopilot on, plus anything still tracked in `autopilot` —
 *  so an entry whose agent is gone, or whose project was just switched off, gets
 *  visited once more and dropped rather than lingering.
 *
 *  The primary repo (index 0) keeps the plain agent id, exactly as the Git panel
 *  keys it (`checkoutScopes` in GitPanel/index.tsx); secondaries get `::subdir`.
 *  Sorted so the tick keeps a stable identity across unrelated store writes.
 *
 *  `disabledProjects === null` means the opt-outs haven't loaded (or failed to):
 *  nothing is swept, because "on by default" without knowing who opted out would
 *  act on exactly the projects that said no (see `autopilotProjectOn`). */
export function autopilotKeys(
  agents: readonly AgentRecord[],
  tracked: Record<string, AutopilotState>,
  disabledProjects: readonly string[] | null,
): string[] {
  if (disabledProjects === null) return [];
  const keys = new Set(Object.keys(tracked));
  for (const agent of agents) {
    if (!autopilotProjectOn(disabledProjects, agent.project_id)) continue;
    for (const [i, repo] of (agent.repos ?? []).entries()) {
      keys.add(checkoutKey(agent.id, i === 0 ? undefined : repo.subdir));
    }
  }
  return [...keys].sort();
}

/** Mount once, at the app root. */
export function useAutopilotSync() {
  const keys = useAppStore(
    useShallow((s) =>
      autopilotKeys(s.workspace?.agents ?? [], s.autopilot, s.autopilotDisabledProjects),
    ),
  );

  // Verification is an async round-trip that outlives a tick; without this a slow
  // verify would be re-issued every 10s for the same cycle.
  const verifying = useRef<Set<string>>(new Set());

  const tick = useCallback(() => autopilotPass(keys, verifying.current), [keys]);

  usePoll(tick, AUTOPILOT_TICK_MS, [tick]);
}

/** One sweep over the given checkouts, applying what the policy decided.
 *
 *  Exported so the WIRING is testable without a rendered hook: the decision is
 *  pure and covered by `autopilot.test.ts`, but the store transitions, the
 *  delegation it sends and the per-agent/per-checkout guards below only exist
 *  here. Same reason `planDelegationPass` is exported from `delegationSync`.
 *
 *  `verifying` is owned by the caller (a ref on the hook) because it must
 *  outlive a single pass — see `useAutopilotSync`. */
export async function autopilotPass(keys: string[], verifying: Set<string>) {
  // Agents that were handed a rung earlier in THIS pass. A dispatch sends the
  // agent a message, but the resulting flip to `running` arrives asynchronously
  // from the backend, so for the rest of this pass the snapshot still reads
  // `idle`. Two checkouts of one multi-repo agent (`a1` and `a1::web`) would
  // both see an idle agent and both dispatch, and Claude would coalesce the two
  // triggers into ONE turn — the exact failure `queued` exists to prevent, and
  // the one `delegationInFlight` cannot catch because it is keyed per checkout
  // and the sibling has no delegation of its own. So an agent already dispatched
  // to counts as busy, which also stops a sibling cycle from being verified
  // while a turn we just started rewrites the tree under it. The loser waits for
  // the next tick, by which time the real status has caught up. Mirrors the
  // per-pass `dequeued` set in `planDelegationPass`.
  const dispatchedTo = new Set<string>();

  for (const key of keys) {
    const s = useAppStore.getState();
    const { agentId, subdir } = splitCheckoutKey(key);
    const agent = s.workspace?.agents.find((a) => a.id === agentId);
    // The checkout's agent is gone (archived/discarded), or its project switched
    // autopilot off — drop the entry rather than ticking forever against nothing.
    // A project that comes back on starts its checkouts fresh on the next tick.
    if (!agent || !autopilotProjectOn(s.autopilotDisabledProjects, agent.project_id)) {
      if (s.autopilot[key]) s.unenrollAutopilot(key);
      continue;
    }
    // Autopilot is on by default: a checkout the driver hasn't seen yet is
    // enrolled here, on its first tick, rather than by a click.
    if (!s.autopilot[key]) s.enrollAutopilot(key);
    const state = useAppStore.getState().autopilot[key];
    const git = s.gitStates[key] ?? null;
    const readiness = {
      git,
      pr: s.prStates[key] ?? null,
      checks: s.prChecks[key] ?? null,
      comments: s.prComments[key] ?? null,
    };
    const now = Date.now();
    const effect = autopilotStep({
      state,
      readiness,
      ladder: { base: git?.parent_branch || "main", commitMode: "commit-pr" },
      agentBusy:
        agent.status === "running" || agent.status === "spawning" || dispatchedTo.has(agentId),
      delegationInFlight: s.delegations[key] != null,
      // Only a verdict produced FOR the current cycle counts as its evidence
      // (cleared on dispatch); see `autopilotVerdicts`.
      verification: s.autopilotVerdicts[key] ?? null,
      now,
    });
    if (effect.do === "dispatch") dispatchedTo.add(agentId);
    await apply(key, agentId, subdir, effect, now, verifying);
  }
}

/** Perform one effect. Split out so the tick reads as the sweep it is. */
async function apply(
  key: string,
  agentId: string,
  subdir: string | undefined,
  effect: AutopilotEffect,
  now: number,
  verifying: Set<string>,
) {
  const s = useAppStore.getState();
  // Log the four effects a user would want to know about (see `autopilotLog`).
  // `wait` is most ticks and would bury them; `verify` / `await-evidence` are
  // steps inside a cycle whose outcome already reports how it went. Each call
  // sits where the attempt number is correct — a dispatch has no cycle until
  // `openAutopilotCycle` creates one, while settle/retry read the cycle the
  // action they precede is about to clear. Stamped with the pass's `now` rather
  // than a fresh clock read, so an entry's time matches the decision behind it.
  const log = (entry: Omit<AutopilotLogEntry, "at">) =>
    useAppStore.getState().recordAutopilotEvent(key, { at: now, ...entry });
  const attemptNow = () => useAppStore.getState().autopilot[key]?.cycle?.attempt;

  switch (effect.do) {
    case "dispatch": {
      s.openAutopilotCycle(key, effect.rung, effect.signature);
      log({ outcome: "dispatch", rung: effect.rung, attempt: attemptNow() });
      // Same trigger construction the panel and Mission Control use, including
      // the `repo=` scope for a secondary checkout.
      s.delegateAction(
        agentId,
        effect.rung,
        appActionMessage(
          effect.action,
          subdir ? { ...effect.params, repo: subdir } : effect.params,
        ),
        subdir,
      );
      return;
    }
    case "await-evidence":
      // A state-judged rung (a reconcile): nothing to run, the world just needs
      // time to settle. Start the phase clock so the evidence timeout applies.
      s.advanceAutopilotCycle(key, "awaiting-evidence", now);
      return;
    case "verify": {
      // Enter `awaiting-evidence` first: the phase clock starts now, and the
      // effect is idempotent from here even if the verify call is slow or fails.
      s.advanceAutopilotCycle(key, "awaiting-evidence", now);
      if (verifying.has(key)) return;
      verifying.add(key);
      try {
        // Recorded per checkout so the next tick reads it as THIS cycle's
        // evidence.
        s.recordAutopilotVerdict(key, await api.runVerification(agentId, subdir));
      } catch {
        // No local verdict available — the cycle falls back to judging on CI,
        // bounded by the evidence timeout. A verify that couldn't run must never
        // look like a fix that didn't work.
      } finally {
        verifying.delete(key);
      }
      return;
    }
    case "settle":
      log({ outcome: "settle", rung: effect.rung, attempt: attemptNow() });
      s.settleAutopilotCycle(key, effect.rung);
      return;
    case "retry":
      log({ outcome: "retry", rung: effect.rung, attempt: attemptNow() });
      s.retryAutopilotCycle(key, effect.rung, effect.barren);
      return;
    case "escalate":
      log({ outcome: "escalate", rung: effect.rung, reason: effect.reason });
      s.markAutopilotStuck(key, effect.reason, effect.rung, now, effect.blockers);
      return;
    case "revive":
      // Recorded, because "it picked this back up on its own" is exactly the kind
      // of unattended action the audit trail exists to explain.
      log({ outcome: "revive", rung: null });
      s.reviveAutopilot(key);
      return;
    case "wait":
      return;
  }
}
