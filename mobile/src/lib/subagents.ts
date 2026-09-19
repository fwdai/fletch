// Derivations over an agent's background-task map for the list row, the
// header line and the chat strip: which sub-agents are worth showing, and the
// words for them. Pure, so the same numbers land on every surface.

import {
  type BackgroundTask,
  type BackgroundTaskMap,
  quietForMs,
} from "@desktop/adapters/shared/backgroundTasks";

/** A failure older than this has had its moment; the row goes quiet again. */
export const RECENT_FAILURE_MS = 10 * 60_000;

/** Past this silence the strip adds a "quiet Nm" hint. A nudge to look, not a
 *  verdict: a sub-agent deep in a long tool call is silent too. */
export const QUIET_HINT_MS = 3 * 60_000;

/** Sub-agents the main agent launched — not background Bash, and not the
 *  sub-agents a sub-agent launched, which thread under their parent's row. */
export const isTopLevelSubagent = (t: BackgroundTask) =>
  t.taskType === "local_agent" && !t.ownedBySubagent;

const failedRecently = (t: BackgroundTask, now: number) =>
  t.status === "failed" && now - (t.endedAt ?? t.lastActivityAt) < RECENT_FAILURE_MS;

/** What the strip lists: running sub-agents plus recent failures, oldest first. */
export function visibleSubagents(tasks: BackgroundTaskMap | undefined, now: number) {
  return Object.values(tasks ?? {})
    .filter((t) => isTopLevelSubagent(t) && (t.status === "running" || failedRecently(t, now)))
    .sort((a, b) => a.startedAt - b.startedAt);
}

export type SubagentCounts = { running: number; failed: number };

/** What a row needs: how many sub-agents are working, how many just failed. */
export function subagentCounts(tasks: BackgroundTaskMap | undefined, now: number): SubagentCounts {
  const counts = { running: 0, failed: 0 };
  for (const t of visibleSubagents(tasks, now)) {
    if (t.status === "running") counts.running += 1;
    else counts.failed += 1;
  }
  return counts;
}

export const subagentLabel = (n: number) => `${n} sub-agent${n === 1 ? "" : "s"}`;

export const subagentName = (t: BackgroundTask) => t.description || t.subagentType || "sub-agent";

/** "quiet 4m" once a running sub-agent has been silent past the threshold. */
export function quietHint(task: BackgroundTask, now: number): string | undefined {
  if (task.status !== "running") return undefined;
  const quiet = quietForMs(task, now);
  return quiet >= QUIET_HINT_MS ? `quiet ${Math.floor(quiet / 60_000)}m` : undefined;
}
