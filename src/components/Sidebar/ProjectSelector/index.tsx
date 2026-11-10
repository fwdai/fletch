import { Icon } from "@/components/Icon";
import { MenuButton } from "@/components/ui/MenuButton";
import type { ProjectFocus } from "../projectFocus";
import type { ProjectSignal } from "../projectSignal";
import type { ProjectOption } from "./ProjectOptionRow";
import { ProjectSelectorMenu } from "./ProjectSelectorMenu";
import { SignalDot } from "./SignalDot";

export type { ProjectOption };

interface Props {
  /** Every project, in sidebar order. */
  projects: readonly ProjectOption[];
  /** The keys the sidebar is narrowed to; empty for all projects. */
  focus: ProjectFocus;
  /** The strongest signal among the projects the focus keeps off screen. */
  hiddenSignal: ProjectSignal | null;
  onChange: (focus: string[]) => void;
}

/** Which projects the sidebar lists: all of them, one, or a chosen few.
 *
 *  The trigger says what the focus is — "All projects", or "2 of 6
 *  projects" — so a narrowed list never looks like projects went missing. It
 *  counts rather than names: the focused projects' own headers are right
 *  below it, and the menu ticks which ones they are.
 *
 *  What a focus hides can still need the user, so the trigger wears a dot
 *  while an agent in a hidden project is waiting, has unopened results or is
 *  working; the menu marks which project it is. */
export function ProjectSelector({ projects, focus, hiddenSignal, onChange }: Props) {
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
          {hiddenSignal && <SignalDot signal={hiddenSignal} where="in a hidden project" />}
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
