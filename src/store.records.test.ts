import { describe, expect, it } from "vitest";

import { reduceRecords } from "./store";
import type { SessionRecord } from "./api";

// Canonical session_records hold verbatim per-provider transcript bodies.
// reduceRecords renders them the same way on-disk replay does:
// normalizeTranscript → reduce.
function rec(body: Record<string, unknown>): SessionRecord {
  return {
    seq: 0,
    provider: "pi",
    source: "transcript",
    native_id: "x",
    agent_version: null,
    body,
  };
}

describe("reduceRecords", () => {
  it("renders Pi on-disk records via normalizeTranscript + reduce", () => {
    const records = [
      rec({ type: "session", id: "s" }),
      rec({ type: "message", message: { role: "user", content: [{ type: "text", text: "hi" }] } }),
      rec({
        type: "message",
        message: { role: "assistant", content: [{ type: "text", text: "yo" }] },
      }),
    ];
    expect(reduceRecords("pi", records)).toEqual([
      { kind: "user_message", text: "hi" },
      { kind: "agent_message", text: "yo" },
    ]);
  });

  it("is defensive against malformed bodies", () => {
    const records = [
      rec(null as unknown as Record<string, unknown>),
      rec({ type: "weird" }),
    ];
    expect(() => reduceRecords("pi", records)).not.toThrow();
    expect(reduceRecords("pi", records)).toEqual([]);
  });
});
