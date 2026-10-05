// SubagentThread/thread.ts — the desktop's view of a sub-agent thread.
//
// Resolving a thread out of a log (by the chain of launching tool_use ids,
// see `openThread` in store/ui), its label, its result and its live state are
// shared with the phone (adapters/shared/subagents) and re-exported here so
// the pane's modules have one import. What is desktop-only is the status
// line, which leans on the chat's own formatters.

import type { ThreadState } from "@/adapters/shared/subagents";
import { type BackgroundTask, QUIET_AFTER_MS, quietForMs } from "@/store";
import { formatDuration, formatTokens } from "@/util/format";
import { fmtDur } from "../RunTimer";

export {
  isSubagentCall,
  type ResolvedThread,
  resolveThread,
  type ThreadState,
  threadLabel,
  threadResult,
  threadState,
  threadSteps,
  threadType,
} from "@/adapters/shared/subagents";

/** The one-line status a card and the thread header both show. Running: how
 *  far it has got, what it is on, and a "quiet" hint past `QUIET_AFTER_MS`
 *  (a heuristic — a long step goes quiet too). Failed: how it ended. Done:
 *  the size of the run. Empty parts are left out rather than shown as zero;
 *  pass `steps` as 0 to leave the count out altogether (the card does). */
export function threadStatusText(
  state: ThreadState,
  steps: number,
  task: BackgroundTask | undefined,
  now: number,
): string {
  const parts: string[] = [];
  if (state === "failed") {
    parts.push(task?.failureStatus ?? "failed");
    if (task?.summary) parts.push(task.summary);
    return parts.join(" — ");
  }
  if (steps > 0) parts.push(`${steps} step${steps === 1 ? "" : "s"}`);
  if (state === "running") {
    if (task?.lastToolName) parts.push(task.lastToolName);
    const quiet = task?.status === "running" ? quietForMs(task, now) : 0;
    if (quiet > QUIET_AFTER_MS) parts.push(`quiet ${formatDuration(quiet)}`);
    if (parts.length === 0) parts.push("working");
  } else {
    if (task && task.durationMs > 0) parts.push(fmtDur(task.durationMs / 1000));
    if (task && task.totalTokens > 0) parts.push(`${formatTokens(task.totalTokens)} tokens`);
    if (parts.length === 0) parts.push("done");
  }
  return parts.join(" · ");
}
