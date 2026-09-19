// The `system` task events Claude emits for sub-agents and background Bash keep
// flowing after the main turn has ended, and a finished task can start again
// when its sub-agent is resumed. These pin the reducer's lifecycle handling
// against the wire shapes captured from a live `--output-format stream-json`
// probe.

import { describe, expect, it } from "vitest";
import {
  applyTaskEvent,
  type BackgroundTaskMap,
  ENDED_TTL_MS,
  isRecentTask,
  isTaskEvent,
  liveBackgroundTasks,
  quietForMs,
  subagentActivity,
  taskForToolUse,
} from "@/adapters/shared/backgroundTasks";
import type { RawEvent } from "@/adapters/types";

const T0 = 1_000_000;

const started: RawEvent = {
  type: "system",
  subtype: "task_started",
  task_id: "a74d5be",
  tool_use_id: "toolu_01RK",
  description: "Sleep and reply task",
  subagent_type: "general-purpose",
  is_backgrounded: true,
  spawn_depth: 1,
  task_type: "local_agent",
  prompt: "sleep then reply",
  uuid: "u1",
  session_id: "s1",
};

const progress: RawEvent = {
  type: "system",
  subtype: "task_progress",
  task_id: "a74d5be",
  tool_use_id: "toolu_01RK",
  description: "Running Sleep for 25 seconds",
  subagent_type: "general-purpose",
  usage: { total_tokens: 12353, tool_uses: 1, duration_ms: 1896 },
  last_tool_name: "Bash",
};

const notified = (status: string): RawEvent => ({
  type: "system",
  subtype: "task_notification",
  task_id: "a74d5be",
  tool_use_id: "toolu_01RK",
  status,
  output_file: "/private/tmp/tasks/a74d5be.output",
  summary: "done",
  usage: { total_tokens: 14345, tool_uses: 2, duration_ms: 31053 },
});

const changed = (ids: string[]): RawEvent => ({
  type: "system",
  subtype: "background_tasks_changed",
  tasks: ids.map((task_id) => ({ task_id, task_type: "local_agent", description: "x" })),
});

describe("isTaskEvent", () => {
  it("accepts only the task subtypes of system events", () => {
    expect(isTaskEvent(started)).toBe(true);
    expect(isTaskEvent({ type: "system", subtype: "init" })).toBe(false);
    expect(isTaskEvent({ type: "result", subtype: "task_started" })).toBe(false);
  });
});

