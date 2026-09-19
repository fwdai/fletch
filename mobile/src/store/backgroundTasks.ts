// Claude's background sub-agents (and background Bash), per agent — folded
// from the `system` task events on `agent:event` through the shared reducer.
// Never persisted: a task only means something while the host's process is
// alive, so the map starts empty on every handshake.

import { applyTaskEvent, type BackgroundTaskMap } from "@desktop/adapters/shared/backgroundTasks";
import type { RawEvent } from "../adapters";
import type { MobileState } from "./index";

const EMPTY: BackgroundTaskMap = {};

/** The state patch for one task event; empty when the event changed nothing. */
export function foldTaskEvent(
  s: MobileState,
  agentId: string,
  ev: RawEvent,
  now = Date.now(),
): Partial<MobileState> {
  const prev = s.backgroundTasks[agentId] ?? EMPTY;
  const next = applyTaskEvent(prev, ev, now);
  if (next === prev) return {};
  return { backgroundTasks: { ...s.backgroundTasks, [agentId]: next } };
}

/** Forget an agent's tasks: its process is gone (stopped, errored, deleted). */
export function dropTasks(s: MobileState, agentId: string): Partial<MobileState> {
  if (!s.backgroundTasks[agentId]) return {};
  const { [agentId]: _, ...rest } = s.backgroundTasks;
  return { backgroundTasks: rest };
}
