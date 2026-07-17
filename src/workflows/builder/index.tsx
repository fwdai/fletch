// WorkflowsPane — the Workflows settings pane: define workflows (list ⇄ builder).
// Launching + monitoring live elsewhere (the run monitor); this pane is purely
// the definition surface, persisting through the v1 `wf_def_*` commands (§13).

import { useEffect, useState } from "react";
import { api } from "../../api";
import { installStarterPack, STARTER_WORKFLOW_NAME } from "../../starterPack";
import { useAppStore } from "../../store";
import type { Definition, ImportReport, Spec } from "../spec";
import { ImportDialog } from "./ImportDialog";
import { blankEditor, type EditorState, fromDefinition, toSpec } from "./model";
import { WorkflowBuilder } from "./WorkflowBuilder";
import { WorkflowList } from "./WorkflowList";
import { exportDefinitionYaml, pickYamlText } from "./yamlFile";

type Editing = { state: EditorState; isNew: boolean };

export function WorkflowsPane() {
  const agents = useAppStore((s) => s.customAgents);
  const createCustomAgent = useAppStore((s) => s.createCustomAgent);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const setLastError = useAppStore((s) => s.setLastError);

  const [definitions, setDefinitions] = useState<Definition[]>([]);
  const [loading, setLoading] = useState(true);
  const [installing, setInstalling] = useState(false);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [importReport, setImportReport] = useState<ImportReport | null>(null);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);

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

  // Seed the specialist bundle (four custom agents + the Feature pipeline
  // workflow). Idempotent — re-running skips anything already present by name.
  const installStarter = async () => {
    setInstalling(true);
    try {
      await installStarterPack({
        existingAgents: agents,
        createAgent: createCustomAgent,
        existingDefinitions: definitions,
        saveDefinition: (spec, hue) => api.wfDefSave(spec, undefined, hue),
      });
      await reload();
    } catch (e) {
      setLastError(`Failed to install the starter pack: ${e}`);
    } finally {
      setInstalling(false);
    }
  };

  const hasStarter = definitions.some((d) => d.name === STARTER_WORKFLOW_NAME);

  const exportDef = async (d: Definition) => {
    try {
      await exportDefinitionYaml(d.id, d.spec.name);
    } catch (e) {
      setLastError(`Failed to export workflow: ${e}`);
    }
  };

  // Pick a YAML file, parse+validate it in the backend, and open the mapping
  // dialog. A parse/validation failure is reported; skill/provider issues come
  // back as warnings inside the report, not as an error here.
  const startImport = async () => {
    try {
      const yaml = await pickYamlText();
      if (yaml == null) return;
      setImportReport(await api.wfDefImportYaml(yaml));
      setImportError(null);
    } catch (e) {
      setLastError(`Couldn't import that file: ${e}`);
    }
  };

  const finishImport = async (spec: Spec) => {
    setImporting(true);
    setImportError(null);
    try {
      await api.wfDefSave(spec);
      await reload();
      setImportReport(null);
    } catch (e) {
      setImportError(String(e));
    } finally {
      setImporting(false);
    }
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
    <>
      <WorkflowList
        definitions={definitions}
        loading={loading}
        agents={agents}
        modelsByAgent={modelsByAgent}
        onNew={() => openEditor({ state: blankEditor(definitions.length), isNew: true })}
        onEdit={(d) => openEditor({ state: fromDefinition(d), isNew: false })}
        onDuplicate={duplicate}
        onDelete={remove}
        onExport={exportDef}
        onImport={startImport}
        onInstallStarter={hasStarter ? undefined : installStarter}
        installing={installing}
      />
      {importReport && (
        <ImportDialog
          report={importReport}
          saving={importing}
          error={importError}
          onCancel={() => setImportReport(null)}
          onImport={finishImport}
        />
      )}
    </>
  );
}
