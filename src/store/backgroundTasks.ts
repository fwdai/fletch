// Desktop store wiring for Claude's background tasks (sub-agents, background
// Bash), keyed per agent. The lifecycle reducer and selectors live in
// adapters/shared/backgroundTasks so the mobile app can fold the same events
// through its own store; this slice only owns the state key and its cleanup.
//
// Fed from `onAgentEvent` (store/eventListeners) for `system` task events.
// Cleared when the agent's process stops or errors (`onAgentStatus`), on
// discard (`dropAgentEntries`), and stashed per environment (environmentSwitch).

import type { RawEvent } from "@/adapters";
import { applyTaskEvent, type BackgroundTaskMap } from "@/adapters/shared/backgroundTasks";
import type { SliceCreator } from "./types";

export interface BackgroundTasksSlice {
  /** Per agent, per task_id. Ended tasks stay in the map so a failure remains
   *  visible after the fact; the UI decides how long to show them. */
  backgroundTasks: Record<string, BackgroundTaskMap>;
  /** Fold one `agent:event` for `agentId` in; a no-op for non-task events. */
  applyBackgroundTaskEvent: (agentId: string, ev: RawEvent) => void;
  clearBackgroundTasks: (agentId: string) => void;
}

export const createBackgroundTasksSlice: SliceCreator<BackgroundTasksSlice> = (set) => ({
  backgroundTasks: {},

  applyBackgroundTaskEvent: (agentId, ev) =>
    set((state) => {
      const prev = state.backgroundTasks[agentId] ?? {};
      const next = applyTaskEvent(prev, ev, Date.now());
      if (next === prev) return {};
      return { backgroundTasks: { ...state.backgroundTasks, [agentId]: next } };
    }),

  clearBackgroundTasks: (agentId) =>
    set((state) => {
      if (!(agentId in state.backgroundTasks)) return {};
      const { [agentId]: _gone, ...backgroundTasks } = state.backgroundTasks;
      return { backgroundTasks };
    }),
});
