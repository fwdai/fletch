import { useState } from "react";
import { DEFAULT_PROVIDER_ID } from "../../../data/providers";
import { useAppStore } from "../../../store";
import type { CustomAgent, NewCustomAgent } from "../../../storage/customAgents";
import { AgentEditor } from "./AgentEditor";
import { AgentList } from "./AgentList";
import { CA_HUES } from "./shared";

// Custom Agents settings pane: a list ⇄ editor switch. The list shows every
// saved preset; the editor creates or edits one. All mutations go through the
// store slice, which writes to the db and keeps the list in sync.

/** A blank agent for the "New agent" flow. A fresh hue each time keeps new
 *  agents visually distinct (index by current count, wrapping). */
function blankAgent(seed: number): NewCustomAgent {
  return {
    name: "",
    description: "",
    color: CA_HUES[seed % CA_HUES.length],
    base: DEFAULT_PROVIDER_ID,
    model: null,
    effort: null,
    instructions: "",
  };
}

type EditTarget =
  | { mode: "new"; initial: NewCustomAgent }
  | { mode: "edit"; id: string; initial: NewCustomAgent };

export function CustomAgentsPane() {
  const agents = useAppStore((s) => s.customAgents);
  const createCustomAgent = useAppStore((s) => s.createCustomAgent);
  const updateCustomAgent = useAppStore((s) => s.updateCustomAgent);
  const deleteCustomAgent = useAppStore((s) => s.deleteCustomAgent);
  const duplicateCustomAgent = useAppStore((s) => s.duplicateCustomAgent);
  const setLastError = useAppStore((s) => s.setLastError);

  const [editing, setEditing] = useState<EditTarget | null>(null);

  const startNew = () => setEditing({ mode: "new", initial: blankAgent(agents.length) });
  const startEdit = (a: CustomAgent) =>
    setEditing({ mode: "edit", id: a.id, initial: a });

  const save = async (values: NewCustomAgent) => {
    try {
      if (editing?.mode === "edit") {
        await updateCustomAgent(editing.id, values);
      } else {
        await createCustomAgent(values);
      }
      setEditing(null);
    } catch (e) {
      setLastError(`Failed to save custom agent: ${e}`);
    }
  };

  const remove = async (a: CustomAgent) => {
    try {
      await deleteCustomAgent(a.id);
    } catch (e) {
      setLastError(`Failed to delete custom agent: ${e}`);
    }
  };

  const duplicate = async (a: CustomAgent) => {
    try {
      await duplicateCustomAgent(a.id);
    } catch (e) {
      setLastError(`Failed to duplicate custom agent: ${e}`);
    }
  };

  if (editing) {
    return (
      <AgentEditor
        initial={editing.initial}
        isNew={editing.mode === "new"}
        onCancel={() => setEditing(null)}
        onSave={save}
      />
    );
  }

  return (
    <AgentList
      agents={agents}
      onNew={startNew}
      onEdit={startEdit}
      onDuplicate={duplicate}
      onDelete={remove}
    />
  );
}
