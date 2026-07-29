// The autopilot driver: one tick, every enrolled checkout.
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
import { api } from "@/api";
import { type AutopilotEffect, autopilotStep } from "@/autopilot";
import { appActionMessage } from "@/delegation";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";
import { splitCheckoutKey } from "./git";

/** How often to evaluate enrolled checkouts. Slower than the git poll on
 *  purpose: every action this can take costs an agent turn, so there is nothing
 *  to gain from reacting inside a second, and a slow tick makes accidental
 *  double-dispatch structurally unlikely. */
const AUTOPILOT_TICK_MS = 10_000;

/** Mount once, at the app root. */
export function useAutopilotSync() {
  // Only enrolled checkouts are ever considered. Sorted + shallow-compared so the
  // tick keeps a stable identity across unrelated store writes.
  const keys = useAppStore(
    useShallow((s) =>
      Object.entries(s.autopilot)
        .filter(([, a]) => a.enrolled)
        .map(([key]) => key)
        .sort(),
    ),
  );

  // Verification is an async round-trip that outlives a tick; without this a slow
  // verify would be re-issued every 10s for the same cycle.
  const verifying = useRef<Set<string>>(new Set());

  const tick = useCallback(async () => {
    for (const key of keys) {
      const s = useAppStore.getState();
      const { agentId, subdir } = splitCheckoutKey(key);
      const agent = s.workspace?.agents.find((a) => a.id === agentId);
      // The checkout's agent is gone (archived/discarded) — drop the enrollment
      // rather than ticking forever against nothing.
      if (!agent) {
        s.unenrollAutopilot(key);
        continue;
      }
      const git = s.gitStates[key] ?? null;
      const readiness = {
        git,
        pr: s.prStates[key] ?? null,
        checks: s.prChecks[key] ?? null,
        comments: s.prComments[key] ?? null,
      };
      const now = Date.now();
      const effect = autopilotStep({
        state: s.autopilot[key],
        readiness,
        ladder: { base: git?.parent_branch || "main", commitMode: "commit-pr" },
        agentBusy: agent.status === "running" || agent.status === "spawning",
        delegationInFlight: s.delegations[key] != null,
        // Only a verdict produced FOR the current cycle counts as its evidence
        // (cleared on dispatch); see `autopilotVerdicts`.
        verification: s.autopilotVerdicts[key] ?? null,
        now,
      });
      await apply(key, agentId, subdir, effect, now, verifying.current);
    }
  }, [keys]);

  usePoll(tick, AUTOPILOT_TICK_MS, [tick]);
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
  switch (effect.do) {
    case "dispatch": {
      s.openAutopilotCycle(key, effect.rung, effect.signature);
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
      s.settleAutopilotCycle(key, effect.rung);
      return;
    case "retry":
      s.retryAutopilotCycle(key, effect.rung, effect.barren);
      return;
    case "escalate":
      s.markAutopilotStuck(key, effect.reason, effect.rung, now);
      return;
    case "wait":
      return;
  }
}