describe("applyTaskEvent", () => {
  it("returns the same reference for events it does not handle", () => {
    const prev: BackgroundTaskMap = {};
    expect(applyTaskEvent(prev, { type: "assistant" }, T0)).toBe(prev);
    expect(applyTaskEvent(prev, { type: "system", subtype: "init" }, T0)).toBe(prev);
    expect(applyTaskEvent(prev, { type: "result" }, T0)).toBe(prev);
    // A terminal update for a task never seen is nothing to act on.
    expect(
      applyTaskEvent(prev, { type: "system", subtype: "task_updated", task_id: "nope" }, T0),
    ).toBe(prev);
  });

  it("tracks started → progress → completed notification", () => {
    let tasks = applyTaskEvent({}, started, T0);
    expect(tasks.a74d5be).toMatchObject({
      taskId: "a74d5be",
      toolUseId: "toolu_01RK",
      description: "Sleep and reply task",
      taskType: "local_agent",
      subagentType: "general-purpose",
      backgrounded: true,
      ownedBySubagent: false,
      spawnDepth: 1,
      prompt: "sleep then reply",
      status: "running",
      startedAt: T0,
      lastActivityAt: T0,
      toolUses: 0,
      totalTokens: 0,
    });

    tasks = applyTaskEvent(tasks, progress, T0 + 2000);
    expect(tasks.a74d5be).toMatchObject({
      status: "running",
      // The progress `description` is the current activity, not the name.
      description: "Sleep and reply task",
      lastActivityAt: T0 + 2000,
      toolUses: 1,
      totalTokens: 12353,
      durationMs: 1896,
      lastToolName: "Bash",
    });

    tasks = applyTaskEvent(tasks, notified("completed"), T0 + 31_000);
    expect(tasks.a74d5be).toMatchObject({
      status: "completed",
      endedAt: T0 + 31_000,
      summary: "done",
      outputFile: "/private/tmp/tasks/a74d5be.output",
      toolUses: 2,
      totalTokens: 14345,
      durationMs: 31053,
    });
    expect(tasks.a74d5be.failureStatus).toBeUndefined();
  });

  it("marks a non-completed notification as failed and keeps its status", () => {
    let tasks = applyTaskEvent({}, started, T0);
    tasks = applyTaskEvent(tasks, notified("stopped"), T0 + 500);
    expect(tasks.a74d5be).toMatchObject({
      status: "failed",
      failureStatus: "stopped",
      endedAt: T0 + 500,
      summary: "done",
    });
    // Still in the map — the UI wants the failure visible after the fact.
    expect(Object.keys(tasks)).toEqual(["a74d5be"]);
  });

  it("ends a task via task_updated, honoring the patch's end_time", () => {
    let tasks = applyTaskEvent({}, started, T0);
    tasks = applyTaskEvent(
      tasks,
      {
        type: "system",
        subtype: "task_updated",
        task_id: "a74d5be",
        patch: { status: "completed", end_time: T0 + 777 },
      },
      T0 + 900,
    );
    expect(tasks.a74d5be).toMatchObject({ status: "completed", endedAt: T0 + 777 });

    // A non-terminal patch changes nothing.
    const same = applyTaskEvent(
      tasks,
      { type: "system", subtype: "task_updated", task_id: "a74d5be", patch: { status: "running" } },
      T0 + 1000,
    );
    expect(same).toBe(tasks);
  });

  it("reconciles against background_tasks_changed, ending vanished tasks", () => {
    let tasks = applyTaskEvent({}, started, T0);
    tasks = applyTaskEvent(tasks, { ...started, task_id: "other", tool_use_id: "toolu_02" }, T0);
    expect(liveBackgroundTasks(tasks).map((t) => t.taskId)).toEqual(["a74d5be", "other"]);

    // `other` vanished without a notification: completed, absent failure info.
    tasks = applyTaskEvent(tasks, changed(["a74d5be"]), T0 + 100);
    expect(tasks.other).toMatchObject({ status: "completed", endedAt: T0 + 100 });
    expect(tasks.a74d5be.status).toBe("running");

    // Nothing to reconcile: same reference.
    expect(applyTaskEvent(tasks, changed(["a74d5be"]), T0 + 200)).toBe(tasks);

    // A live task we never saw start is adopted as running.
    tasks = applyTaskEvent(tasks, changed(["a74d5be", "late"]), T0 + 300);
    expect(tasks.late).toMatchObject({
      status: "running",
      taskType: "local_agent",
      description: "x",
      startedAt: T0 + 300,
    });
  });

  it("treats a task_started for a completed task as running again", () => {
    let tasks = applyTaskEvent({}, started, T0);
    tasks = applyTaskEvent(tasks, progress, T0 + 1);
    tasks = applyTaskEvent(tasks, notified("completed"), T0 + 2);
    expect(tasks.a74d5be.status).toBe("completed");

    tasks = applyTaskEvent(tasks, started, T0 + 5000);
    expect(tasks.a74d5be).toMatchObject({
      status: "running",
      startedAt: T0 + 5000,
      lastActivityAt: T0 + 5000,
      // Claude reports cumulative usage, so the counters carry over.
      toolUses: 2,
      totalTokens: 14345,
    });
    expect(tasks.a74d5be.endedAt).toBeUndefined();
    expect(tasks.a74d5be.summary).toBeUndefined();
    expect(tasks.a74d5be.failureStatus).toBeUndefined();
  });

  describe("a task whose start was missed (e.g. the client reconnected mid-run)", () => {
    const bashNotified: RawEvent = {
      type: "system",
      subtype: "task_notification",
      task_id: "bash1",
      tool_use_id: "toolu_bash",
      status: "failed",
      output_file: "/tmp/tasks/bash1.output",
      summary: 'Background command "sleep 25" failed (exit code 1)',
    };

    it("types a task_progress as a sub-agent from its subagent_type", () => {
      const tasks = applyTaskEvent({}, progress, T0);
      expect(tasks.a74d5be).toMatchObject({
        status: "running",
        taskType: "local_agent",
        subagentType: "general-purpose",
        toolUseId: "toolu_01RK",
        description: "",
        toolUses: 1,
        lastToolName: "Bash",
      });
      expect(subagentActivity(tasks, T0)).toEqual({ running: 1, failed: 0, quietest: 0 });
    });

    it("types a sub-agent's task_notification from its usage", () => {
      const tasks = applyTaskEvent({}, notified("failed"), T0);
      expect(tasks.a74d5be).toMatchObject({
        status: "failed",
        failureStatus: "failed",
        taskType: "local_agent",
        totalTokens: 14345,
      });
      expect(subagentActivity(tasks, T0)).toEqual({ running: 0, failed: 1, quietest: undefined });
    });

    it("leaves a bash task_notification unknown rather than calling it a sub-agent", () => {
      const tasks = applyTaskEvent({}, bashNotified, T0);
      expect(tasks.bash1).toMatchObject({
        status: "failed",
        failureStatus: "failed",
        taskType: "unknown",
        ownedBySubagent: false,
        summary: 'Background command "sleep 25" failed (exit code 1)',
      });
      // Neither a running nor a failed sub-agent, whatever the summary says.
      expect(subagentActivity(tasks, T0)).toEqual({ running: 0, failed: 0, quietest: undefined });
    });

    it("upgrades an unknown type from a later background_tasks_changed", () => {
      const bashProgress: RawEvent = {
        type: "system",
        subtype: "task_progress",
        task_id: "bash1",
        tool_use_id: "toolu_bash",
        description: "still sleeping",
      };
      let tasks = applyTaskEvent({}, bashProgress, T0);
      expect(tasks.bash1).toMatchObject({ status: "running", taskType: "unknown" });

      tasks = applyTaskEvent(
        tasks,
        {
          type: "system",
          subtype: "background_tasks_changed",
          tasks: [{ task_id: "bash1", task_type: "local_bash", description: "Sleep" }],
        },
        T0 + 100,
      );
      // Typed in place: still the same running task, not a fresh start.
      expect(tasks.bash1).toMatchObject({
        status: "running",
        taskType: "local_bash",
        startedAt: T0,
      });
      expect(subagentActivity(tasks, T0 + 100)).toEqual({
        running: 0,
        failed: 0,
        quietest: undefined,
      });

      // A list that repeats what we know changes nothing.
      const same = applyTaskEvent(
        tasks,
        {
          type: "system",
          subtype: "background_tasks_changed",
          tasks: [{ task_id: "bash1", task_type: "local_bash", description: "Sleep" }],
        },
        T0 + 200,
      );
      expect(same).toBe(tasks);
    });

    it("upgrades an unknown type from a later task_started", () => {
      let tasks = applyTaskEvent({}, bashNotified, T0);
      expect(tasks.bash1.taskType).toBe("unknown");

      tasks = applyTaskEvent(
        tasks,
        {
          type: "system",
          subtype: "task_started",
          task_id: "bash1",
          tool_use_id: "toolu_bash2",
          description: "Sleep again",
          is_backgrounded: true,
          task_type: "local_bash",
          owned_by_subagent: true,
        },
        T0 + 500,
      );
      expect(tasks.bash1).toMatchObject({
        status: "running",
        taskType: "local_bash",
        ownedBySubagent: true,
        description: "Sleep again",
        startedAt: T0 + 500,
      });
    });
  });

  it("tracks a background bash task owned by a sub-agent", () => {
    let tasks = applyTaskEvent(
      {},
      {
        type: "system",
        subtype: "task_started",
        task_id: "b1xsk0ed3",
        owned_by_subagent: true,
        tool_use_id: "toolu_01WT",
        description: "Sleep for 25 seconds",
        is_backgrounded: true,
        task_type: "local_bash",
      },
      T0,
    );
    expect(tasks.b1xsk0ed3).toMatchObject({
      taskType: "local_bash",
      ownedBySubagent: true,
      backgrounded: true,
      status: "running",
    });
    expect(tasks.b1xsk0ed3.subagentType).toBeUndefined();
    expect(tasks.b1xsk0ed3.spawnDepth).toBeUndefined();
    expect(tasks.b1xsk0ed3.prompt).toBeUndefined();

    // Bash notifications carry no usage; the counters stay put.
    tasks = applyTaskEvent(
      tasks,
      {
        type: "system",
        subtype: "task_notification",
        task_id: "b1xsk0ed3",
        tool_use_id: "toolu_01WT",
        status: "completed",
        output_file: "/tmp/tasks/b1xsk0ed3.output",
        summary: "exit 0",
      },
      T0 + 25_000,
    );
    expect(tasks.b1xsk0ed3).toMatchObject({
      status: "completed",
      summary: "exit 0",
      outputFile: "/tmp/tasks/b1xsk0ed3.output",
      toolUses: 0,
      totalTokens: 0,
    });
  });
});

