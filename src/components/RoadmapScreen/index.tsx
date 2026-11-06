import type { IconName } from "@/components/Icon";
import { ProjectContext } from "@/components/ProjectContext";
import { ProjectHeader, ViewTabs } from "@/components/ProjectScreen/Header";
import { useAppStore } from "@/store";
import type { RoadmapTab } from "@/store/ui";
import { Activity } from "./Activity";
import { Counts } from "./Counts";
import { Roadmap, useRoadmap } from "./Roadmap";

const TABS: { id: RoadmapTab; label: string; icon: IconName }[] = [
  { id: "board", label: "Board", icon: "map" },
  { id: "activity", label: "Activity", icon: "activity" },
  { id: "context", label: "Context", icon: "graph" },
];

/** A project's roadmap, full screen: the board with the PM thread beside it,
 *  Activity — what has been built — and, while the layer's developer gate is
 *  open, Context — what the host knows about the project — as its tabs. */
export function RoadmapScreen({ repoPath }: { repoPath: string }) {
  const contextLayer = useAppStore((s) => s.contextLayerEnabled);
  // The one place the Context tab's gate is applied: with the layer off, a
  // tab left on Context shows the board.
  const tab = useAppStore((s) =>
    s.roadmapTab === "context" && !s.contextLayerEnabled ? "board" : s.roadmapTab,
  );
  const setTab = useAppStore((s) => s.setRoadmapTab);
  const projectId = useAppStore(
    (s) => s.workspace?.projects.find((p) => p.path === repoPath)?.project_id,
  );
  // Lives here, not in <Roadmap>, because the header shows the same counts —
  // and they move when the user accepts a proposal. Keyed by the repo, which
  // the hook resolves to the owning project (one roadmap per project).
  const roadmap = useRoadmap(repoPath);
  const tabs = contextLayer ? TABS : TABS.filter((t) => t.id !== "context");

  return (
    <div className="proj-screen">
      <ProjectHeader repoPath={repoPath} view="roadmap">
        <ViewTabs tabs={tabs} value={tab} onChange={setTab} />
        <Counts roadmap={roadmap} />
      </ProjectHeader>

      {tab === "board" ? (
        <Roadmap roadmap={roadmap} repoPath={repoPath} />
      ) : (
        <div className="ps-content">
          <div className="ps-sections">
            {projectId &&
              (tab === "activity" ? (
                <Activity projectId={projectId} />
              ) : (
                <ProjectContext projectId={projectId} />
              ))}
          </div>
        </div>
      )}
    </div>
  );
}
