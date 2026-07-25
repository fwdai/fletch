import { describe, expect, it } from "vitest";
import { antigravityAdapter } from "@/adapters/antigravity";
import { claudeAdapter } from "@/adapters/claude";
import { codexAdapter } from "@/adapters/codex";
import { cursorAdapter } from "@/adapters/cursor";
import { opencodeAdapter } from "@/adapters/opencode";
import { piAdapter } from "@/adapters/pi";
import type { RawEvent } from "@/adapters/types";
import { EMPTY_SNAPSHOT, usageFromRecords } from "@/adapters/usage";
import type { SessionRecord } from "@/api";

// Bodies below are the agents' real ON-DISK transcript shapes (captured from
// live sessions), which is what session_records persists and what the usage
// translators read — distinct from the live event stream the reducers consume.

function record(provider: string, body: RawEvent, seq = 0): SessionRecord {
  return { seq, provider, source: "transcript", native_id: `n${seq}`, agent_version: null, body };
}

function records(provider: string, bodies: RawEvent[]): SessionRecord[] {
  return bodies.map((body, i) => record(provider, body, i));
}

// ── claude ───────────────────────────────────────────────────────────────────

/** One assistant transcript line. Claude writes several of these per API call
 *  while a response streams, all sharing `message.id` + `requestId`. */
function claudeAssistant(opts: {
  msgId?: string;
  requestId?: string;
  input?: number;
  output?: number;
  cacheRead?: number;
  cacheWrite?: number;
  model?: string;
  sidechain?: boolean;
  extra?: Record<string, unknown>;
}): RawEvent {
  return {
    type: "assistant",
    ...(opts.requestId !== undefined ? { requestId: opts.requestId } : {}),
    ...(opts.sidechain ? { isSidechain: true } : {}),
    ...opts.extra,
    message: {
      ...(opts.msgId !== undefined ? { id: opts.msgId } : {}),
      ...(opts.model !== undefined ? { model: opts.model } : {}),
      usage: {
        input_tokens: opts.input ?? 0,
        output_tokens: opts.output ?? 0,
        cache_read_input_tokens: opts.cacheRead ?? 0,
        cache_creation_input_tokens: opts.cacheWrite ?? 0,
      },
    },
  } as RawEvent;
}