describe("selectors", () => {
  it("summarise running/failed counts and the quietest running task", () => {
    let tasks = applyTaskEvent({}, started, T0);
    tasks = applyTaskEvent(tasks, { ...started, task_id: "b", tool_use_id: "toolu_b" }, T0 + 10);
    tasks = applyTaskEvent(tasks, { ...started, task_id: "c", tool_use_id: "toolu_c" }, T0 + 20);
    tasks = applyTaskEvent(tasks, { ...progress, task_id: "b" }, T0 + 400);
    tasks = applyTaskEvent(tasks, { ...notified("failed"), task_id: "c" }, T0 + 30);

    expect(quietForMs(tasks.a74d5be, T0 + 500)).toBe(500);
    expect(quietForMs(tasks.b, T0 + 500)).toBe(100);
    expect(liveBackgroundTasks(tasks).map((t) => t.taskId)).toEqual(["a74d5be", "b"]);
    expect(subagentActivity(tasks, T0 + 500)).toEqual({ running: 2, failed: 1, quietest: 500 });
    expect(subagentActivity({}, T0)).toEqual({ running: 0, failed: 0, quietest: undefined });
  });

  it("count only sub-agents, not background bash", () => {
    let tasks = applyTaskEvent({}, started, T0);
    tasks = applyTaskEvent(
      tasks,
      { ...started, task_id: "bash", tool_use_id: "toolu_bash", task_type: "local_bash" },
      T0,
    );
    expect(liveBackgroundTasks(tasks).map((t) => t.taskId)).toEqual(["a74d5be", "bash"]);
    expect(subagentActivity(tasks, T0)).toEqual({ running: 1, failed: 0, quietest: 0 });
  });

  it("find the task a tool_call launched, preferring a running one", () => {
    let tasks = applyTaskEvent({}, started, T0);
    tasks = applyTaskEvent(tasks, { ...started, task_id: "b", tool_use_id: "toolu_b" }, T0 + 10);
    tasks = applyTaskEvent(tasks, notified("failed"), T0 + 20);
    // A stale duplicate under the same tool_use (never seen live, but the map
    // is permissive) must not shadow the running entry.
    tasks = applyTaskEvent(tasks, { ...started, task_id: "b2", tool_use_id: "toolu_b" }, T0 + 30);
    tasks = applyTaskEvent(tasks, { ...notified("failed"), task_id: "b" }, T0 + 40);

    expect(taskForToolUse(tasks, "toolu_01RK")?.status).toBe("failed");
    expect(taskForToolUse(tasks, "toolu_b")?.taskId).toBe("b2");
    expect(taskForToolUse(tasks, "toolu_none")).toBeUndefined();
    expect(taskForToolUse(undefined, "toolu_b")).toBeUndefined();
    expect(taskForToolUse(tasks, undefined)).toBeUndefined();
  });

  it("keep an ended task recent until the TTL passes", () => {
    let tasks = applyTaskEvent({}, started, T0);
    expect(isRecentTask(tasks.a74d5be, T0 + ENDED_TTL_MS * 5)).toBe(true);
    tasks = applyTaskEvent(tasks, notified("completed"), T0 + 100);
    expect(isRecentTask(tasks.a74d5be, T0 + 100 + ENDED_TTL_MS - 1)).toBe(true);
    expect(isRecentTask(tasks.a74d5be, T0 + 100 + ENDED_TTL_MS)).toBe(false);
  });
});
