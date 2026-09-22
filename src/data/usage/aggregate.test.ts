import { describe, expect, it } from "vitest";
import type { UsageBucket, UsageScan, UsageScanTokens, UsageSessionSpan } from "@/api";
import type { SlimCatalog } from "@/data/modelCatalog";
import type { EnvironmentEntry } from "@/store/environments";
import { dayKeysBetween, localDay } from "@/util/format";
import { aggregateUsage, localHourStart, mergeScans, rangeBounds } from "./aggregate";
import { costLabel, coverageLabel, modelCostLabel } from "./costLabel";
import type { HostUsageScan } from "./types";
import { isFresh, SCAN_TTL_MS, usageScanHosts } from "./useUsageStats";

// A fixture catalog rather than a real one: what models.dev charges changes
// under us, and this file is testing the fold, not the price table. Rates are
// USD per million tokens; "sonnet-5" is priced, "mystery" is not.
const CATALOG: SlimCatalog = {
  "sonnet-5": {
    id: "sonnet-5",
    name: "Sonnet 5",
    contextWindow: 200_000,
    reasoning: true,
    cost: { input: 10, output: 10, cacheRead: 1, cacheWrite: 10 },
  },
};

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

  it("opens 24h on the hour that contains 24 hours ago, never later", () => {
    const { sinceMs, untilMs } = rangeBounds("24h", now);
    expect(untilMs).toBe(now);
    // 15:30 now → 15:30 yesterday sits in the 15:00 bucket, so the window
    // opens there: 24.5 elapsed hours, and nothing from the past 24h is missed.
    expect(sinceMs).toBe(at("2026-09-16", 15));
    expect(new Date(sinceMs).getMinutes()).toBe(0);
    expect((now - sinceMs) / 3_600_000).toBeCloseTo(24.5, 10);
  });

  it("covers at least 24 elapsed hours and at most 25, whatever the minute", () => {
    for (const minute of [0, 1, 30, 59]) {
      const from = new Date(2026, 8, 17, 10, minute).getTime();
      const { sinceMs } = rangeBounds("24h", from);
      expect(sinceMs).toBe(at("2026-09-16", 10));
      const elapsed = (from - sinceMs) / 3_600_000;
      expect(elapsed).toBeGreaterThanOrEqual(24);
      expect(elapsed).toBeLessThan(25);
      // Hour starts in [sinceMs, from]: 10:00 yesterday through 10:00 today.
      const hours = Math.floor((localHourStart(from) - sinceMs) / 3_600_000) + 1;
      expect(hours).toBe(25);
    }
  });

  it("starts multi-day ranges at local midnight so the chart shows whole days", () => {
    const { sinceMs, untilMs } = rangeBounds("7d", now);
    expect(untilMs).toBe(now);
    expect(localDay(sinceMs)).toBe("2026-09-11");
    expect(new Date(sinceMs).getHours()).toBe(0);
    expect(dayKeysBetween(sinceMs, untilMs)).toHaveLength(7);
    const thirty = rangeBounds("30d", now);
    expect(dayKeysBetween(thirty.sinceMs, thirty.untilMs)).toHaveLength(30);
    const ninety = rangeBounds("90d", now);
    expect(dayKeysBetween(ninety.sinceMs, ninety.untilMs)).toHaveLength(90);
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
        model: "sonnet-5",
        tokens: tokens(1_000, 200, 4_000, 800),
      }),
      bucket({
        hourStartMs: at(days[2], 10),
        model: "sonnet-5",
        tokens: tokens(500, 100, 0, 0),
      }),
      bucket({
        hourStartMs: at(days[2], 11),
        model: "mystery",
        provider: "codex",
        tokens: tokens(2_000, 400, 0, 0),
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

  // The two sonnet-5 buckets sum to 1,500 input, 300 output, 4,000 cache read,
  // 800 cache write. At the fixture rates (10/10/1/10 per million):
  const SONNET_COST = (1_500 * 10 + 300 * 10 + 4_000 * 1 + 800 * 10) / 1e6; // 0.030
  const SONNET_SAVINGS = (4_000 * (10 - 1)) / 1e6; // 0.036

  const stats = aggregateUsage(full, CATALOG, range);

  it("sums every token bucket and the processed total", () => {
    expect(stats.totals).toEqual({
      input: 3_500,
      output: 700,
      cacheRead: 4_000,
      cacheWrite: 800,
      processed: 9_000,
      cacheSavingsUsd: SONNET_SAVINGS,
      // "mystery" is unpriced but read nothing from cache, so the savings
      // figure is missing nothing — it is exact even though the cost isn't.
      unpricedCacheReadTokens: 0,
    });
    expect(stats.totalTokens).toBe(9_000);
    expect(stats.empty).toBe(false);
    expect(stats.scannedFiles).toBe(3);
  });

  it("costs only what the catalog prices, and says how much it couldn't", () => {
    // The sonnet-5 buckets are priced; the codex "mystery" tokens are not.
    expect(stats.totalCostUsd).toBeCloseTo(SONNET_COST, 10);
    expect(stats.unpricedTokens).toBe(2_400);
  });

  it("carries pricing coverage down to providers, days and day rows", () => {
    const [claude, codex] = stats.providers;
    expect(claude.unpricedTokens).toBe(0);
    expect(codex.unpricedTokens).toBe(2_400);

    // Day 0 is all sonnet-5; the last day mixes a priced and an unpriced model.
    expect(stats.daily[0].unpricedTokens).toBe(0);
    expect(stats.daily[2].unpricedTokens).toBe(2_400);
    const bySlice = Object.fromEntries(
      stats.daily[2].byProvider.map((p) => [p.provider, p.unpricedTokens]),
    );
    expect(bySlice).toEqual({ claude: 0, codex: 2_400 });

    expect(stats.byDay.map((d) => d.unpricedTokens)).toEqual([2_400, 0]);
  });

  it("reports full coverage when the catalog prices everything", () => {
    const priced = aggregateUsage(
      scan([bucket({ hourStartMs: at(days[0], 9), model: "sonnet-5" })]),
      CATALOG,
      range,
    );
    expect(priced.unpricedTokens).toBe(0);
    expect(priced.totals.unpricedCacheReadTokens).toBe(0);
    expect(priced.providers[0].unpricedTokens).toBe(0);
    expect(priced.byDay[0].unpricedTokens).toBe(0);
  });

  it("reports every token as unpriced against an empty catalog", () => {
    const blind = aggregateUsage(full, {}, range);
    expect(blind.totalCostUsd).toBe(0);
    expect(blind.unpricedTokens).toBe(blind.totalTokens);
    // Every cache read ran unpriced, so the savings coverage is the cache reads.
    expect(blind.totals.unpricedCacheReadTokens).toBe(4_000);
    expect(blind.totals.cacheSavingsUsd).toBe(0);
    expect(blind.providers.every((p) => p.unpricedTokens === p.tokens)).toBe(true);
    expect(blind.byDay.every((d) => d.unpricedTokens === d.tokens)).toBe(true);
    expect(blind.byModel.every((m) => m.costUsd === null)).toBe(true);
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
    expect(stats.byModel.map((m) => m.model)).toEqual(["sonnet-5", "mystery"]);
    const [sonnet, mystery] = stats.byModel;
    expect(sonnet.costUsd).toBeCloseTo(SONNET_COST, 10);
    expect(mystery.costUsd).toBeNull();
    expect(mystery.tokens).toBe(2_400);
    expect(mystery.share).toBeCloseTo(2_400 / 9_000, 10);
  });

  it("derives the calendar day from each bucket's hour", () => {
    expect(stats.daily.map((d) => d.day)).toEqual(days);
    expect(stats.daily[1]).toEqual({
      day: days[1],
      tokens: 0,
      costUsd: 0,
      unpricedTokens: 0,
      byProvider: [],
    });
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
    const blank = aggregateUsage(scan([]), CATALOG, range);
    expect(blank.empty).toBe(true);
    expect(blank.totalTokens).toBe(0);
    expect(blank.totalCostUsd).toBe(0);
    expect(blank.unpricedTokens).toBe(0);
    expect(blank.byModel).toEqual([]);
    expect(blank.byDay).toEqual([]);
    expect(blank.daily.map((d) => d.tokens)).toEqual([0, 0, 0]);
  });

  it("keeps a provider that had sessions but no recorded tokens", () => {
    const only = aggregateUsage(
      scan([], [session({ provider: "codex", id: "cx", firstMs: at(days[1], 8) })]),
      CATALOG,
      range,
    );
    expect(only.providers).toEqual([
      { provider: "codex", sessions: 1, tokens: 0, costUsd: 0, unpricedTokens: 0, share: 0 },
    ]);
  });
});

