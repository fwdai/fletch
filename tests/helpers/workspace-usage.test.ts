// A rewind starts a new session in place: the conversation's own records start
// over, but the workspace's recorded spend must not — what the abandoned branch
// spent is still the workspace's. A superseded session never changes, so its
// spend is folded once.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionRecord, SupersededSession } from "@/api";

const { host, upserts } = vi.hoisted(() => ({
  // The host's superseded sessions; it sends records only for unknown ids.
  host: { superseded: [] as SupersededSession[], fail: false },
  upserts: [] as Array<Record<string, unknown>>,
}));
const readSupersededRecords = vi.hoisted(() =>
  vi.fn(async (_workspaceId: string, known: string[]) => {
    if (host.fail) throw new Error("unknown op");
    return host.superseded.map(({ session_id, records }) => ({
      session_id,
      records: known.includes(session_id) ? [] : records,
    }));
  }),
);
vi.mock("@/api", () => ({ api: { readSupersededRecords } }));
vi.mock("@/storage/db", () => ({
  dbUpsert: async (_table: string, data: Record<string, unknown>) => {
    upserts.push(data);
    return "ok";
  },
}));
vi.mock("@/adapters/usage", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/adapters/usage")>();
  return { ...actual, sessionSpend: vi.fn(actual.sessionSpend) };
});

import type { RawEvent } from "@/adapters/types";
import { sessionSpend, usageFromRecords } from "@/adapters/usage";
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
const knownAt = (i: number) => readSupersededRecords.mock.calls[i][1];

describe("recordWorkspaceUsage", () => {
  beforeEach(() => {
    upserts.length = 0;
    host.superseded = [];
    host.fail = false;
    readSupersededRecords.mockClear();
    vi.mocked(sessionSpend).mockClear();
  });

  it("keeps the recorded spend from dropping across a rewind", async () => {
    const old = [call("m1", 100, 10), call("m2", 200, 20)];
    await recordWorkspaceUsage("ws-rewind", "p1", usageFromRecords("claude", old));

    // Rewound to before m2: the new session shows m1 and ran m3 of its own.
    host.superseded = [{ session_id: "s-rewind", records: old }];
    const current = [call("m1", 100, 10, true), call("m3", 7, 3)];
    const total = await recordWorkspaceUsage(
      "ws-rewind",
      "p1",
      usageFromRecords("claude", current),
    );
    await flush();

    expect(recorded()).toEqual([330, 340]);
    expect(total.spend.tokens.input).toBe(307);
  });

  it("folds each superseded session once, and sums it from then on", async () => {
    host.superseded = [{ session_id: "s-first", records: [call("a", 100, 10)] }];
    const usage = usageFromRecords("claude", [call("n", 5, 5)]);

    const first = await recordWorkspaceUsage("ws-once", "p1", usage);
    const again = await recordWorkspaceUsage("ws-once", "p1", usage);

    expect(vi.mocked(sessionSpend)).toHaveBeenCalledTimes(1);
    expect(knownAt(1)).toContain("s-first");
    expect(again.spend).toEqual(first.spend);
    expect(again.spend.tokens.input).toBe(105);

    // A second rewind: only the new session's records are sent and folded.
    host.superseded.push({ session_id: "s-second", records: [call("b", 20, 2)] });
    const later = await recordWorkspaceUsage("ws-once", "p1", usage);

    expect(vi.mocked(sessionSpend)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionSpend).mock.calls[1][0]).toEqual([call("b", 20, 2)]);
    expect(knownAt(2)).not.toContain("s-second");
    expect(later.spend.tokens.input).toBe(125);
  });

  it("is the conversation's spend on a host that can't list superseded sessions", async () => {
    host.fail = true;
    const usage = usageFromRecords("claude", [call("m1", 100, 10)]);

    expect(await recordWorkspaceUsage("ws-old-host", "p1", usage)).toBe(usage);
    await flush();
    expect(recorded()).toEqual([110]);
  });
});
