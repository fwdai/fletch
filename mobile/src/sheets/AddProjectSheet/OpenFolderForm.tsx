import { useStore } from "../../store";
import { FolderPicker } from "./FolderPicker";
import type { RunAddProject } from "./useAddProject";

/** Pin a folder that is already on the Mac. */
export function OpenFolderForm({
  busy,
  error,
  run,
}: {
  busy: boolean;
  error: string | null;
  run: RunAddProject;
}) {
  const addWorkspaceRepo = useStore((s) => s.addWorkspaceRepo);
  return (
    <FolderPicker
      actionLabel="Use this folder"
      hint="Not a git repo yet? Fletch will run git init here."
      busy={busy}
      error={error}
      onUse={(path) => void run(() => addWorkspaceRepo(path))}
    />
  );
}
