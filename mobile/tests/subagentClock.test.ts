// A failed sub-agent is shown for RECENT_FAILURE_MS, then dropped. Nothing else
// need change in the store for that to happen, so the strip and the row have
// to re-render on their own until the failure has expired — and stop once
// there is nothing left to show.
//
// `createElement` rather than JSX because the suite's glob is `tests/**/*.test.ts`.
import type { BackgroundTask } from "@desktop/adapters/shared/backgroundTasks";
import type { AgentRecord } from "@desktop/api/types/agent";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentRow } from "../src/components/AgentRow";
import { RECENT_FAILURE_MS, subagentTickMs } from "../src/lib/subagents";
import { SubagentStrip } from "../src/screens/Agent/SubagentStrip";
import { useStore } from "../src/store";

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

const failed = () => task({ status: "failed", failureStatus: "stopped", endedAt: NOW });
const map = (...tasks: BackgroundTask[]) => Object.fromEntries(tasks.map((t) => [t.taskId, t]));

describe("subagentTickMs", () => {
  it("ticks each second while a sub-agent runs or the surface has its own timer", () => {
    expect(subagentTickMs({ running: 1, failed: 0 })).toBe(1_000);
    expect(subagentTickMs({ running: 1, failed: 1 })).toBe(1_000);
    expect(subagentTickMs({ running: 0, failed: 0 }, true)).toBe(1_000);
  });

  it("keeps a minute tick while only a recent failure is shown", () => {
    expect(subagentTickMs({ running: 0, failed: 1 })).toBe(60_000);
  });

  it("runs no timer with nothing to show", () => {
    expect(subagentTickMs({ running: 0, failed: 0 })).toBeUndefined();
  });
});

describe("rendered expiry", () => {
  let host: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useFakeTimers({ now: NOW });
    // ProviderMark fetches its brand icon; a miss falls back to the monogram.
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: false, status: 404 }) as Response),
    );
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    host.remove();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  const advance = (ms: number) =>
    act(async () => {
      vi.advanceTimersByTime(ms);
    });

  it("drops a failed sub-agent from the strip once its moment has passed", async () => {
    await act(async () => {
      root.render(createElement(SubagentStrip, { tasks: map(failed()), onJump: () => {} }));
    });
    expect(host.querySelector(".subagent.err")).not.toBeNull();
    expect(vi.getTimerCount()).toBe(1);

    await advance(RECENT_FAILURE_MS - 60_000);
    expect(host.querySelector(".subagent.err")).not.toBeNull();

    await advance(2 * 60_000);
    expect(host.querySelector(".subagents")).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("drops the row's failure chip the same way", async () => {
    const agent = {
      id: "a",
      name: "Audit",
      status: "idle",
      provider: "claude",
      task: "audit the store",
      project_id: "p1",
      created_at: "2026-01-01T00:00:00.000Z",
      repos: [],
    } as unknown as AgentRecord;
    useStore.setState({ backgroundTasks: { a: map(failed()) } });
    await act(async () => {
      root.render(createElement(AgentRow, { agent, onClick: () => {} }));
    });
    expect(host.textContent).toContain("sub-agent failed");

    await advance(RECENT_FAILURE_MS + 60_000);
    expect(host.textContent).not.toContain("sub-agent failed");
    expect(vi.getTimerCount()).toBe(0);
  });
});
