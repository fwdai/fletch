// The slice keys Claude's background tasks by agent and drops one agent's map
// without disturbing the others. The reducer itself is covered in
// tests/adapters/shared/backgroundTasks.test.ts.

import { describe, expect, it } from "vitest";
import { create } from "zustand";
import type { RawEvent } from "@/adapters";
import { createBackgroundTasksSlice } from "./backgroundTasks";
import type { AppState } from "./types";

const started = (task_id: string): RawEvent => ({
  type: "system",
  subtype: "task_started",
  task_id,
  tool_use_id: `toolu_${task_id}`,
  description: "Sleep and reply task",
  is_backgrounded: true,
  task_type: "local_agent",
});

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createBackgroundTasksSlice(...a) }) as AppState);

describe("backgroundTasks slice", () => {
  it("keys tasks by agent and clears one agent's tasks without touching others", () => {
    const store = makeStore();
    store.getState().applyBackgroundTaskEvent("agent-1", started("a"));
    store.getState().applyBackgroundTaskEvent("agent-2", started("z"));
    expect(store.getState().backgroundTasks["agent-1"]?.a.status).toBe("running");

    // A non-task event leaves the map reference untouched.
    const before = store.getState().backgroundTasks;
    store.getState().applyBackgroundTaskEvent("agent-1", { type: "assistant" });
    expect(store.getState().backgroundTasks).toBe(before);

    store.getState().clearBackgroundTasks("agent-1");
    expect(store.getState().backgroundTasks["agent-1"]).toBeUndefined();
    expect(store.getState().backgroundTasks["agent-2"]?.z.status).toBe("running");

    // Clearing an unknown agent is a no-op.
    const after = store.getState().backgroundTasks;
    store.getState().clearBackgroundTasks("nobody");
    expect(store.getState().backgroundTasks).toBe(after);
  });
});
