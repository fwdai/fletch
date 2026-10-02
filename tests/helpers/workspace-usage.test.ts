// A rewind starts a new session in place: the conversation's own records start
// over, but the workspace's recorded spend must not — what the abandoned branch
// spent is still the workspace's.

import { beforeEach, describe, expect, it, vi } from "vitest";

const { readSupersededRecords, upserts } = vi.hoisted(() => ({
  readSupersededRecords: vi.fn(),
  upserts: [] as Array<Record<string, unknown>>,
}));
vi.mock("@/api", () => ({ api: { readSupersededRecords } }));
vi.mock("@/storage/db", () => ({
  dbUpsert: async (_table: string, data: Record<string, unknown>) => {
    upserts.push(data);
    return "ok";
  },
}));

import type { RawEvent } from "@/adapters/types";
import { usageFromRecords } from "@/adapters/usage";
import type { SessionRecord } from "@/api";
import { recordWorkspaceUsage } from "@/helpers/usage";

const call = (id: string, input: number, output: number, inherited = false): SessionRecord => ({
  seq: 0,
  provider: "claude",
  source: "transcript",
  native_id: id,
  agent_version: null,
  inherited,
  body: {
    type: "assistant",
    requestId: id,
    message: { id, usage: { input_tokens: input, output_tokens: output } },
  } as RawEvent,
});

const flush = () => new Promise((r) => setTimeout(r, 0));
const recorded = () => upserts.map((row) => Number(row.input_tokens) + Number(row.output_tokens));

describe("recordWorkspaceUsage", () => {
  beforeEach(() => {
    upserts.length = 0;
    readSupersededRecords.mockReset();
  });

  it("keeps the recorded spend from dropping across a rewind", async () => {
    const old = [call("m1", 100, 10), call("m2", 200, 20)];
    readSupersededRecords.mockResolvedValue([]);
    await recordWorkspaceUsage("ws-rewind", "p1", usageFromRecords("claude", old));

    // Rewound to before m2: the new session shows m1 and ran m3 of its own.
    readSupersededRecords.mockResolvedValue([old]);
    const current = [call("m1", 100, 10, true), call("m3", 7, 3)];
    const total = await recordWorkspaceUsage(
      "ws-rewind",
      "p1",
      usageFromRecords("claude", current),
    );
    await flush();

    expect(recorded()).toEqual([330, 340]);
    expect(total.spend.tokens.input).toBe(307);
    expect(readSupersededRecords).toHaveBeenCalledWith("ws-rewind");
  });

  it("is the conversation's spend on a host that can't list superseded sessions", async () => {
    readSupersededRecords.mockRejectedValue(new Error("unknown op"));
    const usage = usageFromRecords("claude", [call("m1", 100, 10)]);

    expect(await recordWorkspaceUsage("ws-old-host", "p1", usage)).toBe(usage);
    await flush();
    expect(recorded()).toEqual([110]);
  });
});
