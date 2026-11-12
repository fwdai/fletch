import { SetRow, SetToggle } from "@/components/SettingsScreen/primitives";
import { FILE_LABEL } from "../projectFile/files";
import type { ProjectFileEditor } from "../projectFile/useProjectFileEditor";
import { enabledEdit, enabledSource } from "./enabled";

/** Whether new workspaces start from a starting disk, and which file says so. */
export function EnabledRow({ editor, on }: { editor: ProjectFileEditor; on: boolean }) {
  const { files, target, canEdit, editGate, busy, edit } = editor;
  const source = enabledSource(files);
  const flip = () => {
    const { file, change } = enabledEdit(files, target, !on);
    void edit(file, change);
  };

  return (
    <SetRow
      title="Use a starting disk"
      sub={
        source
          ? `${on ? "On" : "Off"} in ${FILE_LABEL[source]}.`
          : "On unless a settings file turns it off."
      }
    >
      <SetToggle on={on} onClick={flip} disabled={!canEdit || busy} tip={editGate ?? undefined} />
    </SetRow>
  );
}