describe("claude usage", () => {
  it("maps fresh input / output / cache and the window the call was made against", () => {
    const u = usageFromRecords("claude", [
      record(
        "claude",
        claudeAssistant({
          msgId: "m1",
          requestId: "r1",
          input: 2,
          output: 300,
          cacheRead: 7900,
          cacheWrite: 10783,
          model: "claude-opus-4-8",
        }),
      ),
    ]);
    expect(u.spend.tokens).toEqual({ input: 2, output: 300, cacheRead: 7900, cacheWrite: 10783 });
    expect(u.context).toMatchObject({
      state: "measured",
      fill: { input: 2, cacheRead: 7900, cacheWrite: 10783 },
      tokens: 2 + 7900 + 10783,
      model: "claude-opus-4-8",
    });
    // Claude reports no dollars in-transcript, so cost is absent rather than 0.
    expect(u.spend.costUsd).toBeNull();
  });

  // The bug this whole model exists for: Claude appends a line every time it
  // re-writes a streaming response. Summing lines multiplied a turn's input and
  // cache reads by the snapshot count and inflated output on top.
  it("counts one API call once, however many times the line is re-written", () => {
    const snapshot = (output: number) =>
      claudeAssistant({
        msgId: "msg_A",
        requestId: "req_1",
        input: 4,
        output,
        cacheRead: 20_000,
        cacheWrite: 300,
      });
    const u = usageFromRecords(
      "claude",
      records("claude", [snapshot(9), snapshot(9), snapshot(159)]),
    );
    expect(u.spend.tokens).toEqual({
      input: 4,
      output: 159, // the settled count, not 9 + 9 + 159
      cacheRead: 20_000,
      cacheWrite: 300,
    });
  });

  it("still sums genuinely separate calls", () => {
    const u = usageFromRecords(
      "claude",
      records("claude", [
        claudeAssistant({ msgId: "m1", requestId: "r1", input: 5, output: 100, cacheWrite: 2000 }),
        claudeAssistant({
          msgId: "m2",
          requestId: "r2",
          input: 3,
          output: 50,
          cacheRead: 2000,
          cacheWrite: 80,
        }),
      ]),
    );
    expect(u.spend.tokens).toEqual({ input: 8, output: 150, cacheRead: 2000, cacheWrite: 2080 });
    // The window is the LAST call's, never a sum of both.
    expect(u.context.tokens).toBe(3 + 2000 + 80);
    expect(u.context.fill).toEqual({ input: 3, cacheRead: 2000, cacheWrite: 80 });
  });

  it("records without a message id can't be mistaken for duplicates", () => {
    const u = usageFromRecords(
      "claude",
      records("claude", [claudeAssistant({ input: 10 }), claudeAssistant({ input: 10 })]),
    );
    expect(u.spend.tokens.input).toBe(20);
  });

  it("counts subagent turns as spend but keeps them out of the gauge", () => {
    const u = usageFromRecords(
      "claude",
      records("claude", [
        claudeAssistant({ msgId: "m1", input: 5, cacheRead: 50_000, model: "claude-opus-4-8" }),
        claudeAssistant({
          msgId: "m2",
          input: 900,
          output: 40,
          cacheRead: 1000,
          model: "claude-haiku-4-5",
          sidechain: true,
        }),
      ]),
    );
    expect(u.spend.tokens.input).toBe(905);
    expect(u.spend.tokens.output).toBe(40);
    // The Task's small window must not read as the main conversation's.
    expect(u.context.tokens).toBe(5 + 50_000);
    expect(u.context.model).toBe("claude-opus-4-8");
  });

  it("prefers the TTL breakdown for cache writes when present", () => {
    const body = {
      type: "assistant",
      message: {
        id: "m1",
        usage: {
          input_tokens: 1,
          output_tokens: 2,
          cache_creation_input_tokens: 0,
          cache_creation: { ephemeral_5m_input_tokens: 700, ephemeral_1h_input_tokens: 300 },
        },
      },
    } as RawEvent;
    expect(usageFromRecords("claude", [record("claude", body)]).spend.tokens.cacheWrite).toBe(1000);
  });

  it("ignores API-error replays and the synthetic model", () => {
    const u = usageFromRecords(
      "claude",
      records("claude", [
        claudeAssistant({ msgId: "m1", input: 100, extra: { isApiErrorMessage: true } }),
        claudeAssistant({ msgId: "m2", input: 100, model: "<synthetic>" }),
        claudeAssistant({ msgId: "m3", input: 7 }),
      ]),
    );
    expect(u.spend.tokens.input).toBe(7);
  });

  it("ignores non-assistant and zero-usage records", () => {
    expect(usageFromRecords("claude", [record("claude", { type: "user" } as RawEvent)])).toBe(
      EMPTY_SNAPSHOT,
    );
    expect(
      usageFromRecords("claude", [
        record("claude", { type: "assistant", message: {} } as RawEvent),
      ]),
    ).toBe(EMPTY_SNAPSHOT);
  });
});

