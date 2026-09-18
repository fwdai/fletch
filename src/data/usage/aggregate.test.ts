import { describe, expect, it } from "vitest";
import type { UsageBucket, UsageScan, UsageScanTokens, UsageSessionSpan } from "@/api";
import type { SlimCatalog } from "@/data/modelCatalog";
import { localDay } from "@/util/format";
import {
  aggregateUsage,
  daysInRange,
  localHourStart,
  processedTokens,
  rangeBounds,
  type UsagePricing,
} from "./aggregate";
import { isFresh, SCAN_TTL_MS } from "./useUsageStats";

// Pricing is stubbed rather than driven off a real catalog: what models.dev
// charges for a given id changes under us, and this file is testing the fold,
// not the price table. "sonnet" is priced, "mystery" is not.
const PRICES: Record<string, number> = { sonnet: 0.000_01, opus: 0.000_05 };

const pricing: UsagePricing = {
  priceTokens: (_catalog, modelId, t) => {
    const rate = modelId ? PRICES[modelId] : undefined;
    return rate === undefined ? null : processedTokens(t) * rate;
  },
  cacheSavingsUsd: (_catalog, modelId, t) => {
    const rate = modelId ? PRICES[modelId] : undefined;
    return rate === undefined ? null : t.cacheRead * rate;
  },
};

const CATALOG: SlimCatalog = {};

/** Local wall-clock ms for a `YYYY-MM-DD` key, so fixtures are timezone-proof. */
const at = (day: string, hour = 0, minute = 0) =>
  new Date(
    Number(day.slice(0, 4)),
    Number(day.slice(5, 7)) - 1,
    Number(day.slice(8, 10)),
    hour,
    minute,
  ).getTime();

const tokens = (input: number, output: number, cacheRead = 0, cacheWrite = 0): UsageScanTokens => ({
  input,
  output,
  cacheRead,
  cacheWrite,
});

const bucket = (
  b: Partial<UsageBucket> & Pick<UsageBucket, "hourStartMs" | "model">,
): UsageBucket => ({
  provider: "claude",
  requests: 1,
  tokens: tokens(100, 10),
  ...b,
});

const session = (s: Partial<UsageSessionSpan> & Pick<UsageSessionSpan, "firstMs">) => ({
  provider: "claude" as const,
  id: `s${s.firstMs}`,
  lastMs: s.firstMs,
  ...s,
});

/** A window whose days are stable regardless of the machine's timezone. */
const windowOf = (days: string[]) => ({
  sinceMs: at(days[0], 0),
  untilMs: at(days[days.length - 1], 23),
});

const scan = (buckets: UsageBucket[], sessions: UsageSessionSpan[] = []): UsageScan => ({
  buckets,
  sessions,
  scannedFiles: buckets.length,
  filesRead: buckets.length,
  bytesRead: 0,
  sinceMs: buckets.length ? Math.min(...buckets.map((b) => b.hourStartMs)) : 0,
  untilMs: Date.now(),
});

describe("rangeBounds", () => {
  const now = new Date(2026, 8, 17, 15, 30).getTime(); // Thu Sep 17 2026, 15:30 local

  it("treats 24h as a rolling day", () => {
    const { sinceMs, untilMs } = rangeBounds("24h", now);
    expect(untilMs).toBe(now);
    expect(sinceMs).toBe(now - 86_400_000);
  });

  it("starts multi-day ranges at local midnight so the chart shows whole days", () => {
    const { sinceMs, untilMs } = rangeBounds("7d", now);
    expect(untilMs).toBe(now);
    expect(localDay(sinceMs)).toBe("2026-09-11");
    expect(new Date(sinceMs).getHours()).toBe(0);
    expect(daysInRange({ sinceMs, untilMs })).toHaveLength(7);
    expect(daysInRange(rangeBounds("30d", now))).toHaveLength(30);
    expect(daysInRange(rangeBounds("90d", now))).toHaveLength(90);
  });
});

describe("localHourStart", () => {
  it("snaps to the top of the containing local hour", () => {
    expect(localHourStart(at("2026-09-17", 15, 42))).toBe(at("2026-09-17", 15));
    expect(localHourStart(at("2026-09-17", 15))).toBe(at("2026-09-17", 15));
  });
});