// The scan covers the widest window and every narrower range is cut out of it
// in memory, so the cut is the thing that has to be exactly right.
describe("aggregateUsage slicing", () => {
  const days = ["2026-09-14", "2026-09-15", "2026-09-16", "2026-09-17"];

  const wide = scan(days.map((d) => bucket({ hourStartMs: at(d, 9), model: "sonnet-5" })));

  it("keeps only the buckets inside the range", () => {
    const sliced = aggregateUsage(wide, CATALOG, windowOf(days.slice(2)));
    expect(sliced.byDay.map((d) => d.day)).toEqual(["2026-09-17", "2026-09-16"]);
    expect(sliced.totalTokens).toBe(220);
  });

  it("excludes the hour before the range and includes the hour it opens in", () => {
    const one = (hourStartMs: number) =>
      scan([bucket({ hourStartMs, model: "sonnet-5" })], []).buckets;
    const from = { sinceMs: at(days[3], 10), untilMs: at(days[3], 20) };

    const before = aggregateUsage({ ...wide, buckets: one(at(days[3], 9)) }, CATALOG, from);
    expect(before.empty).toBe(true);

    const opening = aggregateUsage({ ...wide, buckets: one(at(days[3], 10)) }, CATALOG, from);
    expect(opening.empty).toBe(false);
  });

  it("does not widen a mid-hour start back to the top of its hour", () => {
    // A window opening at 10:42 does not cover the 10:00 bucket: most of that
    // bucket is before the window, and admitting it is how "past 24h" used to
    // stretch to nearly 25. `rangeBounds` never produces a mid-hour start now,
    // so no range depends on the old rounding.
    const from = { sinceMs: at(days[3], 10, 42), untilMs: at(days[3], 20) };
    const sliced = aggregateUsage(
      { ...wide, buckets: [bucket({ hourStartMs: at(days[3], 10), model: "sonnet-5" })] },
      CATALOG,
      from,
    );
    expect(sliced.empty).toBe(true);
  });

  it("covers the 25 hourly buckets that hold the past 24 hours, and no more", () => {
    const now = at(days[3], 10, 59);
    const hourly = scan(
      // 25 hours back through the current one, one bucket each.
      Array.from({ length: 26 }, (_, i) =>
        bucket({ hourStartMs: at(days[3], 10) - i * 3_600_000, model: "sonnet-5" }),
      ),
    );
    const sliced = aggregateUsage(hourly, CATALOG, rangeBounds("24h", now));
    // 10:59 yesterday is inside yesterday's 10:00 bucket, so that bucket is in;
    // the one before it (09:00 yesterday) holds nothing from the past 24h.
    expect(sliced.totalTokens).toBe(25 * 110);
  });

  it("excludes a bucket sitting exactly on untilMs", () => {
    const from = { sinceMs: at(days[3], 0), untilMs: at(days[3], 12) };
    const sliced = aggregateUsage(
      { ...wide, buckets: [bucket({ hourStartMs: at(days[3], 12), model: "sonnet-5" })] },
      CATALOG,
      from,
    );
    expect(sliced.empty).toBe(true);
  });
});