describe("claude compaction", () => {
  const boundary = (compactMetadata?: Record<string, unknown>): RawEvent =>
    ({
      type: "system",
      subtype: "compact_boundary",
      content: "Conversation compacted",
      ...(compactMetadata ? { compactMetadata } : {}),
    }) as RawEvent;

  const big = claudeAssistant({ msgId: "m1", input: 40, output: 900, cacheRead: 180_000 });

  // The reported symptom: after /compact the gauge kept showing the fill the
  // user had just spent a turn getting rid of, because no record after the
  // boundary describes the new window until the next turn runs.
  it("stops reporting the pre-compaction window once the boundary lands", () => {
    const before = usageFromRecords("claude", [record("claude", big)]);
    expect(before.context.tokens).toBe(180_040);

    const after = usageFromRecords("claude", records("claude", [big, boundary()]));
    expect(after.context.state).toBe("reset");
    expect(after.context.tokens).toBe(0);
    // Spend is untouched: those tokens were spent whatever happened next.
    expect(after.spend.tokens.cacheRead).toBe(180_000);
    expect(after.spend.tokens.output).toBe(900);
  });

  it("adopts postTokens as the new window when the CLI reports it", () => {
    const u = usageFromRecords(
      "claude",
      records("claude", [
        big,
        boundary({ trigger: "manual", preTokens: 180_040, postTokens: 21_500 }),
      ]),
    );
    expect(u.context.state).toBe("measured");
    expect(u.context.tokens).toBe(21_500);
  });

  it("hands the window back to the next real turn", () => {
    const u = usageFromRecords(
      "claude",
      records("claude", [
        big,
        boundary(),
        claudeAssistant({ msgId: "m2", input: 12, cacheWrite: 22_000 }),
      ]),
    );
    expect(u.context.state).toBe("measured");
    expect(u.context.tokens).toBe(22_012);
  });

  it("honors a micro-compaction only when it states the resulting size", () => {
    const micro = (meta?: Record<string, unknown>) =>
      ({
        type: "system",
        subtype: "microcompact_boundary",
        ...(meta ? { microcompactMetadata: meta } : {}),
      }) as RawEvent;
    // Without a size, blanking a still-mostly-full window is worse than keeping
    // the reading we have.
    expect(usageFromRecords("claude", records("claude", [big, micro()])).context.tokens).toBe(
      180_040,
    );
    expect(
      usageFromRecords("claude", records("claude", [big, micro({ postTokens: 90_000 })])).context
        .tokens,
    ).toBe(90_000);
  });
});

// ── codex ────────────────────────────────────────────────────────────────────

function codexCounter(
  total: { input: number; cached?: number; output: number; reasoning?: number },
  last?: { total?: number; input?: number; cached?: number },
): RawEvent {
  return {
    type: "event_msg",
    payload: {
      type: "token_count",
      info: {
        total_token_usage: {
          input_tokens: total.input,
          cached_input_tokens: total.cached ?? 0,
          output_tokens: total.output,
          reasoning_output_tokens: total.reasoning ?? 0,
        },
        ...(last
          ? {
              last_token_usage: {
                total_tokens: last.total ?? 0,
                input_tokens: last.input ?? 0,
                cached_input_tokens: last.cached ?? 0,
              },
            }
          : {}),
        model_context_window: 258_400,
      },
    },
  } as RawEvent;
}

describe("codex usage", () => {
  it("derives fresh input and does not add reasoning on top of output", () => {
    // codex's output_tokens already includes reasoning_output_tokens — its own
    // blended_total() adds only non-cached input and output.
    const u = usageFromRecords("codex", [
      record(
        "codex",
        codexCounter(
          { input: 65_134, cached: 56_064, output: 959, reasoning: 336 },
          {
            total: 33_939,
            input: 33_939,
            cached: 31_104,
          },
        ),
      ),
    ]);
    expect(u.spend.tokens).toEqual({
      input: 65_134 - 56_064,
      output: 959,
      cacheRead: 56_064,
      cacheWrite: 0,
    });
    expect(u.context.limit).toBe(258_400);
  });

  it("measures the window by total_tokens, which includes the turn's output", () => {
    const u = usageFromRecords("codex", [
      record(
        "codex",
        codexCounter({ input: 100, output: 10 }, { total: 40_000, input: 33_939, cached: 31_104 }),
      ),
    ]);
    // 40k, not the 33.9k input side alone.
    expect(u.context.tokens).toBe(40_000);
    expect(u.context.fill).toEqual({ input: 40_000 - 31_104, cacheRead: 31_104, cacheWrite: 0 });
  });

  it("differences the counter: a re-emitted snapshot adds nothing", () => {
    const u = usageFromRecords(
      "codex",
      records("codex", [
        codexCounter({ input: 100, output: 10 }, { total: 100 }),
        codexCounter({ input: 100, output: 10 }, { total: 100 }),
        codexCounter({ input: 250, output: 25 }, { total: 150 }),
      ]),
    );
    expect(u.spend.tokens.input).toBe(250);
    expect(u.spend.tokens.output).toBe(25);
    expect(u.context.tokens).toBe(150);
  });

  // A resumed rollout, or a fork that inherited its parent's records, restarts
  // the counter. "Latest wins" used to erase everything before the restart.
  it("rebases when the counter restarts instead of losing the history", () => {
    const u = usageFromRecords(
      "codex",
      records("codex", [
        codexCounter({ input: 200, output: 20 }, { total: 200 }),
        codexCounter({ input: 500, output: 50 }, { total: 300 }),
        codexCounter({ input: 30, output: 3 }, { total: 30 }),
      ]),
    );
    expect(u.spend.tokens.input).toBe(530);
    expect(u.spend.tokens.output).toBe(53);
  });

  // The restart the aggregate can't see: the resumed rollout sends a big fresh
  // prompt, so its total is HIGHER than the parent's even though its cached
  // prefix and output start over. Comparing totals alone reads that as
  // continuation and clamps the restarted categories to zero.
  it("rebases when only some categories restart and the total still rose", () => {
    const u = usageFromRecords(
      "codex",
      records("codex", [
        codexCounter({ input: 60_000, cached: 50_000, output: 2_000 }),
        codexCounter({ input: 100_000, cached: 5_000, output: 1_000 }),
      ]),
    );
    expect(u.spend.tokens.input).toBe(10_000 + 95_000);
    expect(u.spend.tokens.cacheRead).toBe(50_000 + 5_000);
    expect(u.spend.tokens.output).toBe(2_000 + 1_000);
  });

  it("ignores non token_count event_msgs", () => {
    expect(
      usageFromRecords("codex", [
        record("codex", { type: "event_msg", payload: { type: "agent_message" } } as RawEvent),
      ]),
    ).toBe(EMPTY_SNAPSHOT);
  });
});

