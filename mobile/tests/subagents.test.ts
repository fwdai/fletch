// Live sub-agent visibility: the pure derivations behind the list row and the
// chat strip, the store's fold/drop helpers, and the mock host's scripted
// sub-agent turn end to end (jsdom URL in vite.config.ts puts this in mock
// mode).

import type { BackgroundTask } from "@desktop/adapters/shared/backgroundTasks";
import { beforeAll, describe, expect, it, vi } from "vitest";
import {
  QUIET_HINT_MS,
  quietHint,
  RECENT_FAILURE_MS,
  subagentCounts,
  subagentLabel,
  visibleSubagents,
} from "../src/lib/subagents";
import { agentOf, type MobileState, useStore } from "../src/store";
import { dropTasks, foldTaskEvent } from "../src/store/backgroundTasks";

const NOW = 1_700_000_000_000;

const task = (over: Partial<BackgroundTask>): BackgroundTask => ({
  taskId: over.taskId ?? "t",
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

const map = (...tasks: BackgroundTask[]) => Object.fromEntries(tasks.map((t) => [t.taskId, t]));

describe("subagentCounts", () => {
  it("counts only the main agent's own sub-agents", () => {
    const tasks = map(
      task({ taskId: "a" }),
      task({ taskId: "b" }),
      task({ taskId: "bash", taskType: "local_bash" }),
      task({ taskId: "nested", ownedBySubagent: true }),
      task({ taskId: "done", status: "completed", endedAt: NOW }),
    );
    expect(subagentCounts(tasks, NOW)).toEqual({ running: 2, failed: 0 });
  });

  it("keeps a failure on the row for a while, then lets it go", () => {
    const fresh = task({ taskId: "f", status: "failed", failureStatus: "stopped", endedAt: NOW });
    expect(subagentCounts(map(fresh), NOW)).toEqual({ running: 0, failed: 1 });
    expect(subagentCounts(map(fresh), NOW + RECENT_FAILURE_MS)).toEqual({ running: 0, failed: 0 });
  });

  it("is quiet with no map at all", () => {
    expect(subagentCounts(undefined, NOW)).toEqual({ running: 0, failed: 0 });
  });
});

describe("visibleSubagents", () => {
  it("lists running and recently failed sub-agents, oldest first", () => {
    const tasks = map(
      task({ taskId: "late", startedAt: NOW - 1_000 }),
      task({ taskId: "early", startedAt: NOW - 9_000 }),
      task({ taskId: "failed", status: "failed", startedAt: NOW - 5_000, endedAt: NOW }),
      task({ taskId: "done", status: "completed", endedAt: NOW }),
    );
    expect(visibleSubagents(tasks, NOW).map((t) => t.taskId)).toEqual(["early", "failed", "late"]);
  });
});

describe("words", () => {
  it("pluralises the row label", () => {
    expect(subagentLabel(1)).toBe("1 sub-agent");
    expect(subagentLabel(2)).toBe("2 sub-agents");
  });

  it("hints at silence only past the threshold, and never for a finished task", () => {
    expect(quietHint(task({}), NOW)).toBeUndefined();
    const silent = task({ lastActivityAt: NOW - QUIET_HINT_MS - 61_000 });
    expect(quietHint(silent, NOW)).toBe("quiet 4m");
    expect(quietHint({ ...silent, status: "completed" }, NOW)).toBeUndefined();
  });
});

describe("store helpers", () => {
  const started = {
    type: "system",
    subtype: "task_started",
    task_id: "t1",
    tool_use_id: "toolu_1",
    description: "Audit",
    task_type: "local_agent",
    is_backgrounded: true,
  };

  it("folds a task event into the agent's map and drops it with the agent", () => {
    const s0 = { backgroundTasks: {} } as MobileState;
    const patch = foldTaskEvent(s0, "a", started, NOW);
    expect(patch.backgroundTasks?.a.t1).toMatchObject({ status: "running", toolUseId: "toolu_1" });
    const s1 = { ...s0, ...patch } as MobileState;
    // An unrelated frame is a no-op patch, not a fresh map.
    expect(foldTaskEvent(s1, "a", { type: "assistant" }, NOW)).toEqual({});
    expect(dropTasks(s1, "a")).toEqual({ backgroundTasks: {} });
    expect(dropTasks(s1, "other")).toEqual({});
  });
});

describe("over the mock host", () => {
  const state = () => useStore.getState();

  beforeAll(async () => {
    await state().init();
    await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
  });

  it("keeps a sub-agent running after the turn, threads its turns, then ends it", async () => {
    await state().send("kamakura", "delegate the store audit to a sub-agent");
    const running = () => Object.values(state().backgroundTasks.kamakura ?? {})[0];
    // The main turn ends with the sub-agent still going: idle, yet working.
    await vi.waitFor(
      () => {
        expect(agentOf(state(), "kamakura")?.status).toBe("idle");
        expect(running()?.status).toBe("running");
      },
      { timeout: 5000, interval: 2 },
    );
    expect(subagentCounts(state().backgroundTasks.kamakura, Date.now()).running).toBe(1);
    await vi.waitFor(() => expect(running()?.status).toBe("completed"), { timeout: 5000 });
    const done = running();
    expect(done.toolUses).toBe(2);
    expect(done.summary).toContain("turn:sent");
    // The sub-agent's own turns landed under the Agent row, not in the log.
    const row = (state().logs.kamakura ?? []).find(
      (i) => i.kind === "tool_call" && i.id === done.toolUseId,
    );
    expect(row?.kind === "tool_call" && (row.children?.length ?? 0)).toBeGreaterThan(0);
    expect(state().logs.kamakura?.some((i) => i.kind === "tool_call" && i.name === "Grep")).toBe(
      false,
    );
  });

  it("empties the map on a fresh handshake", async () => {
    useStore.setState({ backgroundTasks: { x: { t: {} as BackgroundTask } } });
    await state().reconnect();
    await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
    expect(state().backgroundTasks).toEqual({});
  });
});
