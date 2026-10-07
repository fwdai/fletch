import { describe, expect, it } from "vitest";
import type { AccountLimits } from "@/api/types/providers";
import { limitsKeyParts, parseAccountLimits, withLimitsChange } from "./providerLimits";

const row = (percent: number): AccountLimits => ({
  limits: {
    five_hour: { percent, resets_at: 1_788_265_323 },
    seven_day: null,
    as_of: 1_788_000_000,
    source: "app_server",
  },
  refresh: null,
});

describe("limitsKeyParts", () => {
  it("splits a limits key into provider and account", () => {
    expect(limitsKeyParts("provider_limits_claude_default")).toEqual({
      provider: "claude",
      account: "default",
    });
    expect(limitsKeyParts("provider_limits_codex_team-2")).toEqual({
      provider: "codex",
      account: "team-2",
    });
  });

  it("ignores every other key, and a limits key missing a part", () => {
    expect(limitsKeyParts("provider_account_claude")).toBeNull();
    expect(limitsKeyParts("provider_limits_claude")).toBeNull();
    expect(limitsKeyParts("provider_limits_claude_")).toBeNull();
    expect(limitsKeyParts("provider_limits__work")).toBeNull();
  });
});

describe("parseAccountLimits", () => {
  it("reads a stored row and fills absent halves with null", () => {
    expect(parseAccountLimits(JSON.stringify(row(42)))).toEqual(row(42));
    expect(parseAccountLimits("{}")).toEqual({ limits: null, refresh: null });
  });

  it("reads an unreadable row as no data", () => {
    expect(parseAccountLimits("{nope")).toBeNull();
    expect(parseAccountLimits("null")).toBeNull();
  });
});

describe("withLimitsChange", () => {
  it("folds a write into its provider's map, leaving the other accounts alone", () => {
    const current = { claude: { default: row(1) }, codex: { default: row(2) } };
    const next = withLimitsChange(current, "provider_limits_claude_work", JSON.stringify(row(3)));
    expect(next).toEqual({
      claude: { default: row(1), work: row(3) },
      codex: { default: row(2) },
    });
  });

  it("drops an account whose row was deleted", () => {
    const next = withLimitsChange(
      { claude: { work: row(1) } },
      "provider_limits_claude_work",
      null,
    );
    expect(next).toEqual({ claude: {} });
  });

  it("has nothing to do for any other setting", () => {
    expect(withLimitsChange({}, "notify_turn_complete", "true")).toBeNull();
  });
});
