// ── Autopilot, rolled up to one sidebar row ──────────────────────────────────
//
// Autopilot is per CHECKOUT; the sidebar row is per AGENT. A multi-repo agent
// therefore has several autopilot states behind one row, and the row has space
// for exactly one mark — so this picks which of them the row speaks for.
//
// Only two situations earn a mark at all. `working` explains motion the user
// didn't start; `stuck` is waiting on them. Enrolled-and-idle is the norm (on is
// the default), so it renders nothing — a mark there would train the eye to skip
// the two that matter.
//
// Kept pure and out of the component for the usual reason: the interesting part
// is the choice, not the markup, and the choice is what a test can pin down.

import type { AutopilotState, StuckReason } from "@/autopilot";
import type { DelegationKind } from "@/delegation";
import { rungNoun, stuckLabel } from "@/helpers/autopilotCopy";

/** The one autopilot state a row speaks for, plus the context its tooltip needs. */
export interface AutopilotSignal {
  mode: "working" | "stuck";
  /** The secondary repo this came from, or null for the agent's primary — so a
   *  multi-repo agent's tooltip can say WHERE without the row growing a label. */
  repo: string | null;
  /** What it is (or was) working on. */
  rung: DelegationKind | null;
  /** Retry number of the in-flight cycle, when there is one. */
  attempt: number | null;
  /** Why autopilot handed it back, when stuck. */
  reason: StuckReason | null;
}

/** The most attention-worthy autopilot state across ALL of an agent's checkouts,
 *  or null when none of them is working or stuck.
 *
 *  `stuck` outranks `working`: a working sibling will resolve itself, an
 *  abandoned one won't. Scans the primary key (`agentId`) plus every
 *  `agentId::subdir` secondary — the same prefix scan `maxBehind` and
 *  `stuckCheckout` use, because a secondary repo's autopilot is just as real as
 *  the primary's. Ties keep the first checkout scanned, so the row doesn't flip
 *  between equally-loud siblings as the map's key order shifts. */
export function autopilotSignal(
  autopilot: Record<string, AutopilotState>,
  agentId: string,
): AutopilotSignal | null {
  const prefix = `${agentId}::`;
  let best: AutopilotSignal | null = null;
  for (const [key, state] of Object.entries(autopilot)) {
    if (key !== agentId && !key.startsWith(prefix)) continue;
    if (!state.enrolled) continue;
    const repo = key === agentId ? null : key.slice(prefix.length);
    if (state.stuck) {
      if (best?.mode === "stuck") continue;
      best = {
        mode: "stuck",
        repo,
        rung: state.stuck.rung,
        attempt: null,
        reason: state.stuck.reason,
      };
    } else if (state.cycle?.phase === "working" && best === null) {
      // Only the agent's own turn, not the wait for CI afterwards — the mark
      // explains motion, and the PR pill already says "checks running".
      best = {
        mode: "working",
        repo,
        rung: state.cycle.rung,
        attempt: state.cycle.attempt,
        reason: null,
      };
    }
  }
  return best;
}

/** The hover line for the row's mark. Phrased as what is happening to the PR,
 *  since the mark itself is a single glyph and can't say it. */
export function autopilotTip(signal: AutopilotSignal): string {
  const where = signal.repo ? ` (${signal.repo})` : "";
  if (signal.mode === "stuck") {
    // `reason` is always set alongside a stuck mode; the fallback exists so a
    // tooltip can never come out empty.
    return `${signal.reason ? stuckLabel(signal.reason, signal.rung) : "Needs you"}${where}`;
  }
  const what = signal.rung ? `the ${rungNoun(signal.rung)}` : "the PR";
  // A second or third try is the part worth knowing — it's how close this is to
  // giving up.
  const attempt = signal.attempt && signal.attempt > 1 ? `, try ${signal.attempt}` : "";
  return `Working on ${what} automatically${where}${attempt}`;
}
