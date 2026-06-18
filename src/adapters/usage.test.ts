import { describe, expect, it } from "vitest";

import { claudeAdapter } from "./claude";
import { codexAdapter } from "./codex";
import { opencodeAdapter } from "./opencode";
import { piAdapter } from "./pi";
import { cursorAdapter } from "./cursor";
import { antigravityAdapter } from "./antigravity";
import { usageFromRecords, addTurnUsage, EMPTY_USAGE } from "./usage";
import type { SessionRecord } from "../api";
import type { RawEvent } from "./types";

// Bodies below are the agents' real ON-DISK transcript shapes (captured from
// live sessions), which is what session_records persists and what the usage
// extractors read — distinct from the live event stream the reducers consume.

function record(provider: string, body: RawEvent, seq = 0): SessionRecord {
  return { seq, provider, source: "transcript", native_id: `n${seq}`, agent_version: null, body };
}

describe("claude extractUsage", () => {
  const body = {
    type: "assistant",
    message: {
      model: "claude-opus-4-8",
      usage: {
        input_tokens: 2,
        output_tokens: 300,
        cache_creation_input_tokens: 10783,
        cache_read_input_tokens: 7900,
      },
    },
  } as RawEvent;

  it("maps fresh input / output / cache and context fill", () => {
    expect(claudeAdapter.extractUsage!(body)).toEqual({
      inputTokens: 2,
      outputTokens: 300,
      cacheReadTokens: 7900,
      cacheWriteTokens: 10783,
      context: { input: 2, cacheRead: 7900, cacheWrite: 10783 },
      model: "claude-opus-4-8",
    });
  });

  it("ignores non-assistant and zero-usage records", () => {
    expect(claudeAdapter.extractUsage!({ type: "user" } as RawEvent)).toBeUndefined();
    expect(
      claudeAdapter.extractUsage!({ type: "assistant", message: {} } as RawEvent),
    ).toBeUndefined();
  });
});

describe("codex extractUsage", () => {
  const body = {
    type: "event_msg",
    payload: {
      type: "token_count",
      info: {
        total_token_usage: {
          input_tokens: 65134,
          cached_input_tokens: 56064,
          output_tokens: 959,
          reasoning_output_tokens: 336,
        },
        last_token_usage: { input_tokens: 33939, cached_input_tokens: 31104 },
        model_context_window: 258400,
      },
    },
  } as RawEvent;

  it("takes cumulative totals, derives fresh input, sums reasoning", () => {
    expect(codexAdapter.extractUsage!(body)).toEqual({
      cumulative: true,
      inputTokens: 65134 - 56064,
      outputTokens: 959 + 336,
      cacheReadTokens: 56064,
      cacheWriteTokens: 0,
      context: { input: 33939 - 31104, cacheRead: 31104, cacheWrite: 0 },
      contextWindow: 258400,
    });
  });

  it("ignores non token_count event_msgs", () => {
    expect(
      codexAdapter.extractUsage!({ type: "event_msg", payload: { type: "agent_message" } } as RawEvent),
    ).toBeUndefined();
  });
});

describe("opencode extractUsage", () => {
  const body = {
    type: "step-finish",
    tokens: { input: 1532, output: 33, reasoning: 51, cache: { read: 12864, write: 0 } },
    cost: 0.0123,
  } as RawEvent;

  it("maps per-step delta with cost and context fill", () => {
    expect(opencodeAdapter.extractUsage!(body)).toEqual({
      inputTokens: 1532,
      outputTokens: 33 + 51,
      cacheReadTokens: 12864,
      cacheWriteTokens: 0,
      costUsd: 0.0123,
      context: { input: 1532, cacheRead: 12864, cacheWrite: 0 },
    });
  });
});

