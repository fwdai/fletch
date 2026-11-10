import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { ADD_PROJECT_GATES, useAnyGate } from "@/store/capabilities";
import { useShortcutKeys } from "@/util/shortcuts";
import { type ProjectOption, ProjectSelector } from "./ProjectSelector";
import type { ProjectFocus } from "./projectFocus";

interface Props {
  projects: readonly ProjectOption[];
  focus: ProjectFocus;
  onFocusChange: (focus: string[]) => void;
}

/** The row pinned above the project list: which projects it shows, and the
 *  way to add one.
 *
 *  A selector over fewer than two projects has nothing to choose between, so
 *  until there is a second one the row is the full-width "Add project" button
 *  — which is also what a first launch needs to see. From two projects on, it
 *  is the selector with a compact "+" beside it. */
export function ProjectsBar({ projects, focus, onFocusChange }: Props) {
  // Open while the environment can run any of the three add flows — this Mac
  // always can, a host by its descriptor — and closed, with the reason, when
  // it can run none. A disabled button gets no pointer events in the WebView,
  // so the reason's tooltip trigger has to be the wrapper.
  const gate = useAnyGate(ADD_PROJECT_GATES);
  const openAddProject = useAppStore((s) => s.openAddProject);
  const addKeys = useShortcutKeys("addProject");
  const selectable = projects.length > 1;

  return (
    <div className="side-projects flex-center">
      {selectable && <ProjectSelector projects={projects} focus={focus} onChange={onFocusChange} />}
      <span className={`side-add ${gate ? "tip" : ""}`} data-tip={gate}>
        {selectable ? (
          <IconButton
            variant="outline"
            className="side-add-btn"
            tip={gate ? undefined : `Add project  ${addKeys}`}
            aria-label="Add project"
            onClick={openAddProject}
            disabled={gate !== null}
          >
            <Icon name="plus" size={13} />
          </IconButton>
        ) : (
          <button
            type="button"
            className="add-proj-cta flex-center text-sm"
            onClick={openAddProject}
            disabled={gate !== null}
            aria-label="Add project"
          >
            <Icon name="plus" size={13} />
            <span>Add project</span>
          </button>
        )}
      </span>
    </div>
  );
}
