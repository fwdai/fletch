import type { PrSetEntry } from "@/api";

/** How many PRs the switcher shows as chips before the rest fold into `+N`.
 *  Four fits the panel's narrowest width beside the overflow button. */
export const INLINE_CHIPS = 4;

/** Which of a checkout's PRs render as chips and which only in the overflow
 *  menu.
 *
 *  The focused PR is always a chip — it is the one the panel below describes.
 *  Then open PRs before settled ones (merged/closed are history; open ones are
 *  what the user is likely to switch to), newest first within each group. The
 *  chips keep the set's own order (newest number first) rather than this
 *  priority order, so selecting one never shuffles the row under the pointer. */
export function splitChips(
  entries: PrSetEntry[],
  focused: number | null,
  cap: number = INLINE_CHIPS,
): { inline: PrSetEntry[]; overflow: PrSetEntry[] } {
  if (entries.length <= cap) return { inline: entries, overflow: [] };
  const rank = (e: PrSetEntry) =>
    e.state.number === focused ? 0 : e.state.state === "open" ? 1 : 2;
  const byPriority = [...entries].sort(
    (a, b) => rank(a) - rank(b) || b.state.number - a.state.number,
  );
  const shown = new Set(byPriority.slice(0, cap).map((e) => e.state.number));
  return {
    inline: entries.filter((e) => shown.has(e.state.number)),
    overflow: entries.filter((e) => !shown.has(e.state.number)),
  };
}
