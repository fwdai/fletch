import { useAppStore } from "@/store";
import { AddProject } from "./AddProject";
import { EnvironmentSwitcher } from "./EnvironmentSwitcher";
import { ListFilter } from "./ListFilter";
import type { ProjectOption } from "./ProjectSelector";
import type { ProjectFocus } from "./projectFocus";

/** Below this the header drops what it can spare: the scope's long label and
 *  the filter's keycap. */
const COMPACT_BELOW = 300;

interface Props {
  projects: readonly ProjectOption[];
  focus: ProjectFocus;
  onFocusChange: (focus: string[]) => void;
  query: string;
  onQueryChange: (q: string) => void;
  /** ↓ pressed in the filter: hand focus to the list below. */
  onArrowDown: () => void;
}

/** The sidebar's header, one line: what the list shows (`ListFilter`: which
 *  projects, narrowed by what text) and the way to add a project. With no
 *  project yet there is nothing to filter, and the line is the "Add project"
 *  button alone — which is also what a first launch needs to see. Once a host
 *  is paired, the environment switcher takes a line of its own above it. */
export function SidebarHeader({
  projects,
  focus,
  onFocusChange,
  query,
  onQueryChange,
  onArrowDown,
}: Props) {
  const compact = useAppStore((s) => s.leftWidth < COMPACT_BELOW);
  const empty = projects.length === 0;

  return (
    <div className="side-head flex-center">
      <EnvironmentSwitcher />
      {!empty && (
        <ListFilter
          projects={projects}
          focus={focus}
          onFocusChange={onFocusChange}
          query={query}
          onQueryChange={onQueryChange}
          onArrowDown={onArrowDown}
          compact={compact}
        />
      )}
      <AddProject full={empty} />
    </div>
  );
}
