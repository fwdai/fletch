import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";
import { ProjectIcon } from "@/components/ProjectIcon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import type { ProjectView } from "@/store/ui";
import { basename } from "@/util/format";

/** Each project view's name, as its header and the title bar's crumb say it. */
export const VIEW_LABEL: Record<ProjectView, string> = {
  roadmap: "Roadmap",
  context: "Context",
  settings: "Settings",
};

/** The thin bar over each of a project's full-screen views: back, the
 *  project's mark and name, and the view's name, muted. `children` fill the
 *  centre and right zones (the roadmap's sub-tabs and counts). */
export function ProjectHeader({
  repoPath,
  view,
  children,
}: {
  repoPath: string;
  view: ProjectView;
  children?: ReactNode;
}) {
  const close = useAppStore((s) => s.closeProjectScreen);
  const project = useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath));
  const name = project?.name ?? basename(repoPath);

  return (
    <div className="ps-head">
      <div className="ps-ident flex-center">
        <IconButton tip="Back to app" onClick={close}>
          <Icon name="chevL" size={13} />
        </IconButton>
        <ProjectIcon project={project ?? { name }} />
        <span className="ps-name text-base truncate">{name}</span>
        <span className="ps-view text-base">{VIEW_LABEL[view]}</span>
      </div>
      {children}
    </div>
  );
}
