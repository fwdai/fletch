import { formatCost, formatPercent, formatTokens } from "@/util/format";
import type { UsageCostCoverage, UsageModelRow } from "./types";

// How much of a dollar figure the catalog could actually price, turned into the
// one sentence every surface in the pane says about it.
//
// A cost summed over buckets where some models had no rate is a lower bound.
// Rendering it as "$0.00" (nothing priced) or "$4.10" (some priced) states a
// total that isn't one — the catalog can be empty on first launch, and a local
// or brand-new model may never be priceable at all. So the aggregate carries
// `unpricedTokens` beside every `costUsd`, and this decides what to show.

/** Exact: everything was priced. Partial: some tokens had no rate, so `usd` is
 *  a floor. Unpriced: nothing was priced, so there is no number to show. */
export type CostKind = "exact" | "partial" | "unpriced";

export interface CostLabel {
  kind: CostKind;
  /** Dollars that were priced. A floor when `kind` is "partial", 0 when
   *  "unpriced" — never render it bare in either case, use `text`. */
  usd: number;
  /** What to render: "$4.10", "≥ $4.10", or "Unpriced". */
  text: string;
  /** Why the figure is short, for a tooltip. null when it isn't. */
  tip: string | null;
}

/** The unpriced tokens as a share of the slice, 0–1. */
const shareOf = (unpricedTokens: number, tokens: number) =>
  tokens > 0 ? Math.min(1, unpricedTokens / tokens) : 0;

/** Classify a cost against its coverage and phrase it.
 *
 *  `tokens` is the processed tokens the cost covers and `unpricedTokens` the
 *  part of them that had no rate, both as the aggregate reports them. A slice
 *  with no tokens at all (an idle day on the chart) is "exact $0.00" — nothing
 *  ran, so nothing is missing. */
export function costLabel(costUsd: number, tokens: number, unpricedTokens: number): CostLabel {
  if (unpricedTokens <= 0)
    return { kind: "exact", usd: costUsd, text: formatCost(costUsd), tip: null };

  const missing = Math.min(unpricedTokens, tokens);
  const tip = `${formatTokens(missing)} tokens (${formatPercent(shareOf(missing, tokens))}) ran on models without a known price`;
  // Everything unpriced: there is no floor worth printing, and "$0.00" would
  // claim the calls were free.
  if (missing >= tokens) return { kind: "unpriced", usd: 0, text: "Unpriced", tip };
  return { kind: "partial", usd: costUsd, text: `≥ ${formatCost(costUsd)}`, tip };
}

/** `costLabel` for anything the aggregate gives a `costUsd`/`unpricedTokens`
 *  pair — provider rows, days, day rows, totals. */
export function coverageLabel(slice: UsageCostCoverage & { tokens: number }): CostLabel {
  return costLabel(slice.costUsd, slice.tokens, slice.unpricedTokens);
}

/** `costLabel` for a model row, which is priced all-or-nothing: one unpriced
 *  bucket nulls the row, so its tokens are either all priced or none are. */
export function modelCostLabel(row: Pick<UsageModelRow, "costUsd" | "tokens">): CostLabel {
  return row.costUsd === null
    ? costLabel(0, row.tokens, row.tokens)
    : costLabel(row.costUsd, row.tokens, 0);
}