describe("aggregateUsage session spans", () => {
  const day = "2026-09-17";
  const range = { sinceMs: at(day, 10), untilMs: at(day, 14) };

  const counted = (sessions: UsageSessionSpan[]) =>
    aggregateUsage(scan([], sessions), CATALOG, range).totalSessions;

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
    );
    expect(stats.totalSessions).toBe(3);
    expect(stats.providers.map((p) => [p.provider, p.sessions])).toEqual([
      ["claude", 2],
      ["codex", 1],
    ]);
  });
});

describe("costLabel", () => {
  it("states a fully priced figure exactly", () => {
    expect(costLabel(4.2, 1_000, 0)).toEqual({
      kind: "exact",
      usd: 4.2,
      text: "$4.20",
      tip: null,
    });
  });

  it("floors a partly priced figure and says how much is missing", () => {
    const label = costLabel(4.2, 1_000, 250);
    expect(label.kind).toBe("partial");
    expect(label.usd).toBe(4.2);
    expect(label.text).toBe("≥ $4.20");
    expect(label.tip).toBe("250 tokens (25%) ran on models without a known price");
  });

  it("refuses to print $0.00 when nothing was priced", () => {
    const label = costLabel(0, 2_400, 2_400);
    expect(label.kind).toBe("unpriced");
    expect(label.usd).toBe(0);
    expect(label.text).toBe("Unpriced");
    expect(label.tip).toBe("2.4k tokens (100%) ran on models without a known price");
  });

  it("treats a slice with no tokens as exactly nothing", () => {
    expect(costLabel(0, 0, 0)).toMatchObject({ kind: "exact", text: "$0.000" });
  });

  it("labels an all-or-nothing model row from its null cost", () => {
    expect(modelCostLabel({ costUsd: 0.03, tokens: 6_600 })).toMatchObject({
      kind: "exact",
      text: "$0.030",
    });
    expect(modelCostLabel({ costUsd: null, tokens: 2_400 })).toMatchObject({
      kind: "unpriced",
      text: "Unpriced",
    });
  });

  it("reads coverage straight off an aggregated slice", () => {
    const [claude, codex] = aggregateUsage(
      scan([
        bucket({ hourStartMs: at("2026-09-17", 9), model: "sonnet-5" }),
        bucket({ hourStartMs: at("2026-09-17", 9), model: "mystery", provider: "codex" }),
      ]),
      CATALOG,
      { sinceMs: at("2026-09-17", 0), untilMs: at("2026-09-17", 23) },
    ).providers;
    expect(coverageLabel(claude).kind).toBe("exact");
    expect(coverageLabel(codex).kind).toBe("unpriced");
  });
});

