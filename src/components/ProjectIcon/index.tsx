import { api } from "@/api";
import type { ProjectRef } from "@/api/types/agent";
import { useProjectIconData } from "./iconCache";
import { ProjectMark } from "./ProjectMark";

/** What a placement knows about the project: a `ProjectRef`, or just a name
 *  where the project is not in the snapshot (the mark is then the initial). */
export type IconSubject = Pick<ProjectRef, "name"> &
  Partial<Pick<ProjectRef, "project_id" | "icon_hash">>;

/** The project's icon, fetched by hash from the environment the UI drives. */
export function ProjectIcon({ project, size }: { project: IconSubject; size?: number }) {
  const icon = useProjectIconData(project.project_id, project.icon_hash, api.projectIconGet);
  return <ProjectMark name={project.name} icon={icon} size={size} />;
}
