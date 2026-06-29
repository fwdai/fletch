// WorkflowsPane — the Workflows settings pane: define workflows (list ⇄ builder).
// Launching + monitoring are NOT here — they're first-class in the app (the
// new-agent page's Workflow toggle, the sidebar run entity, and the main-pane
// monitor). This pane is purely the definition surface.

import { useEffect, useState } from "react";
import { useAppStore } from "../../store";
import { blankWorkflow } from "../shared";
import {
  deleteWorkflow,
  listWorkflows,
  saveWorkflow,
  toDraft,
  type Workflow,
  type WorkflowDraft,
} from "../storage";
import { WorkflowBuilder } from "./WorkflowBuilder";
import { WorkflowList } from "./WorkflowList";

type Editing = { draft: WorkflowDraft; isNew: boolean };

export function WorkflowsPane() {
  const agents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const setLastError = useAppStore((s) => s.setLastError);

  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<Editing | null>(null);

  const reload = async () => {
    try {
      setWorkflows(await listWorkflows());
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

  const save = async (draft: WorkflowDraft) => {
    try {
      await saveWorkflow(draft);
      setEditing(null);
      await reload();
    } catch (e) {
      setLastError(`Failed to save workflow: ${e}`);
    }
  };
  const remove = async (id: string) => {
    try {
      await deleteWorkflow(id);
      await reload();
    } catch (e) {
      setLastError(`Failed to delete workflow: ${e}`);
    }
  };
  const duplicate = async (w: Workflow) => {
    try {
      await saveWorkflow({ ...toDraft(w), id: `wf-${Date.now()}`, name: `${w.name} copy` });
      await reload();
    } catch (e) {
      setLastError(`Failed to duplicate workflow: ${e}`);
    }
  };

  if (editing) {
    return (
      <WorkflowBuilder
        draft={editing.draft}
        isNew={editing.isNew}
        agents={agents}
        modelsByAgent={modelsByAgent}
        onCancel={() => setEditing(null)}
        onSave={save}
      />
    );
  }

  return (
    <WorkflowList
      workflows={workflows}
      loading={loading}
      agents={agents}
      modelsByAgent={modelsByAgent}
      onNew={() => setEditing({ draft: blankWorkflow(workflows.length), isNew: true })}
      onEdit={(w) => setEditing({ draft: toDraft(w), isNew: false })}
      onDuplicate={duplicate}
      onDelete={remove}
    />
  );
}