// ── opencode ─────────────────────────────────────────────────────────────────

describe("opencode usage", () => {
  it("adds reasoning back into output and accumulates cost per step", () => {
    const step = (id: string, input: number, cost: number): RawEvent =>
      ({
        type: "step_finish",
        part: {
          id,
          type: "step-finish",
          modelID: "claude-sonnet-4-6",
          tokens: { input, output: 4, reasoning: 6, cache: { read: 18_560, write: 0 } },
          cost,
        },
      }) as RawEvent;
    const u = usageFromRecords(
      "opencode",
      records("opencode", [step("prt_1", 57, 0.002), step("prt_2", 12, 0.003)]),
    );
    expect(u.spend.tokens.input).toBe(69);
    expect(u.spend.tokens.output).toBe(20); // (4 + 6) twice
    expect(u.spend.costUsd).toBeCloseTo(0.005);
    expect(u.context.model).toBe("claude-sonnet-4-6");
  });

  it("reads the on-disk assistant-message shape too", () => {
    const body = {
      role: "assistant",
      id: "msg_1",
      modelID: "claude-sonnet-4-6",
      tokens: { input: 98, output: 18, reasoning: 0, cache: { read: 10_624, write: 0 } },
      cost: 0,
    } as RawEvent;
    const u = usageFromRecords("opencode", [record("opencode", body)]);
    expect(u.spend.tokens).toEqual({ input: 98, output: 18, cacheRead: 10_624, cacheWrite: 0 });
    // A genuinely free call still reports a cost of 0, not "no cost reported".
    expect(u.spend.costUsd).toBe(0);
  });

  it("ignores user/non-usage messages", () => {
    expect(usageFromRecords("opencode", [record("opencode", { role: "user" } as RawEvent)])).toBe(
      EMPTY_SNAPSHOT,
    );
  });

  it("is live-capture only, so its coverage is partial", () => {
    expect(opencodeAdapter.persistLiveUsage).toBe(true);
    expect(opencodeAdapter.usageCoverage).toBe("partial");
  });
});

// ── pi ───────────────────────────────────────────────────────────────────────

