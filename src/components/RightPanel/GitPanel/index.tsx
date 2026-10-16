import { useState } from "react";
import type { AgentRecord } from "@/api";
import { checkoutKey } from "@/store/git";
import { AutopilotSwitch } from "./Autopilot";
import { GitRepoSection } from "./GitRepoSection";
import { scopeOf, useWorkspaceLanes } from "./hooks/useWorkspaceLanes";
import { Lanes } from "./Lanes";
import { defaultLane } from "./laneModel";

/** State-aware git panel driven by live git state from the Tauri backend: one
 *  header of lanes, one ladder, one footer, whatever the number of repos.
 *
 *  The lanes are the workspace's, so they live here, above the section, and
 *  survive a change of checkout — keyboard focus on a lane stays where it is
 *  when the arrow keys cross into another repo. The section describes the
 *  lane in view and acts on its checkout, and is mounted per checkout, so
 *  everything transient in it (a push in flight, its notice, a half-written
 *  commit message) is that checkout's and never shows on another. */
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
    <div className="git-wrap">
      <Lanes
        lanes={lanes}
        viewedId={viewed?.id ?? null}
        onView={(laneId) => setPick({ agentId: agent.id, laneId })}
        controls={<AutopilotSwitch agentId={agent.id} projectId={agent.project_id} />}
      />
      <GitRepoSection
        key={checkoutKey(agent.id, scope?.subdir)}
        agent={agent}
        repo={scope?.repo}
        subdir={scope?.subdir}
        viewed={viewed}
      />
    </div>
  );
}
