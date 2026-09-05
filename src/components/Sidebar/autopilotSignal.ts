// ── Autopilot, rolled up to one sidebar row ──────────────────────────────────
//
// Autopilot is per CHECKOUT; the sidebar row is per AGENT. A multi-repo agent
// therefore has several autopilot states behind one row, and the row has space
// for exactly one mark — so this picks which of them the row speaks for.
//
// Kept pure and out of the component for the usual reason: the interesting part
// is the choice, not the markup, and the choice is what a test can pin down.

import type { AutopilotState, StuckReason } from "@/autopilot";
// `stuckLabel` is imported rather than re-worded so the Git panel chip and this
// row can never disagree about what happened to a checkout.
import {
  type ChipMode,
  chipMode,
  stuckLabel,
} from "@/components/RightPanel/GitPanel/AutopilotChip";

/** How much of the user's attention each mode has earned.
 *
 *  Ordered by "what would you want to know first if only one thing could be
 *  shown": `stuck` outranks everything because it's the only mode waiting on a
 *  person — a working sibling will resolve itself, an abandoned one won't.
 *  `working` next: it explains motion the user didn't start. `paused` and `idle`
 *  are states the user chose or expects (on is the default, so `idle` is the
 *  norm), and `off` means the project switched it off — so they rank low and
 *  (see the sidebar row) render nothing at all. */
const ATTENTION: Record<ChipMode, number> = {
  off: 0,
  idle: 1,
  paused: 2,
  working: 3,
  stuck: 4,
};

/** The one autopilot state a row speaks for, plus the context its tooltip needs. */
export interface AutopilotSignal {
  mode: Exclude<ChipMode, "off">;
  /** The secondary repo this came from, or null for the agent's primary — so a
   *  multi-repo agent's tooltip can say WHERE without the row growing a label. */
  repo: string | null;
  /** Retry number of the in-flight cycle, when there is one. */
  attempt: number | null;
  /** Why autopilot handed it back, when stuck. */
  reason: StuckReason | null;
}

/** The most attention-worthy autopilot state across ALL of an agent's checkouts,
 *  or null when none of them is enrolled.
 *
 *  Scans the primary key (`agentId`) plus every `agentId::subdir` secondary —
 *  the same prefix scan `maxBehind` and `stuckCheckout` use, because a secondary
 *  repo's autopilot is just as real as the primary's and would otherwise be
 *  invisible outside the Git panel. Ties keep the first checkout scanned, so the
 *  row doesn't flip between equally-loud siblings as the map's key order shifts. */
export function autopilotSignal(
  autopilot: Record<string, AutopilotState>,
  agentId: string,
): AutopilotSignal | null {
  const prefix = `${agentId}::`;
  let best: AutopilotSignal | null = null;
  // Starting at `off`'s rank is what makes "nothing enrolled" return null: an
  // off checkout can never outrank it, so it never becomes the answer. The
  // explicit `off` test below says the same thing to the type checker.
  let bestRank = ATTENTION.off;
  for (const [key, state] of Object.entries(autopilot)) {
    if (key !== agentId && !key.startsWith(prefix)) continue;
    const mode = chipMode(state);
    if (mode === "off" || ATTENTION[mode] <= bestRank) continue;
    bestRank = ATTENTION[mode];
    best = {
      mode,
      repo: key === agentId ? null : key.slice(prefix.length),
      attempt: state.cycle?.attempt ?? null,
      reason: state.stuck?.reason ?? null,
    };
  }
  return best;
}

/** The hover line for the row's mark. Phrased as what autopilot is doing, since
 *  the mark itself is a single glyph and can't say it. */
export function autopilotTip(signal: AutopilotSignal): string {
  const where = signal.repo ? ` (${signal.repo})` : "";
  switch (signal.mode) {
    case "stuck":
      // `reason` is always set alongside a stuck mode; the fallback exists so a
      // tooltip can never come out empty.
      return `${signal.reason ? stuckLabel(signal.reason) : "Autopilot stopped"}${where}`;
    case "working": {
      // A second or third try is the part worth knowing — it's how close this is
      // to giving up.
      const attempt = signal.attempt && signal.attempt > 1 ? `, attempt ${signal.attempt}` : "";
      return `Autopilot working${where}${attempt}`;
    }
    case "paused":
      return `Autopilot paused${where}`;
    default:
      return `Autopilot on${where}`;
  }
}
