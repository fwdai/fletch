import { useState } from "react";
import { api, type ProjectFileEdit, type ProjectFileWhich } from "@/api";
import { useGate } from "@/store/capabilities";
import { useProjectFile } from "@/util/useProjectFile";
import { fileReport } from "./files";

export type ProjectFileEditor = ReturnType<typeof useProjectFileEditor>;

/** A project's settings files for a pane that edits them: what they say, the
 *  file an edit goes to (the user's pick, else shared when the team already
 *  keeps a file, else just you), and one change at a time with its error.
 *  `run` and `edit` resolve whether the change went through. */
export function useProjectFileEditor(projectId: string) {
  const gate = useGate("projectFile");
  const editGate = useGate("projectFileEdit");
  const { status, approve } = useProjectFile(gate ? null : projectId);
  const [which, setTarget] = useState<ProjectFileWhich | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const files = status?.files ?? [];
  const target: ProjectFileWhich =
    which ?? (fileReport(files, "shared")?.exists ? "shared" : "local");

  const run = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await task();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };
  const edit = (file: ProjectFileWhich, change: ProjectFileEdit) =>
    run(() => api.projectFileEdit(projectId, file, change));

  return {
    gate,
    editGate,
    status,
    files,
    approve,
    target,
    setTarget,
    canEdit: !editGate && !!status && !status.error,
    busy,
    error,
    run,
    edit,
  };
}
