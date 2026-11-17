import { AddProject } from "./AddProject";
import { ListFilter } from "./ListFilter";
import type { ProjectOption } from "./ProjectSelector";
import type { ProjectFocus } from "./projectFocus";

interface Props {
  projects: readonly ProjectOption[];
  focus: ProjectFocus;
  onFocusChange: (focus: string[]) => void;
  query: string;
  onQueryChange: (q: string) => void;
  onArrowDown: () => void;
}

export function SidebarHeader({
  projects,
  focus,
  onFocusChange,
  query,
  onQueryChange,
  onArrowDown,
}: Props) {
  const empty = projects.length === 0;

  return (
    <div className="side-head flex-center">
      {!empty && (
        <ListFilter
          projects={projects}
          focus={focus}
          onFocusChange={onFocusChange}
          query={query}
          onQueryChange={onQueryChange}
          onArrowDown={onArrowDown}
        />
      )}
      <AddProject full={empty} />
    </div>
  );
}
