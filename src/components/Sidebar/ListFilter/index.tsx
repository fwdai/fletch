import { Icon } from "@/components/Icon";
import { type ProjectOption, ProjectSelector } from "../ProjectSelector";
import type { ProjectFocus } from "../projectFocus";
import { FilterInput } from "./FilterInput";

interface Props {
  projects: readonly ProjectOption[];
  focus: ProjectFocus;
  onFocusChange: (focus: string[]) => void;
  query: string;
  onQueryChange: (q: string) => void;
  onArrowDown: () => void;
}

export function ListFilter({
  projects,
  focus,
  onFocusChange,
  query,
  onQueryChange,
  onArrowDown,
}: Props) {
  const scoped = projects.length > 1;
  return (
    <div className="side-filter flex-center">
      <Icon name="search" size={12} className="side-filter-icon" />
      <FilterInput query={query} onChange={onQueryChange} onArrowDown={onArrowDown} />
      {scoped && (
        <>
          <span className="side-filter-sep" aria-hidden="true" />
          <ProjectSelector projects={projects} focus={focus} onChange={onFocusChange} />
        </>
      )}
    </div>
  );
}
