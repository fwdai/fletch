import { describe, expect, it } from "vitest";
import type { BackgroundTask, BackgroundTaskMap } from "@/adapters/shared/backgroundTasks";
import { deriveSubagentChildren, failedTip, runningTip } from "./subagentChildren";

const T0 = 1_000_000;
const MIN = 60_000;

function task(over: Partial<BackgroundTask> & { taskId: string }): BackgroundTask {
  return {
    toolUseId: `toolu_${over.taskId}`,
    description: over.taskId,
    taskType: "local_agent",
    backgrounded: true,
    ownedBySubagent: false,
    status: "running",
    startedAt: T0,
    lastActivityAt: T0,
    toolUses: 0,
    totalTokens: 0,
    durationMs: 0,
    ...over,
  };
}

function map(...tasks: BackgroundTask[]): BackgroundTaskMap {
  return Object.fromEntries(tasks.map((t) => [t.taskId, t]));
}

describe("deriveSubagentChildren", () => {
  it("orders by launch time, then id for stable ties", () => {
    const out = deriveSubagentChildren(
      map(
        task({ taskId: "b", startedAt: T0 + 2 }),
        task({ taskId: "a", startedAt: T0 + 1 }),
        task({ taskId: "c", startedAt: T0 + 2 }),
      ),
      T0 + 10,
    );
    expect(out.map((c) => c.task.taskId)).toEqual(["a", "b", "c"]);
  });

  it("keeps only the main agent's own sub-agents", () => {
    const out = deriveSubagentChildren(
      map(
        task({ taskId: "mine" }),
        task({ taskId: "nested", ownedBySubagent: true }),
        task({ taskId: "bash", taskType: "local_bash" }),
      ),
      T0,
    );
    expect(out.map((c) => c.task.taskId)).toEqual(["mine"]);
  });

  it("drops completed tasks at once and failed ones after the TTL", () => {
    const now = T0 + 30 * MIN;
    const out = deriveSubagentChildren(
      map(
        task({ taskId: "live" }),
        task({ taskId: "done", status: "completed", startedAt: T0 + 1, endedAt: now - 1 }),
        task({
          taskId: "fresh-fail",
          status: "failed",
          failureStatus: "stopped",
          startedAt: T0 + 2,
          endedAt: now - 9 * MIN,
        }),
        task({
          taskId: "old-fail",
          status: "failed",
          failureStatus: "stopped",
          startedAt: T0 + 3,
          endedAt: now - 11 * MIN,
        }),
      ),
      now,
    );
    expect(out.map((c) => c.task.taskId)).toEqual(["live", "fresh-fail"]);
    expect(out[0]).toMatchObject({ running: true, failed: false });
    expect(out[1]).toMatchObject({ running: false, failed: true });
  });

  it("labels by description, then sub-agent type, then a generic word", () => {
    const out = deriveSubagentChildren(
      map(
        task({ taskId: "a", description: "Audit the tests" }),
        task({ taskId: "b", description: "", subagentType: "Explore" }),
        task({ taskId: "c", description: "" }),
      ),
      T0,
    );
    expect(out.map((c) => c.label)).toEqual(["Audit the tests", "Explore", "sub-agent"]);
  });

  it("flags a running task as quiet only past the threshold, never a failed one", () => {
    const now = T0 + 10 * MIN;
    const out = deriveSubagentChildren(
      map(
        task({ taskId: "chatty", lastActivityAt: now - 2 * MIN }),
        task({ taskId: "quiet", lastActivityAt: now - 4 * MIN }),
        task({
          taskId: "failed",
          status: "failed",
          endedAt: now - 5 * MIN,
          lastActivityAt: now - 5 * MIN,
        }),
      ),
      now,
    );
    const by = Object.fromEntries(out.map((c) => [c.task.taskId, c]));
    expect(by.chatty.quietMs).toBeUndefined();
    expect(by.quiet.quietMs).toBe(4 * MIN);
    expect(by.failed.quietMs).toBeUndefined();
  });

  it("yields an empty list for a missing or empty map", () => {
    expect(deriveSubagentChildren(undefined, T0)).toEqual([]);
    expect(deriveSubagentChildren({}, T0)).toEqual([]);
  });
});

describe("tooltips", () => {
  it("list running labels, and failed labels with how they ended", () => {
    const children = deriveSubagentChildren(
      map(
        task({ taskId: "a", description: "Audit" }),
        task({ taskId: "b", description: "Lint", startedAt: T0 + 1 }),
        task({
          taskId: "c",
          description: "Ship",
          status: "failed",
          failureStatus: "stopped",
          summary: "killed by user",
          endedAt: T0,
        }),
        task({ taskId: "d", description: "Deploy", status: "failed", endedAt: T0 }),
      ),
      T0 + 1,
    );
    expect(runningTip(children)).toBe("Audit\nLint");
    expect(failedTip(children)).toBe("Ship — stopped: killed by user\nDeploy — failed");
  });
});
