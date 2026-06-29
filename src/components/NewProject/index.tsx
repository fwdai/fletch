import { Icon } from "../Icon";
import { Scrim } from "../ui/Scrim";
import { CloneView } from "./CloneView";
import { CreateView } from "./CreateView";
import { useNewProject } from "./useNewProject";

export type NewProjectMode = "clone" | "create";

/** Centered modal for adding a project by cloning from GitHub or creating a
 *  fresh repo. Launched from the sidebar's "+" popover. */
export function NewProject({ mode, onClose }: { mode: NewProjectMode; onClose: () => void }) {
  const shared = useNewProject();

  return (
    <>
      <Scrim onClose={onClose} zIndex={300} />
      <div className="np-modal" role="dialog" aria-modal="true">
        <div className="np-modal-h flex-center">
          <Icon name={mode === "clone" ? "github" : "sparkle"} size={15} />
          <span>{mode === "clone" ? "Clone from GitHub" : "Create new project"}</span>
          <button className="np-close flex-center" aria-label="Close" onClick={onClose}>
            <Icon name="close" size={14} />
          </button>
        </div>
        {mode === "clone" ? (
          <CloneView shared={shared} onDone={onClose} />
        ) : (
          <CreateView shared={shared} onDone={onClose} />
        )}
      </div>
    </>
  );
}
