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

/** What the sidebar lists, as one field: the text that narrows the
 *  workspaces, then which projects it looks in. Both answer the same question,
 *  so they share one box rather than a row each. The text leads, so the field
 *  reads as a search box at a glance and typing always starts at the same
 *  place; the scope trails it, beside the header's other project control, the
 *  "+".
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
      <Icon name="search" size={12} className="side-filter-icon" />
      <FilterInput
        query={query}
        onChange={onQueryChange}
        onArrowDown={onArrowDown}
        compact={compact}
      />
      {scoped && (
        <>
          <span className="side-filter-sep" aria-hidden="true" />
          <ProjectSelector
            projects={projects}
            focus={focus}
            onChange={onFocusChange}
            compact={compact}
          />
        </>
      )}
    </div>
  );
}
