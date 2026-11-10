import { Icon } from "@/components/Icon";
import { type IconSubject, ProjectIcon } from "@/components/ProjectIcon";

/** A project as the selector lists it. */
export interface ProjectOption {
  /** The sidebar group key the focus is stored by. */
  key: string;
  label: string;
  /** What `ProjectIcon` draws the mark from. */
  icon: IconSubject;
  /** Workspaces, drafts and runs in the project. */
  count: number;
}

interface Props {
  project: ProjectOption;
  checked: boolean;
  /** The checkbox: add the project to the focus or take it out. */
  onToggle: () => void;
  /** The name: focus on this project alone. */
  onOnly: () => void;
}

/** One project in the selector's menu, with two targets: the name narrows the
 *  sidebar to that project and is the common case, so it is the big one; the
 *  checkbox builds a set of several. */
export function ProjectOptionRow({ project, checked, onToggle, onOnly }: Props) {
  return (
    <div className="dd-item proj-opt flex-center">
      <button
        type="button"
        role="menuitemcheckbox"
        aria-checked={checked}
        aria-label={`Show ${project.label}`}
        className="proj-opt-check flex-center"
        onClick={onToggle}
      >
        <span className="proj-opt-box flex-center">
          {checked && <Icon name="check" size={10} />}
        </span>
      </button>
      <button
        type="button"
        role="menuitem"
        aria-label={`Only ${project.label}`}
        className="proj-opt-name flex-center"
        onClick={onOnly}
      >
        <ProjectIcon project={project.icon} size={14} />
        <span className="di-l">{project.label}</span>
        <span className="di-m">{project.count}</span>
      </button>
    </div>
  );
}
