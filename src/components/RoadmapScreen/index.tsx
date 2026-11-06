import { ProjectHeader } from "@/components/ProjectScreen/Header";
import { useAppStore } from "@/store";
import type { RoadmapTab } from "@/store/ui";
import { Activity } from "./Activity";
import { Counts } from "./Counts";
import { Roadmap, useRoadmap } from "./Roadmap";

const TABS: { id: RoadmapTab; label: string }[] = [
  { id: "board", label: "Board" },
  { id: "activity", label: "Activity" },
];

/** A project's roadmap, full screen: the board with the PM thread beside it,
 *  and Activity — what has been built — as its second sub-tab. */
export function RoadmapScreen({ repoPath }: { repoPath: string }) {
  const tab = useAppStore((s) => s.roadmapTab);
  const setTab = useAppStore((s) => s.setRoadmapTab);
  const projectId = useAppStore(
    (s) => s.workspace?.projects.find((p) => p.path === repoPath)?.project_id,
  );
  // Lives here, not in <Roadmap>, because the header shows the same counts —
  // and they move when the user accepts a proposal. Keyed by the repo, which
  // the hook resolves to the owning project (one roadmap per project).
  const roadmap = useRoadmap(repoPath);

  return (
    <div className="proj-screen">
      <ProjectHeader repoPath={repoPath} view="roadmap">
        <nav className="ps-nav">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              className={`ps-tab iflex-center text-sm ${tab === t.id ? "active" : ""}`}
              onClick={() => setTab(t.id)}
            >
              {t.label}
            </button>
          ))}
        </nav>
        <Counts roadmap={roadmap} />
      </ProjectHeader>

      {tab === "board" ? (
        <Roadmap roadmap={roadmap} repoPath={repoPath} />
      ) : (
        <div className="ps-content">
          <div className="ps-sections">{projectId && <Activity projectId={projectId} />}</div>
        </div>
      )}
    </div>
  );
}
