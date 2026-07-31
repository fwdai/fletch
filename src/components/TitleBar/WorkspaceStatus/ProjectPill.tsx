import { useAppStore } from "@/store";
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
  return (
    <>
      <button
        type="button"
        className="ws-cap ws-cap-proj"
        title="Open project page"
        onClick={() => openProjectScreen(repoPath)}
      >
        <StatusDot status={status} />
        <span className="ws-proj-name">{name}</span>
      </button>
      <span className="ws-slash">/</span>
    </>
  );
}
