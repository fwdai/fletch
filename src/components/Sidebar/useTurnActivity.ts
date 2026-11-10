import { useEffect, useState } from "react";
import type { AgentRecord } from "@/api";
import { type AgentActivity, loadAgentActivity, stampAgentActivity } from "@/storage/agentActivity";
import {
  loadProjectActivity,
  type ProjectActivity,
  stampProjectActivity,
} from "@/storage/projectActivity";
import { useAppStore } from "@/store";
import { RECENT_WINDOW_MS } from "./foldAgents";

/** Whether `stamps` holds a later time than `prev` for any id. Only that is a
 *  change; otherwise every groups update would re-stamp (and re-render). */
const isNewer = (stamps: Record<string, number>, prev: Record<string, number>) =>
  Object.entries(stamps).some(([k, t]) => t > (prev[k] ?? 0));

/** When a turn last started, per project and per workspace.
 *
 *  Launches are on the agent records; turns are not. So each turn start the
 *  store reports is stamped here, on the workspace and on its project, and
 *  kept locally. A resumed idle workspace then counts as activity the moment
 *  its message is sent: its project floats to the top ("active projects
 *  first") and the workspace itself comes out of the "older" fold. */
export function useTurnActivity(
  groups: readonly { key: string; agents: readonly AgentRecord[] }[],
): { projectTurns: ProjectActivity; agentTurns: AgentActivity } {
  const turnStartedAt = useAppStore((s) => s.turnStartedAt);
  const [projectTurns, setProjectTurns] = useState(loadProjectActivity);
  const [agentTurns, setAgentTurns] = useState(loadAgentActivity);

  useEffect(() => {
    const projects: ProjectActivity = {};
    const agents: AgentActivity = {};
    for (const g of groups) {
      for (const a of g.agents) {
        const t = turnStartedAt[a.id];
        if (t === undefined) continue;
        agents[a.id] = t;
        if (t > (projects[g.key] ?? 0)) projects[g.key] = t;
      }
    }
    setProjectTurns((prev) => (isNewer(projects, prev) ? stampProjectActivity(projects) : prev));
    setAgentTurns((prev) =>
      isNewer(agents, prev) ? stampAgentActivity(agents, Date.now() - RECENT_WINDOW_MS) : prev,
    );
  }, [groups, turnStartedAt]);

  return { projectTurns, agentTurns };
}
