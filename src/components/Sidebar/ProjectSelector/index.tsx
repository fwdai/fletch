import { Icon } from "@/components/Icon";
import { ProjectIcon } from "@/components/ProjectIcon";
import { MenuButton } from "@/components/ui/MenuButton";
import type { ProjectFocus } from "../projectFocus";
import type { ProjectOption } from "./ProjectOptionRow";
import { ProjectSelectorMenu } from "./ProjectSelectorMenu";

export type { ProjectOption };

interface Props {
  /** Every project, in sidebar order. */
  projects: readonly ProjectOption[];
  /** The keys the sidebar is narrowed to; empty for all projects. */
  focus: ProjectFocus;
  onChange: (focus: string[]) => void;
}

/** Which projects the sidebar lists: all of them, one, or a chosen few. The
 *  trigger names the current view and is tinted while a focus is on, so a
 *  narrowed list never looks like projects went missing. */
export function ProjectSelector({ projects, focus, onChange }: Props) {
  const only = focus.length === 1 ? projects.find((p) => p.key === focus[0]) : undefined;
  const label =
    focus.length === 0 ? "All projects" : only ? only.label : `${focus.length} projects`;

  return (
    <MenuButton
      anchorClassName="proj-select"
      trigger={(trigger) => (
        <button
          type="button"
          className={`proj-select-btn flex-center text-base ${focus.length > 0 ? "on" : ""}`}
          {...trigger}
        >
          {only ? <ProjectIcon project={only.icon} size={14} /> : <Icon name="layers" size={13} />}
          <span className="proj-select-label truncate">{label}</span>
          <Icon name="chevD" size={12} />
        </button>
      )}
    >
      {(close) => (
        <ProjectSelectorMenu projects={projects} focus={focus} onChange={onChange} close={close} />
      )}
    </MenuButton>
  );
}
