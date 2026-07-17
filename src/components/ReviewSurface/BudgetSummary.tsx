// ReviewSurface/BudgetSummary — spent vs cap for the three run-level budgets.
// Tokens can be genuinely unknown (some providers don't report usage), which
// renders as "unknown" rather than a misleading zero.

import type { GateBudget } from "../../api";

const fmt = (n: number) => n.toLocaleString();

/** Token budget display, encoding the degraded-state rule: some providers don't
 *  report token usage, so no cap + zero measured reads as genuinely "unknown"
 *  (never a misleading 0). A cap shows spent/cap; an uncapped-but-measured run
 *  shows what was used. `unknown` is `true` when the value should render quietly. */
export function tokenBudgetLabel(budget: GateBudget): { text: string; unknown: boolean } {
  if (budget.tokens_cap == null && budget.tokens_spent === 0) {
    return { text: "unknown", unknown: true };
  }
  if (budget.tokens_cap != null) {
    return { text: `${fmt(budget.tokens_spent)} / ${fmt(budget.tokens_cap)}`, unknown: false };
  }
  return { text: `${fmt(budget.tokens_spent)} used`, unknown: false };
}

export function BudgetSummary({ budget }: { budget: GateBudget }) {
  const minsSpent = Math.round(budget.wall_ms_spent / 60_000);
  const tokens = tokenBudgetLabel(budget);

  return (
    <div className="rv-budget">
      <Item label="Turns" value={`${fmt(budget.turns_spent)} / ${fmt(budget.turns_cap)}`} />
      <Item label="Tokens" value={tokens.text} muted={tokens.unknown} />
      <Item label="Time" value={`${fmt(minsSpent)} / ${fmt(budget.wall_clock_cap_mins)} min`} />
    </div>
  );
}

function Item({ label, value, muted }: { label: string; value: string; muted?: boolean }) {
  return (
    <div className="rv-budget-item">
      <span className="rv-budget-label">{label}</span>
      <span className={`rv-budget-value mono ${muted ? "muted" : ""}`}>{value}</span>
    </div>
  );
}
