// The phone's words for a sub-agent thread, over the shared resolver
// (`@desktop/adapters/shared/subagents`): how a thread is addressed in a nav
// prop, and the one-line status its card and its screen's subtitle show.

import type { BackgroundTask, BackgroundTaskMap } from "@desktop/adapters/shared/backgroundTasks";
import type { ThreadState } from "@desktop/adapters/shared/subagents";
import { formatTokens } from "@desktop/util/format";
import { fmtElapsed } from "./hooks";
import { quietHint } from "./subagents";

/** Nav props are flat strings, so the chain of launching tool_use ids rides
 *  in one prop joined by this. Tool-use ids are alphanumeric, so it is safe. */
const SEP = "/";

export const joinThreadPath = (path: readonly string[]): string => path.join(SEP);

export const splitThreadPath = (prop: string | undefined): string[] =>
  prop ? prop.split(SEP).filter(Boolean) : [];

/** Background tasks keyed by the tool_use id that launched them — the key a
 *  transcript row looks itself up by. */
export function tasksByToolUse(
  tasks: BackgroundTaskMap | undefined,
): Record<string, BackgroundTask> {
  const map: Record<string, BackgroundTask> = {};
  for (const t of Object.values(tasks ?? {})) if (t.toolUseId) map[t.toolUseId] = t;
  return map;
}

/** Running: what it is on, with the strip's "quiet" hint past the threshold.
 *  Failed: how it ended. Done: how long it ran and what it cost — the two
 *  numbers that matter on a row with no room for more. */
export function threadStatus(
  state: ThreadState,
  task: BackgroundTask | undefined,
  now: number,
): string {
  if (state === "failed") return task?.failureStatus ?? "failed";
  const parts: string[] = [];
  if (state === "running") {
    if (task?.lastToolName) parts.push(task.lastToolName);
    const quiet = task && quietHint(task, now);
    if (quiet) parts.push(quiet);
    return parts.join(" · ") || "working";
  }
  if (task && task.durationMs > 0) parts.push(fmtElapsed(Math.round(task.durationMs / 1000)));
  if (task && task.totalTokens > 0) parts.push(`${formatTokens(task.totalTokens)} tokens`);
  return parts.join(" · ") || "done";
}
