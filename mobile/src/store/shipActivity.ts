// The Ship tab's activity list: a short, live-only memory of what moved the
// checkout toward landing — git actions the agent ran, PR transitions, the
// playbooks asked for (from any device, as the host reports them) and their
// outcomes, and the merges asked for from this phone. Newest first, capped, and
// never persisted: it starts empty on every handshake like `backgroundTasks`.

import type { DelegationEvent } from "@desktop/api/types/git";
import type { PrChecks, PrComments, PrState } from "@desktop/api/types/pr";

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

/** What a `delegation:changed` means in one line, read against the delegation
 *  it replaces (`prev`, absent for a new one): the ask when one is recorded —
 *  once, so a held trigger's later delivery is not a second ask — and the
 *  host's outcome notice when it ends. A turn starting is the strip's to show,
 *  not the log's, and an end with no notice (its agent went away) says nothing. */
export function delegationActivityText(
  prev: DelegationEvent | undefined,
  next: DelegationEvent,
): string | null {
  const asked = askedText(next.kind === "resolve" ? "resolve-conflicts" : next.kind);
  switch (next.phase) {
    case "queued":
      return `${asked} once its turn ends`;
    case "started":
      return prev?.phase === "queued" || prev?.started_at === next.started_at ? null : asked;
    case "running":
      return null;
    case "done":
    case "abandoned":
      return next.notice ?? null;
  }
}

/** What a `pr:checks_changed` means in one line, or null when the rollup did
 *  not settle — still pending, or the same verdict with a different set of
 *  failing names, which is the list's to show and not a new line. */
export function checksSettledText(
  prev: PrChecks | null | undefined,
  next: PrChecks,
): string | null {
  if (prev?.rollup === next.rollup) return null;
  if (next.rollup === "passing") return "Checks passed";
  if (next.rollup === "failing") {
    const first = next.required_failing[0];
    return first ? `Checks failing: ${first}` : "Checks failing";
  }
  return null;
}

/** What a `pr:threads_changed` means in one line: the author of the one new
 *  thread, or a count when several arrived at once. Null when none of the new
 *  ids is in the unresolved set (resolved before the event landed). */
export function newThreadsText(comments: PrComments, newIds: string[]): string | null {
  const fresh = comments.unresolved.filter((t) => newIds.includes(t.id));
  if (fresh.length === 0) return null;
  if (fresh.length === 1) return `New review comment from ${fresh[0].author}`;
  return `${fresh.length} new review comments`;
}
