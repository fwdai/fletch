// The phone's half of a sub-agent thread: the nav prop a thread rides in, the
// words its card and screen show, and the shared resolver reading a nested
// thread back out of a log.

import type { BackgroundTask } from "@desktop/adapters/shared/backgroundTasks";
import { resolveThread, threadState } from "@desktop/adapters/shared/subagents";
import { describe, expect, it } from "vitest";
import type { ChatItem } from "../src/adapters";
import { joinThreadPath, splitThreadPath, tasksByToolUse, threadStatus } from "../src/lib/thread";

const NOW = 1_700_000_000_000;

const task = (over: Partial<BackgroundTask>): BackgroundTask => ({
  taskId: "t",
  toolUseId: "toolu_1",
  description: "Audit",
  taskType: "local_agent",
  backgrounded: true,
  ownedBySubagent: false,
  status: "running",
  startedAt: NOW - 5_000,
  lastActivityAt: NOW - 1_000,
  toolUses: 0,
  totalTokens: 0,
  durationMs: 0,
  ...over,
});

describe("thread path", () => {
  it("round-trips through the flat nav prop", () => {
    expect(joinThreadPath(["toolu_a", "toolu_b"])).toBe("toolu_a/toolu_b");
    expect(splitThreadPath("toolu_a/toolu_b")).toEqual(["toolu_a", "toolu_b"]);
    expect(splitThreadPath(undefined)).toEqual([]);
    expect(splitThreadPath("")).toEqual([]);
  });
});

describe("tasksByToolUse", () => {
  it("keys tasks by the call that launched them, skipping unknown launches", () => {
    const a = task({ taskId: "a", toolUseId: "toolu_a" });
    const b = task({ taskId: "b", toolUseId: "" });
    expect(tasksByToolUse({ a, b })).toEqual({ toolu_a: a });
    expect(tasksByToolUse(undefined)).toEqual({});
  });
});

describe("threadStatus", () => {
  it("names the current tool while running, and the quiet hint past the threshold", () => {
    expect(threadStatus("running", task({ lastToolName: "Bash" }), NOW)).toBe("Bash");
    expect(threadStatus("running", task({ lastActivityAt: NOW - 4 * 60_000 }), NOW)).toBe(
      "quiet 4m",
    );
    expect(threadStatus("running", undefined, NOW)).toBe("working");
  });

  it("reads how it ended, or how long it ran and what it cost", () => {
    expect(threadStatus("failed", task({ failureStatus: "stopped" }), NOW)).toBe("stopped");
    expect(threadStatus("done", task({ durationMs: 108_000, totalTokens: 15_300 }), NOW)).toBe(
      "1m 48s · 15k tokens",
    );
    expect(threadStatus("done", undefined, NOW)).toBe("done");
  });
});

describe("resolveThread (shared)", () => {
  const inner: ChatItem = {
    kind: "tool_call",
    id: "toolu_inner",
    name: "Agent",
    input: { description: "inner" },
    children: [{ kind: "agent_message", text: "deep" }],
  };
  const outer: ChatItem = {
    kind: "tool_call",
    id: "toolu_outer",
    name: "Agent",
    input: { description: "outer" },
    children: [inner, { kind: "tool_result", tool_use_id: "toolu_inner", content: "ok" }],
  };
  const log: ChatItem[] = [
    outer,
    { kind: "tool_result", tool_use_id: "toolu_outer", content: "ok" },
  ];

  it("follows the nav prop's path down into a nested thread", () => {
    const t = resolveThread(log, splitThreadPath("toolu_outer/toolu_inner"));
    expect(t?.call).toBe(inner);
    expect(t?.items).toEqual([{ kind: "agent_message", text: "deep" }]);
    expect(t?.trail.map((c) => c.id)).toEqual(["toolu_outer", "toolu_inner"]);
  });

  it("is settled once its result is in, running only while the parent is", () => {
    const t = resolveThread(log, ["toolu_outer"]);
    const result = t ? t.parent.find((it) => it.kind === "tool_result") : undefined;
    expect(threadState(result?.kind === "tool_result" ? result : null, undefined, true)).toBe(
      "done",
    );
    expect(threadState(null, undefined, true)).toBe("running");
    expect(threadState(null, task({ status: "failed", failureStatus: "stopped" }), false)).toBe(
      "failed",
    );
  });
});
