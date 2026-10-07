import { describe, expect, it } from "vitest";
import type { LimitsRefreshState, ProviderLimits } from "@/api/types/providers";
import { formatClockTime } from "@/util/format";
import { asOfLabel, formatCountdown, inBackoff, refreshHint, resetLabel } from "./limitsFormat";

const MIN = 60_000;
const HOUR = 60 * MIN;
/** A fixed local instant, so the clock text doesn't depend on the machine's zone. */
const NOW = new Date(2026, 9, 7, 12, 17).getTime();
const secs = (ms: number) => Math.floor(ms / 1000);

describe("formatCountdown", () => {
  it("shows two units at most", () => {
    expect(formatCountdown(2 * HOUR + 13 * MIN + 59_000)).toBe("2h 13m");
    expect(formatCountdown(45 * MIN)).toBe("45m");
    expect(formatCountdown(3 * 24 * HOUR + 4 * HOUR + 30 * MIN)).toBe("3d 4h");
  });

  it("drops a zero second unit", () => {
    expect(formatCountdown(2 * HOUR)).toBe("2h");
    expect(formatCountdown(2 * 24 * HOUR)).toBe("2d");
  });

  it("reads under a minute as less than one", () => {
    expect(formatCountdown(59_000)).toBe("<1m");
    expect(formatCountdown(0)).toBe("<1m");
  });
});

describe("resetLabel", () => {
  it("names the clock time and the time left for a reset today", () => {
    const at = NOW + 2 * HOUR + 13 * MIN;
    expect(resetLabel(secs(at), NOW)).toBe(`resets ${formatClockTime(at)} · in 2h 13m`);
  });

  it("adds the weekday for a reset more than a day out", () => {
    const at = NOW + 3 * 24 * HOUR;
    const day = new Date(at).toLocaleDateString(undefined, { weekday: "short" });
    expect(resetLabel(secs(at), NOW)).toBe(`resets ${day} ${formatClockTime(at)} · in 3d`);
  });

  it("marks a reset that already passed since the reading", () => {
    expect(resetLabel(secs(NOW - MIN), NOW)).toBe("reset since this reading");
  });

  it("has nothing to say without a reset time", () => {
    expect(resetLabel(null, NOW)).toBeNull();
  });
});

describe("asOfLabel", () => {
  const reading = (asOfMs: number): ProviderLimits => ({
    five_hour: null,
    seven_day: null,
    as_of: secs(asOfMs),
    source: "app_server",
  });

  it("gives the reading's age and its source", () => {
    expect(asOfLabel(reading(NOW - 5 * MIN), NOW)).toBe("as of 5m ago · Codex app-server");
  });

  it("calls a reading under a minute old just now", () => {
    expect(asOfLabel(reading(NOW - 10_000), NOW)).toBe("as of just now · Codex app-server");
  });
});

describe("refreshHint and inBackoff", () => {
  const state = (s: Partial<LimitsRefreshState>): LimitsRefreshState => ({
    status: "ok",
    at: secs(NOW),
    next_allowed_at: null,
    failures: 0,
    ...s,
  });

  it("says when a rate-limited refresh may run again, until it may", () => {
    const until = NOW + 10 * MIN;
    const limited = state({ status: "rate_limited", next_allowed_at: secs(until), failures: 1 });
    expect(refreshHint(limited, NOW)).toBe(`Rate limited; try again at ${formatClockTime(until)}.`);
    expect(inBackoff(limited, NOW)).toBe(true);
    expect(refreshHint(limited, until + MIN)).toBeNull();
    expect(inBackoff(limited, until + MIN)).toBe(false);
  });

  it("tells a stale login how to refresh it", () => {
    expect(refreshHint(state({ status: "stale" }), NOW)).toMatch(/Run an agent/);
  });

  it("asks a signed-out account to sign in", () => {
    expect(refreshHint(state({ status: "signed_out" }), NOW)).toMatch(/Sign in/);
  });

  it("is silent after a refresh that went fine, or none at all", () => {
    expect(refreshHint(state({}), NOW)).toBeNull();
    expect(refreshHint(null, NOW)).toBeNull();
    expect(inBackoff(null, NOW)).toBe(false);
  });
});
