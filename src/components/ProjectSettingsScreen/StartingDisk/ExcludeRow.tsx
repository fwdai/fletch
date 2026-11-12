import { useState } from "react";
import { SetRow } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import { TextInput } from "@/components/ui/TextInput";
import { FileTarget } from "../projectFile/FileTarget";
import type { ProjectFileEditor } from "../projectFile/useProjectFileEditor";

/** A row to leave one more folder out of the starting disk, written into the
 *  file picked beside it. The draft is kept when the host refuses it. */
export function ExcludeRow({ editor }: { editor: ProjectFileEditor }) {
  const { target, setTarget, busy, edit } = editor;
  const [draft, setDraft] = useState("");
  const add = async () => {
    const path = draft.trim();
    if (path && (await edit(target, { seedExcludeAdd: [path] }))) setDraft("");
  };

  return (
    <SetRow
      title="Leave out a folder"
      sub="Folders that are never carried over, for private data git ignores."
    >
      <TextInput
        mono
        value={draft}
        placeholder="uploads"
        spellCheck={false}
        aria-label="Folder to leave out"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void add();
        }}
      />
      <FileTarget value={target} onChange={setTarget} />
      <Button variant="outline" onClick={() => void add()} disabled={!draft.trim() || busy}>
        Add
      </Button>
    </SetRow>
  );
}
