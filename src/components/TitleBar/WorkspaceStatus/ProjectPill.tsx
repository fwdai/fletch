import { ProjectIcon } from "@/components/ProjectIcon";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import type { DotStatus } from "./derive";
import { StatusDot } from "./StatusDot";

/** The left half of the capsule: status dot + project display name in a pill
 *  that opens the full-screen project page, plus the `/` separator to the
 *  workspace pill beside it. */
export function ProjectPill({
  repoPath,
  name,
  status,
}: {
  repoPath: string;
  name: string;
  status: DotStatus;
}) {
  const openProjectScreen = useAppStore((s) => s.openProjectScreen);
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
        title={roadmapGate ?? "Open project page"}
        disabled={roadmapGate !== null}
        onClick={() => openProjectScreen(repoPath)}
      >
        <StatusDot status={status} />
        <ProjectIcon project={project ?? { name }} size={12} />
        <span className="ws-proj-name">{name}</span>
      </button>
      <span className="ws-slash">/</span>
    </>
  );
}
