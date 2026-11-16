import { describe, expect, it } from "vitest";
import type { AccountLimits } from "@/api/types/providers";
import {
  limitsKeyParts,
  liveWindows,
  parseAccountLimits,
  spentWindow,
  tightestWindow,
  withLimitsChange,
} from "./providerLimits";

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

describe("spentWindow", () => {
  const at = (s: number) => s * 1000;
  const both = (fiveHour: number, sevenDay: number): AccountLimits => ({
    limits: {
      five_hour: { percent: fiveHour, resets_at: 1_000 },
      seven_day: { percent: sevenDay, resets_at: 5_000 },
      as_of: 0,
      source: "stream",
    },
    refresh: null,
  });

  it("is null while every window has room, or nothing is known", () => {
    expect(spentWindow(both(99, 40), at(0))).toBeNull();
    expect(spentWindow(undefined, at(0))).toBeNull();
    expect(spentWindow({ limits: null, refresh: null }, at(0))).toBeNull();
  });

  it("names the spent window until it resets", () => {
    expect(spentWindow(both(100, 40), at(500))?.resets_at).toBe(1_000);
    expect(spentWindow(both(100, 40), at(1_000))).toBeNull();
  });

  it("prefers the window that frees the account last", () => {
    expect(spentWindow(both(100, 100), at(500))?.resets_at).toBe(5_000);
  });
});

describe("liveWindows", () => {
  const at = (s: number) => s * 1000;
  const reading: AccountLimits = {
    limits: {
      five_hour: { percent: 80, resets_at: 1_000 },
      seven_day: { percent: 30, resets_at: null },
      as_of: 0,
      source: "oauth_usage",
      models: [{ model: "Fable", percent: 60, resets_at: 5_000, as_of: 0 }],
    },
    refresh: null,
  };

  it("names every reported window that has not reset since the reading", () => {
    expect(liveWindows(reading, at(500)).map((w) => w.name)).toEqual(["5h", "7d", "7d Fable"]);
  });

  it("drops a window once it resets, and has nothing without a reading", () => {
    expect(liveWindows(reading, at(1_000)).map((w) => w.name)).toEqual(["7d", "7d Fable"]);
    expect(liveWindows(undefined, at(0))).toEqual([]);
    expect(liveWindows({ limits: null, refresh: null }, at(0))).toEqual([]);
  });
});

describe("tightestWindow", () => {
  const at = (s: number) => s * 1000;
  const windows = (fiveHour: number, sevenDay: number): AccountLimits => ({
    limits: {
      five_hour: { percent: fiveHour, resets_at: 1_000 },
      seven_day: { percent: sevenDay, resets_at: 5_000 },
      as_of: 0,
      source: "stream",
    },
    refresh: null,
  });

  it("picks the live window closest to its limit across providers", () => {
    const tightest = tightestWindow(
      [
        { provider: "claude", row: windows(40, 55) },
        { provider: "codex", row: windows(72, 10) },
      ],
      at(500),
    );
    expect(tightest).toMatchObject({ provider: "codex", name: "5h", window: { percent: 72 } });
  });

  it("passes over a window that has reset since its reading", () => {
    const tightest = tightestWindow(
      [
        { provider: "claude", row: windows(40, 55) },
        { provider: "codex", row: windows(72, 10) },
      ],
      at(2_000),
    );
    expect(tightest).toMatchObject({ provider: "claude", name: "7d", window: { percent: 55 } });
  });

  it("is null when no account has a live window", () => {
    expect(tightestWindow([], at(0))).toBeNull();
    expect(tightestWindow([{ provider: "claude", row: undefined }], at(0))).toBeNull();
    expect(tightestWindow([{ provider: "claude", row: windows(90, 90) }], at(6_000))).toBeNull();
  });
});
