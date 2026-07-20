import { useState } from "react";
import { CustomizeSwitch } from "@/components/SettingsScreen/CustomizeSwitch";
import { LibraryList } from "@/components/SettingsScreen/LibraryList";
import type { NewSkill, Skill } from "@/storage/skills";
import { useAppStore } from "@/store";
import { SkillEditor } from "./SkillEditor";

// Skills settings pane: a list ⇄ editor switch over the shared skills library.
// Skills are named instruction documents custom agents attach; at spawn they're
// snapshotted onto the session and materialized as files the agent reads on
// demand. All mutations go through the store slice, which writes to the db and
// keeps the list in sync.

function blankSkill(): NewSkill {
  return { name: "", description: "", body: "" };
}

type EditTarget =
  | { mode: "new"; initial: NewSkill }
  | { mode: "edit"; id: string; initial: NewSkill };

export function SkillsPane() {
  const skills = useAppStore((s) => s.skills);
  const customAgents = useAppStore((s) => s.customAgents);
  const createSkill = useAppStore((s) => s.createSkill);
  const updateSkill = useAppStore((s) => s.updateSkill);
  const deleteSkill = useAppStore((s) => s.deleteSkill);
  const setLastError = useAppStore((s) => s.setLastError);

  const [editing, setEditing] = useState<EditTarget | null>(null);

  const startNew = () => setEditing({ mode: "new", initial: blankSkill() });
  const startEdit = (s: Skill) => setEditing({ mode: "edit", id: s.id, initial: s });

  /** How many agents carry a skill — shown in the list so it's obvious a
   *  library edit reaches every one of them (future sessions only). */
  const usedBy = (id: string) => customAgents.filter((a) => a.skillIds.includes(id)).length;

  const save = async (values: NewSkill) => {
    try {
      if (editing?.mode === "edit") {
        await updateSkill(editing.id, values);
      } else {
        await createSkill(values);
      }
      setEditing(null);
    } catch (e) {
      setLastError(`Failed to save skill: ${e}`);
    }
  };

  const remove = async (s: Skill) => {
    try {
      await deleteSkill(s.id);
    } catch (e) {
      setLastError(`Failed to delete skill: ${e}`);
    }
  };

  if (editing) {
    return (
      <SkillEditor
        initial={editing.initial}
        isNew={editing.mode === "new"}
        onCancel={() => setEditing(null)}
        onSave={save}
      />
    );
  }

  return (
    <LibraryList
      eyebrow="Settings · Customize"
      eyebrowAside={<CustomizeSwitch />}
      title="Skills"
      desc="Named instruction documents your custom agents load on demand. Assign skills to agents in their editor; running sessions keep the version they spawned with."
      newLabel="New skill"
      emptyLabel="Create your first skill"
      icon="notebookPen"
      items={skills}
      row={(s) => {
        const count = usedBy(s.id);
        return {
          name: s.name,
          badge: count === 1 ? "1 agent" : `${count} agents`,
          desc: s.description || s.body || "Empty skill.",
        };
      }}
      onNew={startNew}
      onEdit={startEdit}
      onDelete={remove}
    />
  );
}
