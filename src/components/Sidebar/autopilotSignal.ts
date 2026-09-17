// ── Autopilot, rolled up to one sidebar row ──────────────────────────────────
//
// Autopilot is per CHECKOUT; the sidebar row is per AGENT. A multi-repo agent
// therefore has several autopilot states behind one row, and the row has space
// for exactly one mark — so this picks which of them the row speaks for.
//
// Only one situation earns a mark: `working`, which explains motion the user
// didn't start. Enrolled-and-idle is the norm (on is the default), and a PR
// autopilot has given up on is just a PR — the Git panel's history says what
// happened, the row says nothing. A mark there would train the eye to skip the
// one that matters.
//
// Kept pure and out of the component for the usual reason: the interesting part
// is the choice, not the markup, and the choice is what a test can pin down.

import type { AutopilotState } from "@/autopilot";
import type { DelegationKind } from "@/delegation";
import { rungNoun } from "@/helpers/autopilotCopy";

/** The one autopilot state a row speaks for, plus the context its tooltip needs. */
export interface AutopilotSignal {
  mode: "working";
  /** The secondary repo this came from, or null for the agent's primary — so a
   *  multi-repo agent's tooltip can say WHERE without the row growing a label. */
  repo: string | null;
  /** What it is working on. */
  rung: DelegationKind;
  /** Retry number of the in-flight cycle. */
  attempt: number;
}

/** The autopilot state across ALL of an agent's checkouts worth a mark, or null
 *  when none of them is mid-turn.
 *
 *  Scans the primary key (`agentId`) plus every `agentId::subdir` secondary —
 *  the same prefix scan `maxBehind` uses, because a secondary repo's autopilot
 *  is just as real as the primary's. The first checkout scanned wins, so the row
 *  doesn't flip between equally-busy siblings as the map's key order shifts. */
export function autopilotSignal(
  autopilot: Record<string, AutopilotState>,
  agentId: string,
): AutopilotSignal | null {
  const prefix = `${agentId}::`;
  for (const [key, state] of Object.entries(autopilot)) {
    if (key !== agentId && !key.startsWith(prefix)) continue;
    if (!state.enrolled) continue;
    // Only the agent's own turn, not the wait for CI afterwards — the mark
    // explains motion, and the PR pill already says "checks running".
    if (state.cycle?.phase === "working") {
      return {
        mode: "working",
        repo: key === agentId ? null : key.slice(prefix.length),
        rung: state.cycle.rung,
        attempt: state.cycle.attempt,
      };
    }
  }
  return null;
}

/** The hover line for the row's mark. Phrased as what is happening to the PR,
 *  since the mark itself is a single glyph and can't say it. */
export function autopilotTip(signal: AutopilotSignal): string {
  const where = signal.repo ? ` (${signal.repo})` : "";
  // A second or third try is the part worth knowing — it's how close this is to
  // giving up.
  const attempt = signal.attempt > 1 ? `, try ${signal.attempt}` : "";
  return `Working on the ${rungNoun(signal.rung)} automatically${where}${attempt}`;
}
