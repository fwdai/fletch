import { Icon } from "@/components/Icon";
import { MenuButton } from "@/components/ui/MenuButton";
import type { ProjectFocus } from "../projectFocus";
import { strongestSignal } from "../projectSignal";
import type { ProjectOption } from "./ProjectOptionRow";
import { ProjectSelectorMenu } from "./ProjectSelectorMenu";
import { SignalDot } from "./SignalDot";

export type { ProjectOption };

interface Props {
  /** Every project, in sidebar order. */
  projects: readonly ProjectOption[];
  /** The keys the sidebar is narrowed to; empty for all projects. */
  focus: ProjectFocus;
  onChange: (focus: string[]) => void;
  /** The short label ("All", "2 of 6"), for a narrow sidebar. */
  compact?: boolean;
}

/** Which projects the sidebar lists: all of them, one, or a chosen few. The
 *  leading segment of the list filter's field (`ListFilter`), which its menu
 *  spans.
 *
 *  The trigger says what the focus is — "All projects", or "2 of 6
 *  projects" — so a narrowed list never looks like projects went missing. It
 *  counts rather than names: the focused projects' own headers are right
 *  below it, and the menu ticks which ones they are.
 *
 *  What a focus hides can still need the user, so the trigger wears a dot
 *  while an agent in a hidden project is waiting, has unopened results or is
 *  working; the menu marks which project it is. */
export function ProjectSelector({ projects, focus, onChange, compact = false }: Props) {
  // With no focus on nothing is hidden: every project's rows are in the list.
  const hiddenSignal =
    focus.length === 0
      ? null
      : strongestSignal(projects.filter((p) => !focus.includes(p.key)).map((p) => p.signal));
  const count = focus.length === 0 ? "All" : `${focus.length} of ${projects.length}`;
  const label = compact ? count : `${count} projects`;

  return (
    <MenuButton
      anchorClassName="proj-select"
      trigger={(trigger) => (
        <button
          type="button"
          className={`side-scope flex-center text-base tip ${focus.length > 0 ? "on" : ""}`}
          data-tip="Choose the projects to show"
          aria-label={`Projects shown: ${count === "All" ? "all" : count}`}
          {...trigger}
        >
          <span className="side-scope-label truncate">{label}</span>
          {hiddenSignal && <SignalDot signal={hiddenSignal} where="in a hidden project" />}
          <Icon name="chevD" size={11} />
        </button>
      )}
    >
      {(close) => (
        <ProjectSelectorMenu projects={projects} focus={focus} onChange={onChange} close={close} />
      )}
    </MenuButton>
  );
}
