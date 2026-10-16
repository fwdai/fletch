import type { AgentStatus } from "@/api";

/** The four glanceable states the capsule dot collapses to. Distinct from
 *  AgentStatus: `waiting` is derived (running + a pending question), and
 *  spawning/stopped fold into running/idle. */
export type DotStatus = "running" | "waiting" | "idle" | "error";

export const STATUS_LABEL: Record<DotStatus, string> = {
  running: "Working",
  waiting: "Waiting for input",
  idle: "Idle",
  error: "Failed",
};

/** Map the agent's run status (+ a pending question) to the capsule dot.
 *  `awaiting` mirrors the sidebar row: running with an unanswered prompt is
 *  "your court", not "still working". */
export function dotStatus(status: AgentStatus, awaiting: boolean): DotStatus {
  if (awaiting) return "waiting";
  if (status === "running" || status === "spawning") return "running";
  if (status === "error") return "error";
  return "idle";
}

/** [`dotStatus`] fed straight from store state: `pending` is the agent's
 *  `pendingToolUse` entry, and a question only counts as awaiting while the
 *  agent is actually working. */
export function agentDotStatus(
  status: AgentStatus,
  pending: Record<string, string> | undefined,
): DotStatus {
  const working = status === "running" || status === "spawning";
  return dotStatus(status, working && Object.keys(pending ?? {}).length > 0);
}
