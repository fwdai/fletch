import { ProjectIcon } from "@/components/ProjectIcon";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";

/** The left half of the capsule: the project's icon + display name in a pill
 *  that opens the project's roadmap, plus the `/` separator to the workspace
 *  pill beside it. The agent's status belongs to the workspace, not here. */
export function ProjectPill({ repoPath, name }: { repoPath: string; name: string }) {
  const openRoadmap = useAppStore((s) => s.openRoadmap);
  const project = useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath));
  // The page behind this pill is the roadmap, which a host does not answer for.
  // The pill stays — it is how the user knows which project they are in — but
  // it stops being a door, and says why.
  const roadmapGate = useGate("roadmap");
  return (
    <>
      <button
        type="button"
        className="ws-cap ws-cap-proj"
        title={roadmapGate ?? "Open roadmap"}
        disabled={roadmapGate !== null}
        onClick={() => openRoadmap(repoPath)}
      >
        <ProjectIcon project={project ?? { name }} size={12} />
        <span className="ws-proj-name">{name}</span>
      </button>
      <span className="ws-slash">/</span>
    </>
  );
}
