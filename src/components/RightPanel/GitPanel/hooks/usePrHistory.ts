import { useEffect, useState } from "react";
import { api, type PrState } from "@/api";

/** The PRs this checkout held *before* its current one, newest first.
 *
 *  A workspace doesn't end when its PR merges — it keeps working and opens
 *  follow-ups — so a checkout accumulates PRs over its life. The backend logs
 *  each one (`worktree_prs`, written on every PR-state fetch); this reads that
 *  log and drops the currently-bound PR, which the header already shows.
 *
 *  A plain database read, so it needs no polling: the history only grows when a
 *  new PR binds, which is exactly what `currentNumber` changing means. `null`
 *  for `currentNumber` (no PR yet) still fetches — a recycled checkout can hold
 *  history with nothing bound right now.
 */
export function usePrHistory(
  agentId: string,
  currentNumber: number | null,
  subdir?: string,
): PrState[] {
  const [prior, setPrior] = useState<PrState[]>([]);

  useEffect(() => {
    let live = true;
    api
      .getPrHistory(agentId, subdir)
      .then((all) => {
        if (live) setPrior(all.filter((pr) => pr.number !== currentNumber));
      })
      // Non-fatal: history is context, not state. An empty strip is the right
      // degradation — never a stale one from the previous checkout.
      .catch(() => {
        if (live) setPrior([]);
      });
    return () => {
      live = false;
    };
  }, [agentId, subdir, currentNumber]);

  return prior;
}