describe("pi usage", () => {
  const body = {
    type: "message",
    message: {
      id: "m1",
      role: "assistant",
      model: "claude-opus-4-7",
      usage: {
        input: 2,
        output: 258,
        cacheRead: 0,
        cacheWrite: 4387,
        totalTokens: 4647,
        cost: { total: 0.0338 },
      },
    },
  } as RawEvent;

  it("maps per-message usage with native cost", () => {
    const u = usageFromRecords("pi", [record("pi", body)]);
    expect(u.spend.tokens).toEqual({ input: 2, output: 258, cacheRead: 0, cacheWrite: 4387 });
    expect(u.spend.costUsd).toBeCloseTo(0.0338);
    expect(u.context.model).toBe("claude-opus-4-7");
  });

  it("accumulates every message into the session total", () => {
    const message = (id: string, output: number, cost: number): RawEvent =>
      ({
        type: "message",
        message: {
          id,
          role: "assistant",
          model: "claude-opus-4-7",
          usage: { input: 2, output, cacheRead: 0, cacheWrite: 100, cost: { total: cost } },
        },
      }) as RawEvent;
    const u = usageFromRecords(
      "pi",
      records("pi", [message("m1", 10, 0.01), message("m2", 20, 0.02), message("m3", 30, 0.03)]),
    );
    expect(u.spend.tokens.output).toBe(60);
    expect(u.spend.tokens.input).toBe(6);
    expect(u.spend.tokens.cacheWrite).toBe(300);
    expect(u.spend.costUsd).toBeCloseTo(0.06);
    // …while the window stays the last turn's measurement, not the sum.
    expect(u.context.tokens).toBe(102);
  });

  it("ignores user / toolResult messages", () => {
    expect(
      usageFromRecords("pi", [
        record("pi", { type: "message", message: { role: "user" } } as RawEvent),
      ]),
    ).toBe(EMPTY_SNAPSHOT);
  });
});

// ── cursor ───────────────────────────────────────────────────────────────────

describe("cursor usage (persisted live result)", () => {
  const result = (requestId: string, output: number): RawEvent =>
    ({
      type: "result",
      subtype: "success",
      request_id: requestId,
      usage: { inputTokens: 2, outputTokens: output, cacheReadTokens: 0, cacheWriteTokens: 27_987 },
    }) as RawEvent;

  it("is live-capture only, so its coverage is partial", () => {
    expect(cursorAdapter.persistLiveUsage).toBe(true);
    expect(cursorAdapter.usageCoverage).toBe("partial");
    expect(usageFromRecords("cursor", [record("cursor", result("req-1", 122))]).coverage).toBe(
      "partial",
    );
  });

  it("accumulates one entry per turn and collapses a redelivered result", () => {
    const u = usageFromRecords(
      "cursor",
      records("cursor", [result("req-1", 122), result("req-1", 122), result("req-2", 40)]),
    );
    expect(u.spend.tokens.output).toBe(162);
    expect(u.spend.tokens.cacheWrite).toBe(27_987 * 2);
    expect(u.context.tokens).toBe(2 + 27_987);
  });

  it("ignores cursor's on-disk transcript bodies (no usage there)", () => {
    expect(
      usageFromRecords("cursor", [
        record("cursor", { type: "assistant", message: {} } as RawEvent),
      ]),
    ).toBe(EMPTY_SNAPSHOT);
  });
});

it("antigravity reports no usage at all", () => {
  expect(antigravityAdapter.usageEvents).toBeUndefined();
  expect(usageFromRecords("antigravity", [record("antigravity", {} as RawEvent)])).toBe(
    EMPTY_SNAPSHOT,
  );
});

it("an empty session is EMPTY_SNAPSHOT", () => {
  expect(usageFromRecords("claude", [])).toBe(EMPTY_SNAPSHOT);
});

it("a record the translator throws on costs one record, not the session", () => {
  const broken = {
    type: "assistant",
    get message(): never {
      throw new Error("boom");
    },
  } as unknown as RawEvent;
  const u = usageFromRecords("claude", [
    record("claude", broken, 0),
    record("claude", claudeAssistant({ msgId: "m1", input: 11 }), 1),
  ]);
  expect(u.spend.tokens.input).toBe(11);
});

it("every adapter that reports usage declares how it is read", () => {
  for (const adapter of [claudeAdapter, codexAdapter, cursorAdapter, opencodeAdapter, piAdapter]) {
    expect(adapter.usageEvents).toBeTypeOf("function");
  }
});