describe("mergeScans", () => {
  const hostScan = (envId: string, s: UsageScan, fetchedAt = 1_700_000_000_000): HostUsageScan => ({
    envId,
    envName: envId,
    scan: s,
    fetchedAt,
  });

  it("hands a single host's scan back untouched", () => {
    const only = scan([bucket({ hourStartMs: at("2026-09-17", 9), model: "sonnet-5" })]);
    expect(mergeScans([hostScan("local", only)])).toBe(only);
  });

  it("concatenates buckets in sorted order, leaving same-key cells to the fold", () => {
    const hour = at("2026-09-17", 9);
    const merged = mergeScans([
      hostScan("local", scan([bucket({ hourStartMs: hour, model: "sonnet-5" })])),
      hostScan(
        "hostB",
        scan([
          bucket({ hourStartMs: at("2026-09-17", 8), model: "mystery", provider: "codex" }),
          bucket({ hourStartMs: hour, model: "sonnet-5", tokens: tokens(7, 3) }),
        ]),
      ),
    ]);
    expect(merged.buckets.map((b) => [b.hourStartMs, b.provider, b.model])).toEqual([
      [at("2026-09-17", 8), "codex", "mystery"],
      [hour, "claude", "sonnet-5"],
      [hour, "claude", "sonnet-5"],
    ]);
    // Both cells survive into the aggregate rather than one shadowing the other.
    const stats = aggregateUsage(merged, CATALOG, windowOf(["2026-09-17", "2026-09-17"]));
    const sonnet = stats.byModel.find((m) => m.model === "sonnet-5");
    expect(sonnet?.tokens).toBe(110 + 10);
  });

  it("keeps two hosts' identical session ids apart", () => {
    const span = session({ firstMs: at("2026-09-17", 9), lastMs: at("2026-09-17", 10) });
    const merged = mergeScans([
      hostScan("local", scan([], [{ ...span, id: "abc" }])),
      hostScan("hostB", scan([], [{ ...span, id: "abc" }])),
    ]);
    expect(merged.sessions.map((s) => s.id)).toEqual(["hostB:abc", "local:abc"]);
    expect(
      aggregateUsage(merged, CATALOG, windowOf(["2026-09-17", "2026-09-17"])).totalSessions,
    ).toBe(2);
  });

  it("sums the scan counters and intersects the hosts' windows", () => {
    const a = { ...scan([]), scannedFiles: 3, filesRead: 2, bytesRead: 10 };
    const b = { ...scan([]), scannedFiles: 5, filesRead: 0, bytesRead: 7 };
    // Independent clocks: the merged window is the part both hosts covered.
    a.sinceMs = 100;
    a.untilMs = 900;
    b.sinceMs = 200;
    b.untilMs = 800;
    const merged = mergeScans([hostScan("local", a), hostScan("hostB", b)]);
    expect(merged).toMatchObject({
      scannedFiles: 8,
      filesRead: 2,
      bytesRead: 17,
      sinceMs: 200,
      untilMs: 800,
    });
  });
});

describe("usageScanHosts", () => {
  const env = (e: Partial<EnvironmentEntry> & Pick<EnvironmentEntry, "id">): EnvironmentEntry => ({
    name: e.id,
    kind: "remote",
    connection: "connected",
    ...e,
  });
  const supports = (ops: string[]) => ({ version: 2, ops, events: [], features: [] });
  const ids = (envs: EnvironmentEntry[]) =>
    usageScanHosts(Object.fromEntries(envs.map((e) => [e.id, e]))).map((e) => e.id);

  const local = env({ id: "local", name: "This Mac", kind: "local" });

  it("always scans the local machine, whatever else is around", () => {
    expect(ids([local])).toEqual(["local"]);
  });

  it("scans a connected host that answers the op, local first then by name", () => {
    const b = env({ id: "b", name: "Beta", protocol: supports(["scan_usage_transcripts"]) });
    const a = env({ id: "a", name: "Alpha", protocol: supports(["scan_usage_transcripts"]) });
    expect(ids([b, a, local])).toEqual(["local", "a", "b"]);
  });

  it("skips hosts that are not connected", () => {
    const off = env({
      id: "off",
      connection: "disconnected",
      protocol: supports(["scan_usage_transcripts"]),
    });
    expect(ids([local, off])).toEqual(["local"]);
  });

  it("skips hosts too old to answer the op, reported or defaulted", () => {
    const narrow = env({ id: "narrow", protocol: supports(["get_workspace"]) });
    // No protocol at all means the frozen v2 default set, which predates the op.
    const legacy = env({ id: "legacy" });
    expect(ids([local, narrow, legacy])).toEqual(["local"]);
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
