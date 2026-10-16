// ── Lanes: the branches and pull requests of a workspace ──────────────────────
// A lane is a branch plus the PR on it, if any. A checkout usually has one;
// sub-agents add more, each on its own branch, and a multi-repo workspace has
// a checkout per repo. The working tree of a checkout sits on exactly one of
// its lanes (`current`), which is how the panel answers "do these changes
// belong to a PR or to a new one". The checkout's focused PR is the lane the
// footer's PR actions and autopilot work on (`focused`) — usually the same.
//
// Pure: a projection of the store's PR set and git state, so the ordering and
// the current/focused rules are testable without React.

import type { GitState, PrChecks, PrSetEntry, PrState } from "@/api";
import { checkoutPrs } from "@/util/prSummary";

/** Which checkout a lane belongs to, for a workspace with several. */
export interface LaneScope {
  /** Prefixes the lane ids, so two checkouts' `pr:12` stay apart. */
  id: string;
  /** The checkout's `subdir` as the store keys it: undefined for the primary. */
  subdir?: string;
  /** The repo's name, said on the lane. */
  label: string;
}

export interface Lane {
  /** `pr:<number>` for a PR lane, `branch:<name>` for a branch with no PR,
   *  `detached` before the first branch exists — prefixed by the scope's id
   *  when the workspace has several checkouts. */
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
   *  and what autopilot nurses. Exactly one PR lane per checkout with a PR. */
  focused: boolean;
  /** The checkout's git state — the base, the tree's diff. */
  git: GitState | null;
  /** Set when the workspace has several checkouts. */
  scope?: LaneScope;
}

function laneId(lane: Pick<Lane, "pr" | "branch">, scope?: LaneScope): string {
  const own = lane.pr
    ? `pr:${lane.pr.state.number}`
    : lane.branch
      ? `branch:${lane.branch}`
      : "detached";
  return scope ? `${scope.id}/${own}` : own;
}

/** One checkout's lanes, the focused PR's first (it is the default view and
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
  scope?: LaneScope,
  /** The branch the repo record remembers, for the moment before the first
   *  git read lands — so a checkout doesn't flash "no branch yet". */
  knownBranch?: string | null,
): Lane[] {
  const branch = git?.branch || (git ? null : knownBranch) || null;
  const common = scope ? { git, scope } : { git };
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
    return { id: laneId(lane, scope), ...lane, ...common };
  });
  if (prLanes.some((l) => l.current)) return prLanes;
  // The tree is on a branch no PR covers (or on no branch yet): its own lane.
  const bare = { branch, pr: null, current: true, focused: false };
  const bareLane: Lane = { id: laneId(bare, scope), ...bare, ...common };
  const [first, ...rest] = prLanes;
  return first?.focused ? [first, bareLane, ...rest] : [bareLane, ...prLanes];
}

/** The lane the panel describes when the user hasn't picked one. The order
 *  already puts it first: the primary checkout's focused PR, else its tree. */
export function defaultLane(lanes: Lane[]): Lane | null {
  return lanes[0] ?? null;
}