describe("aggregateUsage", () => {
  const days = ["2026-09-15", "2026-09-16", "2026-09-17"];
  const range = windowOf(days);

  const full = scan(
    [
      bucket({
        hourStartMs: at(days[0], 9),
        model: "sonnet",
        tokens: tokens(1_000, 200, 4_000, 800),
      }),
      bucket({
        hourStartMs: at(days[2], 10),
        model: "sonnet",
        tokens: tokens(500, 100, 0, 0),
        requests: 3,
      }),
      bucket({
        hourStartMs: at(days[2], 11),
        model: "mystery",
        provider: "codex",
        tokens: tokens(2_000, 400, 0, 0),
        requests: 2,
      }),
    ],
    [
      session({ firstMs: at(days[0], 9), lastMs: at(days[0], 10) }),
      session({ firstMs: at(days[2], 10) }),
      session({ firstMs: at(days[2], 11) }),
      session({ firstMs: at(days[2], 12) }),
      session({ provider: "codex", id: "cx", firstMs: at(days[2], 11) }),
    ],
  );

  const stats = aggregateUsage(full, CATALOG, range, pricing);

  it("sums every token bucket and the processed total", () => {
    expect(stats.totals).toEqual({
      input: 3_500,
      output: 700,
      cacheRead: 4_000,
      cacheWrite: 800,
      processed: 9_000,
      cacheSavingsUsd: 4_000 * PRICES.sonnet,
    });
    expect(stats.totalTokens).toBe(9_000);
    expect(stats.empty).toBe(false);
    expect(stats.scannedFiles).toBe(3);
  });

  it("costs only what the catalog prices", () => {
    // 6,600 sonnet tokens priced; the 2,400 codex tokens are unpriced.
    expect(stats.totalCostUsd).toBeCloseTo(6_600 * PRICES.sonnet, 10);
  });

  it("splits by provider with sessions and token share", () => {
    expect(stats.totalSessions).toBe(5);
    expect(stats.providers.map((p) => p.provider)).toEqual(["claude", "codex"]);
    const [claude, codex] = stats.providers;
    expect(claude).toMatchObject({ sessions: 4, tokens: 6_600 });
    expect(claude.share).toBeCloseTo(6_600 / 9_000, 10);
    expect(codex).toMatchObject({ sessions: 1, tokens: 2_400, costUsd: 0 });
    expect(codex.share).toBeCloseTo(2_400 / 9_000, 10);
  });

  it("reports an unpriced model as null cost and ranks it last", () => {
    expect(stats.byModel.map((m) => m.model)).toEqual(["sonnet", "mystery"]);
    const [sonnet, mystery] = stats.byModel;
    expect(sonnet.costUsd).toBeCloseTo(6_600 * PRICES.sonnet, 10);
    expect(sonnet.requests).toBe(4);
    expect(mystery.costUsd).toBeNull();
    expect(mystery.tokens).toBe(2_400);
    expect(mystery.share).toBeCloseTo(2_400 / 9_000, 10);
  });

  it("derives the calendar day from each bucket's hour", () => {
    expect(stats.daily.map((d) => d.day)).toEqual(days);
    expect(stats.daily[1]).toEqual({ day: days[1], tokens: 0, costUsd: 0, byProvider: [] });
    expect(stats.daily[0].tokens).toBe(6_000);
    expect(stats.daily[2].byProvider.map((p) => p.provider)).toEqual(["codex", "claude"]);
  });

  it("folds the two hours of the last day into one column", () => {
    expect(stats.daily[2].tokens).toBe(3_000);
  });

  it("lists only active days in the day breakdown, newest first", () => {
    expect(stats.byDay.map((d) => d.day)).toEqual([days[2], days[0]]);
    expect(stats.byDay[0].tokens).toBe(3_000);
    expect(stats.byDay[0].share).toBeCloseTo(3_000 / 9_000, 10);
  });

  it("is empty and share-safe with no buckets", () => {
    const blank = aggregateUsage(scan([]), CATALOG, range, pricing);
    expect(blank.empty).toBe(true);
    expect(blank.totalTokens).toBe(0);
    expect(blank.totalCostUsd).toBe(0);
    expect(blank.byModel).toEqual([]);
    expect(blank.byDay).toEqual([]);
    expect(blank.daily.map((d) => d.tokens)).toEqual([0, 0, 0]);
  });

  it("keeps a provider that had sessions but no recorded tokens", () => {
    const only = aggregateUsage(
      scan([], [session({ provider: "codex", id: "cx", firstMs: at(days[1], 8) })]),
      CATALOG,
      range,
      pricing,
    );
    expect(only.providers).toEqual([
      { provider: "codex", sessions: 1, tokens: 0, costUsd: 0, share: 0 },
    ]);
  });
});

