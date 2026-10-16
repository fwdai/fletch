import { useState } from "react";
import type { AgentRecord } from "@/api";
import { GitRepoSection } from "./GitRepoSection";
import { scopeOf, useWorkspaceLanes } from "./hooks/useWorkspaceLanes";
import { defaultLane } from "./laneModel";

/** State-aware git panel driven by live git state from the Tauri backend: one
 *  header of lanes, one ladder, one footer, whatever the number of repos. The
 *  section describes the lane in view and acts on its checkout. */
export function GitPanel({ agent }: { agent: AgentRecord }) {
  const { lanes, scopes } = useWorkspaceLanes(agent);

  // Which lane the ladder describes: the user's pick while it exists, else the
  // default. Local to the panel — viewing a lane never moves the host's focus.
  // The pick remembers which agent it was made for, so a switch to another
  // workspace starts from that one's default rather than a stale lane id.
  const [pick, setPick] = useState<{ agentId: string; laneId: string } | null>(null);
  const picked = pick?.agentId === agent.id ? pick.laneId : null;
  const viewed = (picked ? lanes.find((l) => l.id === picked) : undefined) ?? defaultLane(lanes);
  const scope = scopeOf(viewed, scopes);

  return (
    <GitRepoSection
      agent={agent}
      repo={scope?.repo}
      subdir={scope?.subdir}
      lanes={lanes}
      viewed={viewed}
      onView={(laneId) => setPick({ agentId: agent.id, laneId })}
    />
  );
}
