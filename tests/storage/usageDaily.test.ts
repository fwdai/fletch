import { beforeEach, describe, expect, it, vi } from "vitest";

const upserts: Array<{ table: string; data: Record<string, unknown>; conflict: string }> = [];

vi.mock("@/storage/db", () => ({
  dbUpsert: async (table: string, data: Record<string, unknown>, conflict: string) => {
    upserts.push({ table, data, conflict });
    return "ok";
  },
}));

import { EMPTY_USAGE } from "@/adapters/usage";
import { recordUsageSnapshot } from "@/storage/usageDaily";
import { localDay } from "@/util/format";

const usage = (input: number, output: number, cost = 0) => ({
  ...EMPTY_USAGE,
  inputTokens: input,
  outputTokens: output,
  costUsd: cost,
});

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("recordUsageSnapshot", () => {
  beforeEach(() => {
    upserts.length = 0;
  });

  it("upserts today's cumulative snapshot keyed by workspace and day", async () => {
    recordUsageSnapshot("ws1", "p1", usage(100, 50, 0.25));
    await flush();
    expect(upserts).toHaveLength(1);
    expect(upserts[0].table).toBe("usage_daily");
    expect(upserts[0].conflict).toBe("workspace_id,day");
    expect(upserts[0].data).toMatchObject({
      workspace_id: "ws1",
      project_id: "p1",
      day: localDay(Date.now()),
      input_tokens: 100,
      output_tokens: 50,
      cost_usd: 0.25,
    });
  });

  it("skips a re-fold with unchanged totals, writes again when they grow", async () => {
    recordUsageSnapshot("ws2", "p1", usage(100, 50));
    recordUsageSnapshot("ws2", "p1", usage(100, 50));
    recordUsageSnapshot("ws2", "p1", usage(120, 60));
    await flush();
    expect(upserts).toHaveLength(2);
    expect(upserts[1].data).toMatchObject({ input_tokens: 120, output_tokens: 60 });
  });

  it("is a no-op without a project id", async () => {
    recordUsageSnapshot("ws3", undefined, usage(10, 5));
    await flush();
    expect(upserts).toHaveLength(0);
  });
});
