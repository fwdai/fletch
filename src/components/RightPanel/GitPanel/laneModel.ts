// ── Lanes: the branches and pull requests of one checkout ─────────────────────
// A lane is a branch plus the PR on it, if any. A checkout usually has one;
// sub-agents add more, each on its own branch. The working tree sits on exactly
// one lane (`current`), which is how the panel answers "do these changes belong
// to a PR or to a new one". The host's focused PR is the lane the footer's
// PR actions and autopilot work on (`focused`) — usually the same lane.
//
// Pure: a projection of the store's PR set and git state, so the ordering and
// the current/focused rules are testable without React.

import type { GitState, PrChecks, PrSetEntry, PrState } from "@/api";
import { checkoutPrs } from "@/util/prSummary";

export interface Lane {
  /** `pr:<number>` for a PR lane, `branch:<name>` for a branch with no PR,
   *  `detached` before the first branch exists. */
  id: string;
  /** The lane's branch: the PR's head branch, or the checkout's branch for a
   *  branch-only lane. Null when the PR's branch is unknown (an older snapshot)
   *  or the checkout is detached. */
  branch: string | null;
  pr: PrSetEntry | null;
  /** The working tree is on this branch — uncommitted changes and unpushed
   *  commits are this lane's. */
  current: boolean;
  /** The checkout's focused PR: what Merge / Fix checks / Update branch act on
   *  and what autopilot nurses. Exactly one PR lane, when there is a PR. */
  focused: boolean;
}

export function laneId(lane: Pick<Lane, "pr" | "branch">): string {
  if (lane.pr) return `pr:${lane.pr.state.number}`;
  return lane.branch ? `branch:${lane.branch}` : "detached";
}

/** The checkout's lanes, the focused PR's first (it is the default view and
 *  the one the footer acts on), then the branch the tree is on when no PR has
 *  it, then the remaining PRs in set order (newest first).
 *
 *  A PR whose branch the snapshot didn't record is assumed to be on the
 *  checkout's branch when it is the focused one — the same assumption the
 *  branch-mismatch gate makes — and on an unknown branch otherwise. */
export function buildLanes(
  set: readonly PrSetEntry[] | undefined,
  focused: PrState | null,
  focusedChecks: PrChecks | null,
  git: GitState | null,
): Lane[] {
  const branch = git?.branch || null;
  const prLanes: Lane[] = checkoutPrs(set, focused, focusedChecks).map((entry) => {
    const isFocused = entry.state.number === focused?.number;
    const prBranch = entry.state.branch ?? null;
    const current =
      prBranch != null ? branch != null && prBranch === branch : isFocused && branch != null;
    const lane = {
      branch: prBranch ?? (current ? branch : null),
      pr: entry,
      current,
      focused: isFocused,
    };
    return { id: laneId(lane), ...lane };
  });
  if (prLanes.some((l) => l.current)) return prLanes;
  // The tree is on a branch no PR covers (or on no branch yet): its own lane.
  const bare = { branch, pr: null, current: true, focused: false };
  const bareLane: Lane = { id: laneId(bare), ...bare };
  const [first, ...rest] = prLanes;
  return first?.focused ? [first, bareLane, ...rest] : [bareLane, ...prLanes];
}

/** The lane the panel describes when the user hasn't picked one: the focused
 *  PR's, else the tree's. */
export function defaultLane(lanes: Lane[]): Lane | null {
  return lanes.find((l) => l.focused) ?? lanes.find((l) => l.current) ?? lanes[0] ?? null;
}
