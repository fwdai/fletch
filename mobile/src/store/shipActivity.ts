// The Ship tab's activity list: a short, live-only memory of what moved the
// checkout toward landing — git actions the agent ran, PR transitions, the
// playbooks and merges asked for from this phone. Newest first, capped, and
// never persisted: it starts empty on every handshake like `backgroundTasks`.

import type { PrState } from "@desktop/api/types/pr";

export interface ShipActivityEntry {
  /** Epoch millis. */
  at: number;
  text: string;
}

export type ShipActivityMap = Record<string, ShipActivityEntry[]>;

export const SHIP_ACTIVITY_CAP = 20;

/** The map with `text` prepended to `agentId`'s list, oldest entries past the
 *  cap dropped. */
export function appendActivity(
  map: ShipActivityMap,
  agentId: string,
  text: string,
  at = Date.now(),
): ShipActivityMap {
  const next = [{ at, text }, ...(map[agentId] ?? [])].slice(0, SHIP_ACTIVITY_CAP);
  return { ...map, [agentId]: next };
}

/** What a `pr:state_changed` means in one line, or null when it is not a
 *  transition worth a line (a title edit, a mergeable flip, a closed PR
 *  re-reported). */
export function prTransitionText(
  prev: PrState | null | undefined,
  next: PrState | null,
): string | null {
  if (!next) return null;
  if (!prev) return next.state === "open" ? `PR #${next.number} opened` : null;
  if (prev.state === next.state) return null;
  if (prev.state === "open" && next.state === "merged") return `PR #${next.number} merged`;
  if (prev.state === "open" && next.state === "closed") return `PR #${next.number} closed`;
  return null;
}

/** "Asked the agent to <…>" for a playbook trigger name, the short imperative
 *  form of `delegationLabel`'s progress line. An unknown trigger is named. */
export function askedText(action: string): string {
  const what: Record<string, string> = {
    commit: "commit",
    "commit-push": "commit & push",
    "commit-pr": "commit & open a PR",
    "open-pr": "open a PR",
    push: "push",
    "resolve-conflicts": "resolve the conflicts",
    "update-branch": "update the branch",
    "fix-checks": "fix the failing checks",
    "resolve-comments": "work through the review comments",
  };
  return `Asked the agent to ${what[action] ?? action}`;
}