describe("pi extractUsage", () => {
  const body = {
    type: "message",
    message: {
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

  it("maps per-message delta with cost", () => {
    expect(piAdapter.extractUsage!(body)).toEqual({
      inputTokens: 2,
      outputTokens: 258,
      cacheReadTokens: 0,
      cacheWriteTokens: 4387,
      costUsd: 0.0338,
      context: { input: 2, cacheRead: 0, cacheWrite: 4387 },
      model: "claude-opus-4-7",
    });
  });

  it("ignores user / toolResult messages", () => {
    expect(
      piAdapter.extractUsage!({ type: "message", message: { role: "user" } } as RawEvent),
    ).toBeUndefined();
  });
});

describe("cursor extractUsage (persisted live result)", () => {
  const body = {
    type: "result",
    subtype: "success",
    request_id: "req-1",
    usage: { inputTokens: 2, outputTokens: 122, cacheReadTokens: 0, cacheWriteTokens: 27987 },
  } as RawEvent;

  it("is marked persistLiveUsage and reads the result event", () => {
    expect(cursorAdapter.persistLiveUsage).toBe(true);
    expect(cursorAdapter.extractUsage!(body)).toEqual({
      inputTokens: 2,
      outputTokens: 122,
      cacheReadTokens: 0,
      cacheWriteTokens: 27987,
      context: { input: 2, cacheRead: 0, cacheWrite: 27987 },
    });
  });

  it("ignores cursor's on-disk transcript bodies (no usage there)", () => {
    expect(
      cursorAdapter.extractUsage!({ type: "assistant", message: {} } as RawEvent),
    ).toBeUndefined();
  });

  it("folds from records once the result is persisted (live_compiled)", () => {
    const u = usageFromRecords("cursor", [record("cursor", body)]);
    expect(u.outputTokens).toBe(122);
    expect(u.cacheWriteTokens).toBe(27987);
    expect(u.contextTokens).toBe(2 + 0 + 27987);
  });
});

it("antigravity exposes no usage extractor", () => {
  expect(antigravityAdapter.extractUsage).toBeUndefined();
});

describe("usageFromRecords fold", () => {
  it("sums per-message deltas (claude) and tracks latest context fill", () => {
    const recs = [
      record("claude", {
        type: "assistant",
        message: { usage: { input_tokens: 5, output_tokens: 100, cache_creation_input_tokens: 2000, cache_read_input_tokens: 0 } },
      } as RawEvent, 0),
      record("claude", {
        type: "assistant",
        message: { usage: { input_tokens: 3, output_tokens: 50, cache_creation_input_tokens: 80, cache_read_input_tokens: 2000 } },
      } as RawEvent, 1),
    ];
    const u = usageFromRecords("claude", recs);
    expect(u.inputTokens).toBe(8);
    expect(u.outputTokens).toBe(150);
    expect(u.cacheWriteTokens).toBe(2080);
    expect(u.cacheReadTokens).toBe(2000);
    // context fill + breakdown = latest record's composition (not summed)
    expect(u.contextTokens).toBe(3 + 2000 + 80);
    expect(u.contextInput).toBe(3);
    expect(u.contextCacheRead).toBe(2000);
    expect(u.contextCacheWrite).toBe(80);
    expect(u.costUsd).toBe(0);
  });

  it("takes the latest cumulative snapshot (codex), not the sum", () => {
    const mk = (total: number, last: number, seq: number) =>
      record("codex", {
        type: "event_msg",
        payload: {
          type: "token_count",
          info: {
            total_token_usage: { input_tokens: total, cached_input_tokens: 0, output_tokens: 10, reasoning_output_tokens: 0 },
            last_token_usage: { input_tokens: last },
            model_context_window: 258400,
          },
        },
      } as RawEvent, seq);
    // duplicate first event (codex re-emits) must not double-count
    const u = usageFromRecords("codex", [mk(100, 100, 0), mk(100, 100, 1), mk(250, 150, 2)]);
    expect(u.inputTokens).toBe(250);
    expect(u.contextTokens).toBe(150);
    expect(u.contextWindow).toBe(258400);
  });

  it("accumulates native cost (pi/opencode)", () => {
    const recs = [
      record("opencode", { type: "step-finish", tokens: { input: 10, output: 5, cache: {} }, cost: 0.01 } as RawEvent, 0),
      record("opencode", { type: "step-finish", tokens: { input: 20, output: 5, cache: {} }, cost: 0.02 } as RawEvent, 1),
    ];
    expect(usageFromRecords("opencode", recs).costUsd).toBeCloseTo(0.03);
  });

  it("returns EMPTY_USAGE for providers without an extractor or with no usage", () => {
    expect(usageFromRecords("cursor", [record("cursor", { type: "assistant" } as RawEvent)])).toBe(EMPTY_USAGE);
    expect(usageFromRecords("claude", [])).toBe(EMPTY_USAGE);
  });
});

describe("addTurnUsage (fold primitive)", () => {
  it("sums deltas across turns and tracks the latest context fill", () => {
    let acc = { ...EMPTY_USAGE };
    for (const body of [
      { type: "result", usage: { inputTokens: 2, outputTokens: 100, cacheReadTokens: 0, cacheWriteTokens: 5000 } },
      { type: "result", usage: { inputTokens: 3, outputTokens: 50, cacheReadTokens: 5000, cacheWriteTokens: 80 } },
    ] as RawEvent[]) {
      acc = addTurnUsage(acc, cursorAdapter.extractUsage!(body)!);
    }
    expect(acc.inputTokens).toBe(5);
    expect(acc.outputTokens).toBe(150);
    expect(acc.cacheWriteTokens).toBe(5080);
    // latest turn wins for the window breakdown
    expect(acc.contextTokens).toBe(3 + 5000 + 80);
    expect(acc.contextCacheRead).toBe(5000);
  });
});
