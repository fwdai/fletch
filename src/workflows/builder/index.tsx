// WorkflowsPane — the Workflows settings pane: define workflows (list ⇄ builder).
// Launching + monitoring live elsewhere (the run monitor); this pane is purely
// the definition surface, persisting through the v1 `wf_def_*` commands (§13).

import { useEffect, useState } from "react";
import { api } from "../../api";
import { useAppStore } from "../../store";
import type { Definition } from "../spec";
import { blankEditor, type EditorState, fromDefinition, toSpec } from "./model";
import { WorkflowBuilder } from "./WorkflowBuilder";
import { WorkflowList } from "./WorkflowList";

type Editing = { state: EditorState; isNew: boolean };

export function WorkflowsPane() {
  const agents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const setLastError = useAppStore((s) => s.setLastError);

  const [definitions, setDefinitions] = useState<Definition[]>([]);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const reload = async () => {
    try {
      setDefinitions(await api.wfDefList());
    } catch (e) {
      setLastError(`Failed to load workflows: ${e}`);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const save = async (state: EditorState) => {
    setSaving(true);
    setSaveError(null);
    try {
      await api.wfDefSave(toSpec(state), state.id ?? undefined, state.hue);
      setEditing(null);
      await reload();
    } catch (e) {
      // Surface backend validation (the joined §5.2 messages) inline in the builder.
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (id: string) => {
    try {
      await api.wfDefDelete(id);
      await reload();
    } catch (e) {
      setLastError(`Failed to delete workflow: ${e}`);
    }
  };

  const duplicate = async (d: Definition) => {
    try {
      await api.wfDefSave(
        { ...d.spec, name: `${d.spec.name} copy` },
        undefined,
        d.hue ?? undefined,
      );
      await reload();
    } catch (e) {
      setLastError(`Failed to duplicate workflow: ${e}`);
    }
  };

  const openEditor = (next: Editing) => {
    setSaveError(null);
    setEditing(next);
  };

  if (editing) {
    return (
      <WorkflowBuilder
        initial={editing.state}
        isNew={editing.isNew}
        agents={agents}
        modelsByAgent={modelsByAgent}
        saving={saving}
        saveError={saveError}
        onCancel={() => setEditing(null)}
        onSave={save}
      />
    );
  }

  return (
    <WorkflowList
      definitions={definitions}
      loading={loading}
      agents={agents}
      modelsByAgent={modelsByAgent}
      onNew={() => openEditor({ state: blankEditor(definitions.length), isNew: true })}
      onEdit={(d) => openEditor({ state: fromDefinition(d), isNew: false })}
      onDuplicate={duplicate}
      onDelete={remove}
    />
  );
}