// The scan covers the widest window and every narrower range is cut out of it
// in memory, so the cut is the thing that has to be exactly right.
describe("aggregateUsage slicing", () => {
  const days = ["2026-09-14", "2026-09-15", "2026-09-16", "2026-09-17"];

  const wide = scan(
    days.map((d, i) => bucket({ hourStartMs: at(d, 9), model: "sonnet", requests: i + 1 })),
  );

  it("keeps only the buckets inside the range", () => {
    const sliced = aggregateUsage(wide, CATALOG, windowOf(days.slice(2)), pricing);
    expect(sliced.byDay.map((d) => d.day)).toEqual(["2026-09-17", "2026-09-16"]);
    expect(sliced.totalTokens).toBe(220);
    expect(sliced.byModel[0].requests).toBe(7);
  });

  it("excludes the hour before the range and includes the hour it opens in", () => {
    const one = (hourStartMs: number) =>
      scan([bucket({ hourStartMs, model: "sonnet" })], []).buckets;
    const from = { sinceMs: at(days[3], 10), untilMs: at(days[3], 20) };

    const before = aggregateUsage(
      { ...wide, buckets: one(at(days[3], 9)) },
      CATALOG,
      from,
      pricing,
    );
    expect(before.empty).toBe(true);

    const opening = aggregateUsage(
      { ...wide, buckets: one(at(days[3], 10)) },
      CATALOG,
      from,
      pricing,
    );
    expect(opening.empty).toBe(false);
  });

  it("admits the partial hour a mid-hour start lands in", () => {
    // 24h from 10:42 must still show the 10:00 bucket — the window opens inside
    // that hour, so its tokens belong to it.
    const from = { sinceMs: at(days[3], 10, 42), untilMs: at(days[3], 20) };
    const sliced = aggregateUsage(
      { ...wide, buckets: [bucket({ hourStartMs: at(days[3], 10), model: "sonnet" })] },
      CATALOG,
      from,
      pricing,
    );
    expect(sliced.totalTokens).toBe(110);
  });

  it("excludes a bucket sitting exactly on untilMs", () => {
    const from = { sinceMs: at(days[3], 0), untilMs: at(days[3], 12) };
    const sliced = aggregateUsage(
      { ...wide, buckets: [bucket({ hourStartMs: at(days[3], 12), model: "sonnet" })] },
      CATALOG,
      from,
      pricing,
    );
    expect(sliced.empty).toBe(true);
  });
});

describe("aggregateUsage session spans", () => {
  const day = "2026-09-17";
  const range = { sinceMs: at(day, 10), untilMs: at(day, 14) };

  const counted = (sessions: UsageSessionSpan[]) =>
    aggregateUsage(scan([], sessions), CATALOG, range, pricing).totalSessions;

  it("counts a session that straddles either edge of the range", () => {
    expect(counted([session({ firstMs: at(day, 8), lastMs: at(day, 11) })])).toBe(1);
    expect(counted([session({ firstMs: at(day, 13), lastMs: at(day, 18) })])).toBe(1);
    expect(counted([session({ firstMs: at(day, 8), lastMs: at(day, 18) })])).toBe(1);
  });

  it("drops a session that ended before the range or began after it", () => {
    expect(counted([session({ firstMs: at(day, 7), lastMs: at(day, 9) })])).toBe(0);
    expect(counted([session({ firstMs: at(day, 15), lastMs: at(day, 16) })])).toBe(0);
  });

  it("treats the edges as [sinceMs, untilMs)", () => {
    // Ended exactly as the window opened: still in.
    expect(counted([session({ firstMs: at(day, 9), lastMs: at(day, 10) })])).toBe(1);
    // Began exactly as it closed: out.
    expect(counted([session({ firstMs: at(day, 14), lastMs: at(day, 15) })])).toBe(0);
  });

  it("attributes each session to its own provider", () => {
    const stats = aggregateUsage(
      scan(
        [],
        [
          session({ firstMs: at(day, 11) }),
          session({ id: "b", firstMs: at(day, 12) }),
          session({ provider: "codex", id: "c", firstMs: at(day, 12) }),
        ],
      ),
      CATALOG,
      range,
      pricing,
    );
    expect(stats.totalSessions).toBe(3);
    expect(stats.providers.map((p) => [p.provider, p.sessions])).toEqual([
      ["claude", 2],
      ["codex", 1],
    ]);
  });
});

describe("isFresh", () => {
  const now = 1_700_000_000_000;

  it("holds a scan for the TTL and then lets it go", () => {
    expect(isFresh(now, now)).toBe(true);
    expect(isFresh(now - SCAN_TTL_MS + 1, now)).toBe(true);
    expect(isFresh(now - SCAN_TTL_MS, now)).toBe(false);
    expect(isFresh(now - 60 * 60_000, now)).toBe(false);
  });
});
