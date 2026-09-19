// Cursor has no `system` task events; its Task `tool_call` is translated into
// Claude's so the shared background-task reducer tracks a Cursor sub-agent
// with no provider branch. These pin the translation and prove it folds.

import { describe, expect, it } from "vitest";
import { cursorAdapter } from "@/adapters/cursor/index";
import { subagentTypeName, taskEvents } from "@/adapters/cursor/task";
import { applyTaskEvent, isTaskEvent } from "@/adapters/shared/backgroundTasks";
import type { RawEvent } from "@/adapters/types";

const started: RawEvent = {
  type: "tool_call",
  subtype: "started",
  call_id: "task_1",
  tool_call: {
    taskToolCall: {
      args: { description: "Find seams", prompt: "Explore src/.", subagentType: { explore: {} } },
    },
  },
};

const completed = (result: unknown): RawEvent => ({
  ...started,
  subtype: "completed",
  tool_call: {
    taskToolCall: {
      args: { description: "Find seams", prompt: "Explore src/.", subagentType: { explore: {} } },
      result,
    },
  },
});

describe("cursor taskEvents", () => {
  it("is exposed on the adapter and ignores everything but a Task tool_call", () => {
    expect(cursorAdapter.taskEvents).toBe(taskEvents);
    expect(taskEvents({ type: "assistant", message: {} })).toEqual([]);
    expect(
      taskEvents({
        type: "tool_call",
        subtype: "started",
        call_id: "t",
        tool_call: { shellToolCall: { args: { command: "ls" } } },
      }),
    ).toEqual([]);
    // A Task call with no id can't be tracked.
    expect(taskEvents({ ...started, call_id: undefined })).toEqual([]);
    // In-flight subtypes other than `started` carry nothing the store reads.
    expect(taskEvents({ ...started, subtype: "delta" })).toEqual([]);
  });

  it("maps `started` to a foreground task_started keyed by the call id", () => {
    const [ev] = taskEvents(started);
    expect(isTaskEvent(ev)).toBe(true);
    expect(ev).toEqual({
      type: "system",
      subtype: "task_started",
      task_id: "task_1",
      tool_use_id: "task_1",
      task_type: "local_agent",
      description: "Find seams",
      subagent_type: "explore",
      prompt: "Explore src/.",
      is_backgrounded: false,
    });
  });

  it("maps `completed` to a task_notification with the outcome", () => {
    expect(
      taskEvents(
        completed({
          success: {
            conversationSteps: [{ assistantMessage: { text: "Done: two seams." } }],
            durationMs: "1500",
          },
        }),
      ),
    ).toEqual([
      {
        type: "system",
        subtype: "task_notification",
        task_id: "task_1",
        tool_use_id: "task_1",
        task_type: "local_agent",
        status: "completed",
        summary: "Done: two seams.",
        usage: { duration_ms: 1500 },
      },
    ]);
    expect(taskEvents(completed({ error: { error: "aborted" } }))[0]).toMatchObject({
      subtype: "task_notification",
      status: "failed",
      summary: "aborted",
    });
    // Nothing readable: still a completion, just without a summary.
    const bare = taskEvents(completed({ success: {} }))[0];
    expect(bare).toMatchObject({ subtype: "task_notification", status: "completed" });
    expect(bare.summary).toBeUndefined();
    expect(bare.usage).toBeUndefined();
  });

  it("folds through the shared reducer like a Claude sub-agent", () => {
    let tasks = applyTaskEvent({}, taskEvents(started)[0], 1000);
    expect(tasks.task_1).toMatchObject({
      toolUseId: "task_1",
      description: "Find seams",
      subagentType: "explore",
      taskType: "local_agent",
      backgrounded: false,
      status: "running",
      startedAt: 1000,
    });
    tasks = applyTaskEvent(
      tasks,
      taskEvents(completed({ success: { resultSuffix: "ok", durationMs: 250 } }))[0],
      2000,
    );
    expect(tasks.task_1).toMatchObject({
      status: "completed",
      endedAt: 2000,
      summary: "ok",
      durationMs: 250,
    });
    const failed = applyTaskEvent(
      tasks,
      taskEvents(completed({ error: { error: "boom" } }))[0],
      3000,
    );
    expect(failed.task_1).toMatchObject({
      status: "failed",
      failureStatus: "failed",
      summary: "boom",
    });
  });
});

describe("subagentTypeName", () => {
  it("reads the flattened string, the oneof variant, and a custom type's name", () => {
    expect(subagentTypeName("explore")).toBe("explore");
    expect(subagentTypeName({ explore: {} })).toBe("explore");
    expect(subagentTypeName({ custom: { name: "ci-investigator" } })).toBe("ci-investigator");
    expect(subagentTypeName("")).toBeUndefined();
    expect(subagentTypeName(undefined)).toBeUndefined();
    expect(subagentTypeName({})).toBeUndefined();
  });
});
