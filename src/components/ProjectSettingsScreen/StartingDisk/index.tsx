import { SetGroup } from "@/components/SettingsScreen/primitives";
import { FileStates } from "../projectFile/FileStates";
import { useProjectFileEditor } from "../projectFile/useProjectFileEditor";
import { EnabledRow } from "./EnabledRow";
import { ExcludedPaths } from "./ExcludedPaths";
import { ExcludeRow } from "./ExcludeRow";

/** What a project's new VM workspaces start with: its installed dependencies,
 *  from the project's `seed` settings in `.fletch/settings.json` (shared) or
 *  `.fletch/settings.local.json` (just you), edited here in the file the user
 *  picks. */
export function StartingDisk({ projectId }: { projectId: string }) {
  const editor = useProjectFileEditor(projectId);
  const seed = editor.status?.seed;
  // Nothing until the host answers; a host from before starting disks answers
  // without `seed`.
  if (!seed && !editor.gate) return null;

  return (
    <SetGroup
      label="Starting disk"
      sub="New workspaces start with the project's installed dependencies instead of installing them again."
    >
      {seed && (
        <>
          <EnabledRow editor={editor} on={seed.enabled} />
          <ExcludedPaths editor={editor} />
          {editor.canEdit && <ExcludeRow editor={editor} />}
        </>
      )}
      <FileStates editor={editor} warnings={(f) => f.seed.warnings} quiet />
    </SetGroup>
  );
}
