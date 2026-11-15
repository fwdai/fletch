import { Icon } from "@/components/Icon";
import { type ProjectOption, ProjectSelector } from "../ProjectSelector";
import type { ProjectFocus } from "../projectFocus";
import { FilterInput } from "./FilterInput";

interface Props {
  /** Every project, in sidebar order. */
  projects: readonly ProjectOption[];
  /** The projects the list is narrowed to; empty for all of them. */
  focus: ProjectFocus;
  onFocusChange: (focus: string[]) => void;
  query: string;
  onQueryChange: (q: string) => void;
  /** ↓ pressed in the input: hand focus to the list below. */
  onArrowDown: () => void;
  /** The sidebar is narrow: the short scope label, and no keycap. */
  compact: boolean;
}

/** What the sidebar lists, as one field read left to right: which projects,
 *  then the text that narrows their workspaces — "in these projects, matching
 *  this". Both answer the same question, so they share one box rather than a
 *  row each.
 *
 *  A scope over fewer than two projects has nothing to choose between, so
 *  until there is a second one the field is the text filter alone. */
export function ListFilter({
  projects,
  focus,
  onFocusChange,
  query,
  onQueryChange,
  onArrowDown,
  compact,
}: Props) {
  const scoped = projects.length > 1;
  return (
    <div className="side-filter flex-center">
      {scoped ? (
        <>
          <ProjectSelector
            projects={projects}
            focus={focus}
            onChange={onFocusChange}
            compact={compact}
          />
          <span className="side-filter-sep" aria-hidden="true" />
        </>
      ) : (
        <Icon name="search" size={12} className="side-filter-icon" />
      )}
      <FilterInput
        query={query}
        onChange={onQueryChange}
        onArrowDown={onArrowDown}
        compact={compact}
      />
    </div>
  );
}
