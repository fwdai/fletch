import { Modal } from "@/components/ui/Modal";
import { CloneView } from "./CloneView";
import { CreateView } from "./CreateView";
import { useNewProject } from "./useNewProject";

export type NewProjectMode = "clone" | "create";

/** Centered modal for adding a project by cloning from GitHub or creating a
 *  fresh repo. Launched from the sidebar's "+" popover. */
export function NewProject({ mode, onClose }: { mode: NewProjectMode; onClose: () => void }) {
  const shared = useNewProject();

  return (
    <Modal
      icon={mode === "clone" ? "github" : "sparkle"}
      title={mode === "clone" ? "Clone from GitHub" : "Create new project"}
      onClose={onClose}
    >
      {mode === "clone" ? (
        <CloneView shared={shared} onDone={onClose} />
      ) : (
        <CreateView shared={shared} onDone={onClose} />
      )}
    </Modal>
  );
}
