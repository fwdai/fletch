import { useEffect, useState } from "react";
import { SetGroup, SetHead, SetRow } from "@/components/SettingsScreen/primitives";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { DeleteProjectRow } from "./DeleteProjectRow";
import { LinearTeamRow } from "./LinearTeamRow";
import { RepositoriesField } from "./RepositoriesField";

/** Project identity: a custom display name (independent of any folder name),
 *  the repositories the project is made of, where its tickets come from, and
 *  deleting it. The project has no location of its own — each repo row
 *  carries its own path. */
export function GeneralPane({ projectId, name }: { projectId: string; name: string }) {
  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Project settings · General"
        title="General"
        desc="The project's name, its repositories and its issue tracker."
      />
      <SetGroup label="Identity">
        <NameRow projectId={projectId} name={name} />
      </SetGroup>
      <RepositoriesField projectId={projectId} />
      <SetGroup label="Issue tracker">
        <LinearTeamRow projectId={projectId} />
      </SetGroup>
      <SetGroup label="Danger zone" last>
        <DeleteProjectRow projectId={projectId} projectName={name} />
      </SetGroup>
    </div>
  );
}

/** Free-text name, committed on blur / Enter; Escape puts the saved one back. */
function NameRow({ projectId, name }: { projectId: string; name: string }) {
  const renameProject = useAppStore((s) => s.renameProject);
  // Null on this Mac and on any host that answers the project-settings ops.
  const gate = useGate("projectAdmin");
  const [draft, setDraft] = useState(name);
  const [error, setError] = useState<string | null>(null);

  // Follow the store (a rename elsewhere, a trimmed save) so the field never drifts.
  useEffect(() => {
    setDraft(name);
  }, [name]);

  const commit = async () => {
    const trimmed = draft.trim();
    if (!trimmed || trimmed === name) {
      setDraft(name);
      return;
    }
    try {
      await renameProject(projectId, trimmed);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <SetRow
      title="Name"
      sub={error ?? gate ?? "Shown in the sidebar. Independent of the folder name."}
    >
      <TextInput
        value={draft}
        disabled={!!gate}
        spellCheck={false}
        autoComplete="off"
        invalid={!!error}
        aria-label="Project name"
        onChange={(e) => {
          setDraft(e.target.value);
          if (error) setError(null);
        }}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          else if (e.key === "Escape") setDraft(name);
        }}
      />
    </SetRow>
  );
}
