// How a PR, or a checkout's whole set of them, reads at a glance. Every surface
// that shows a PR label (sidebar pill, title-bar badge, the Git panel's chips,
// Mission Control's evidence) tints and ranks through here, so a PR never reads
// "failing" in one place and "open" in another.
//
// Pure — no store, no React — so the queue selector (which must stay free of
// the store) shares it with every component.

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

/** A conflict by either reading: the PR's own `mergeable`, or the checks
 *  read's merge state (the one the title bar's merge gate goes by). */
function conflicting({ state, checks }: PrSetEntry): boolean {
  return state.mergeable === "conflicting" || checks?.merge_state === "dirty";
}

function openRank(e: PrSetEntry): OpenRank {
  if (e.checks?.rollup === "failing") return RANK.failing;
  if (conflicting(e)) return RANK.conflicting;
  if (e.checks?.rollup === "passing") return RANK.passing;
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

/** The open PR most in need of attention (first wins a tie), or null when
 *  none is open. */
function worstOpenPr(entries: readonly PrSetEntry[]): PrSetEntry | null {
  let worst: PrSetEntry | null = null;
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

/** One checkout's PRs to show, focused first, then the rest in set order
 *  (newest number first). The focused reads upsert their PR into the set, so
 *  the set is the whole story; without one (an older host, or before the first
 *  sweep) the focused PR stands alone. */
export function checkoutPrs(
  set: readonly PrSetEntry[] | undefined,
  focused: PrState | null | undefined,
  focusedChecks: PrChecks | null | undefined,
): PrSetEntry[] {
  if (!set) return focused ? [{ state: focused, checks: focusedChecks ?? null }] : [];
  const isFocused = (e: PrSetEntry) => e.state.number === focused?.number;
  return [...set.filter(isFocused), ...set.filter((e) => !isFocused(e))];
}
