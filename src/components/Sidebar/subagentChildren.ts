// Sidebar/subagentChildren.ts — derive an agent row's sub-agent children.
// Claude's backgrounded sub-agents (store/backgroundTasks) outlive the main
// turn, so an agent whose status reads `idle` may still have work in flight.
// This collapses that agent's task map into the ordered subset the AgentRow
// keeps working for, counts in its chips, and expands under itself — one
// child per top-level sub-agent that is running or failed recently. Same
// shape as stepChildren for a workflow run's step agents.

import {
  type BackgroundTask,
  type BackgroundTaskMap,
  isRecentTask,
  QUIET_AFTER_MS,
  quietForMs,
} from "@/adapters/shared/backgroundTasks";

export interface SubagentChild {
  task: BackgroundTask;
  /** The launch description, else the sub-agent type, else a generic label. */
  label: string;
  running: boolean;
  failed: boolean;
  /** How long the task has been silent, set only while running and past
   *  `QUIET_AFTER_MS`. A hint for the row — not a verdict on the task. */
  quietMs?: number;
}

/** Sub-agents the main agent launched itself. A sub-agent's own sub-agents
 *  and background Bash are its business, not the row's. */
function isTopLevelSubagent(task: BackgroundTask): boolean {
  return task.taskType === "local_agent" && !task.ownedBySubagent;
}

/** The top-level sub-agents worth a child row: running, or failed within
 *  `ENDED_TTL_MS`. Completed ones drop out at once (the chat's Task row shows
 *  their result), so the row settles as work finishes. Launch order. */
export function deriveSubagentChildren(
  tasks: BackgroundTaskMap | undefined,
  now: number,
): SubagentChild[] {
  if (!tasks) return [];
  return Object.values(tasks)
    .filter(
      (t) =>
        isTopLevelSubagent(t) &&
        (t.status === "running" || (t.status === "failed" && isRecentTask(t, now))),
    )
    .sort((a, b) => a.startedAt - b.startedAt || a.taskId.localeCompare(b.taskId))
    .map((task) => {
      const running = task.status === "running";
      const quiet = running ? quietForMs(task, now) : 0;
      return {
        task,
        label: task.description || task.subagentType || "sub-agent",
        running,
        failed: task.status === "failed",
        quietMs: quiet > QUIET_AFTER_MS ? quiet : undefined,
      };
    });
}

/** One line per running child, for the running chip's tooltip. */
export function runningTip(children: SubagentChild[]): string {
  return children
    .filter((c) => c.running)
    .map((c) => c.label)
    .join("\n");
}

/** One line per failed child — its label and how it ended — for the failed
 *  chip's tooltip and the child row's mark. */
export function failedTip(children: SubagentChild[]): string {
  return children
    .filter((c) => c.failed)
    .map((c) => {
      const how = c.task.failureStatus ?? "failed";
      return c.task.summary ? `${c.label} — ${how}: ${c.task.summary}` : `${c.label} — ${how}`;
    })
    .join("\n");
}
