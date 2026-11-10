import { Icon } from "@/components/Icon";
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

/** Which projects the sidebar lists: all of them, one, or a chosen few.
 *
 *  The trigger says how much of the list is in view — "All projects", or
 *  "2 of 6 projects" — so a narrowed list never looks like projects went
 *  missing. It counts rather than names: the focused projects' own headers
 *  are right below it, and the menu ticks which ones they are. */
export function ProjectSelector({ projects, focus, onChange }: Props) {
  const label =
    focus.length === 0 ? "All projects" : `${focus.length} of ${projects.length} projects`;

  return (
    <MenuButton
      anchorClassName="proj-select"
      trigger={(trigger) => (
        <button
          type="button"
          className={`proj-select-btn flex-center text-base ${focus.length > 0 ? "on" : ""}`}
          {...trigger}
        >
          <Icon name="layers" size={13} />
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
