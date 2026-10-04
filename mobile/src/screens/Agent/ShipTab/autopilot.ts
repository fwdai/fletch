// The Ship tab's autopilot line, as data: what the host's autopilot is doing for
// this agent in one short line, and whether the phone may flip its pause. The
// words follow the desktop's (`helpers/autopilotCopy`, the sidebar's
// `autopilotTip`), so the two never describe the same cycle differently.

import type { AutopilotCheckout, AutopilotCycle } from "@desktop/api/types/git";
import { rungNoun } from "@desktop/helpers/autopilotCopy";

export interface AutopilotView {
  text: string;
  /** The agent's own switch: on unless paused. */
  on: boolean;
  /** Draw the switch: the host takes `autopilot_set` from this device, and the
   *  project's switch is on — the phone flips only the agent's pause, which
   *  means nothing while the whole project is off. */
  switchable: boolean;
  /** A cycle is open, so the line is about work in flight. */
  busy: boolean;
}

/** "Autopilot: working on the failing checks, try 2". `repo` names a secondary
 *  checkout; the try count shows from the second on, since that is how close it
 *  is to giving up. Waiting for the verdict after the agent's turn is phrased as
 *  checking its work. */
export function cycleText(cycle: AutopilotCycle, repo: string | null): string {
  const where = repo ? ` (${repo})` : "";
  const attempt = cycle.attempt > 1 ? `, try ${cycle.attempt}` : "";
  const doing = cycle.phase === "working" ? "working on" : "checking its work on";
  return `Autopilot: ${doing} the ${rungNoun(cycle.rung)}${where}${attempt}`;
}

/** The line for an agent's checkouts (primary first, `agentCheckouts`), or null
 *  when the host reported none — a host without autopilot, or an agent it does
 *  not run. A multi-repo agent speaks for the first checkout with a cycle. */
export function describeAutopilot(
  rows: AutopilotCheckout[],
  canSet: boolean,
): AutopilotView | null {
  const primary = rows[0];
  if (!primary) return null;
  if (!primary.project_enabled) {
    return { text: "Autopilot is off for this project", on: false, switchable: false, busy: false };
  }
  if (primary.paused) {
    return { text: "Autopilot paused", on: false, switchable: canSet, busy: false };
  }
  const active = rows.find((r) => r.enrolled && r.cycle);
  if (active?.cycle) {
    return {
      text: cycleText(active.cycle, active.subdir),
      on: true,
      switchable: canSet,
      busy: true,
    };
  }
  return { text: "Autopilot on", on: true, switchable: canSet, busy: false };
}
