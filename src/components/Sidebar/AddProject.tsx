import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { ADD_PROJECT_GATES, useAnyGate } from "@/store/capabilities";
import { useShortcutKeys } from "@/util/shortcuts";

/** The way to add a project, in the sidebar header: a compact "+" beside the
 *  list filter, or — `full`, while there is no project at all, which is what a
 *  first launch sees — the whole row as an "Add project" button. */
export function AddProject({ full }: { full: boolean }) {
  // Open while the environment can run any of the three add flows — this Mac
  // always can, a host by its descriptor — and closed, with the reason, when
  // it can run none. A disabled button gets no pointer events in the WebView,
  // so the reason's tooltip trigger has to be the wrapper.
  const gate = useAnyGate(ADD_PROJECT_GATES);
  const openAddProject = useAppStore((s) => s.openAddProject);
  const addKeys = useShortcutKeys("addProject");

  return (
    <span className={`side-add ${full ? "full" : ""} ${gate ? "tip" : ""}`} data-tip={gate}>
      {full ? (
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
      ) : (
        <IconButton
          className="side-add-btn"
          tip={gate ? undefined : `Add project  ${addKeys}`}
          aria-label="Add project"
          onClick={openAddProject}
          disabled={gate !== null}
        >
          <Icon name="plus" size={14} />
        </IconButton>
      )}
    </span>
  );
}
