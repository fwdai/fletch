// How a PR, or a checkout's whole set of them, reads at a glance. Every surface
// that shows a PR label (sidebar pill, title-bar badge, the Git panel's chips,
// Mission Control's evidence) tints and ranks through here, so a PR never reads
// "failing" in one place and "open" in another.
//
// Pure — no store, no React — so the queue selector (which must stay free of
// the store) can share it. `prState.ts` re-exports it for everyone else.

import type { PrChecks, PrSetEntry, PrState } from "@/api";
import type { BadgeVariant } from "@/components/ui/Badge";

/** One PR's pill tint and the status word its tooltip uses. */
export interface PrTint {
  variant: BadgeVariant;
  word: string;
}

/** How bad an open PR is, worst first. Failing CI outranks a conflict (it is
 *  the louder, more specific signal and the conflict often caused it), a
 *  conflict outranks a PR still waiting on CI, and passing is the best an open
 *  PR gets. */
const RANK = { failing: 0, conflicting: 1, pending: 2, passing: 3 } as const;
type OpenRank = (typeof RANK)[keyof typeof RANK];

function openRank({ state, checks }: PrSetEntry): OpenRank {
  if (checks?.rollup === "failing") return RANK.failing;
  if (state.mergeable === "conflicting") return RANK.conflicting;
  if (checks?.rollup === "passing") return RANK.passing;
  return RANK.pending;
}

/** One PR's tint: its state, refined while open by the same precedence the
 *  set summary uses. Pending and "no checks configured" stay on the neutral
 *  open blue — only a settled verdict earns a color. */
export function prTint(state: PrState, checks: PrChecks | null): PrTint {
  if (state.state === "merged") return { variant: "pr-merged", word: "merged" };
  if (state.state === "closed") return { variant: "pr-closed", word: "closed" };
  switch (openRank({ state, checks })) {
    case RANK.failing:
      return { variant: "pr-fail", word: "checks failing" };
    case RANK.conflicting:
      return { variant: "warn", word: "conflicts" };
    case RANK.passing:
      return { variant: "pr-pass", word: "checks passing" };
    default:
      return { variant: "pr-open", word: checks?.rollup === "pending" ? "checks running" : "open" };
  }
}

/** The open PR most in need of attention (first wins a tie, so callers keep
 *  their own order — focused or primary first), or null when none is open. */
export function worstOpenPr<T extends PrSetEntry>(entries: readonly T[]): T | null {
  let worst: T | null = null;
  for (const e of entries) {
    if (e.state.state !== "open") continue;
    if (!worst || openRank(e) < openRank(worst)) worst = e;
  }
  return worst;
}

export interface PrSetSummary {
  variant: BadgeVariant;
  icon: "pr" | "merge";
  /** `#N` for one PR, `N PRs` for several. */
  label: string;
  /** Every PR as `#N status word`, so the pill stays glanceable. */
  tip: string;
}

/** One pill for a set of PRs, tinted by the worst open one; with none open,
 *  closed grey over merged purple. Callers render nothing for an empty set. */
export function summarizePrSet(entries: readonly PrSetEntry[]): PrSetSummary {
  const worst = worstOpenPr(entries);
  const variant: BadgeVariant = worst
    ? prTint(worst.state, worst.checks).variant
    : entries.some((e) => e.state.state === "closed")
      ? "pr-closed"
      : "pr-merged";
  return {
    variant,
    icon: variant === "pr-merged" ? "merge" : "pr",
    label: entries.length === 1 ? `#${entries[0].state.number}` : `${entries.length} PRs`,
    tip: entries.map((e) => `#${e.state.number} ${prTint(e.state, e.checks).word}`).join(" · "),
  };
}

/** One checkout's PRs to show: its set, with the focused PR's entry taken from
 *  the legacy focused maps — the panel's one-shot reads land there first, so
 *  they are the fresher copy. Without a set (an older host, or before the first
 *  sweep) the focused PR stands alone. Newest number first, like the set. */
export function checkoutPrs(
  set: readonly PrSetEntry[] | undefined,
  focused: PrState | null | undefined,
  focusedChecks: PrChecks | null | undefined,
): PrSetEntry[] {
  const own = focused ? { state: focused, checks: focusedChecks ?? null } : null;
  if (!set) return own ? [own] : [];
  if (!own) return [...set];
  const known = set.find((e) => e.state.number === own.state.number);
  const merged = { state: own.state, checks: own.checks ?? known?.checks ?? null };
  const rest = set.filter((e) => e.state.number !== own.state.number);
  return [...rest, merged].sort((a, b) => b.state.number - a.state.number);
}
