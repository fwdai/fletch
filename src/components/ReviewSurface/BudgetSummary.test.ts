// Tests for tokenBudgetLabel — the review surface's budget degraded-state rule.
// The bug this guards: rendering unreported token usage as a confident "0"
// instead of "unknown" (provider token usage is sometimes absent, driver.rs).

import { describe, expect, it } from "vitest";
import type { GateBudget } from "../../api";
import { tokenBudgetLabel } from "./BudgetSummary";

function budget(over: Partial<GateBudget>): GateBudget {
  return {
    turns_spent: 0,
    turns_cap: 100,
    tokens_spent: 0,
    tokens_cap: null,
    wall_ms_spent: 0,
    wall_clock_cap_mins: 480,
    ...over,
  };
}

describe("tokenBudgetLabel", () => {
  it("reads no cap + nothing measured as unknown, not zero", () => {
    const r = tokenBudgetLabel(budget({ tokens_cap: null, tokens_spent: 0 }));
    expect(r).toEqual({ text: "unknown", unknown: true });
  });

  it("shows spent / cap when the run has a token cap", () => {
    const r = tokenBudgetLabel(budget({ tokens_cap: 500_000, tokens_spent: 1234 }));
    expect(r.unknown).toBe(false);
    expect(r.text).toBe("1,234 / 500,000");
  });

  it("shows what was used when uncapped but measured", () => {
    const r = tokenBudgetLabel(budget({ tokens_cap: null, tokens_spent: 2048 }));
    expect(r.unknown).toBe(false);
    expect(r.text).toBe("2,048 used");
  });
});
