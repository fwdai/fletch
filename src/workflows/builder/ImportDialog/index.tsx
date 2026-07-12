// ImportDialog — the YAML import mapping step (spec §14.3). Given the backend's
// `ImportReport`, it lets the user resolve each agent alias (map onto a local
// custom agent vs. use the file's embedded spec), surfaces skill/provider
// warnings, and on confirm hands the resolved `Spec` back to be saved.

import { useState } from "react";
import { Icon } from "../../../components/Icon";
import { Scrim } from "../../../components/ui/Scrim";
import type { AgentResolution, ImportReport, Spec } from "../../spec";
import { type AgentChoice, applyResolutions, initialChoices } from "./resolve";

function AgentRow({
  r,
  choice,
  onChoice,
}: {
  r: AgentResolution;
  choice: AgentChoice;
  onChoice: (c: AgentChoice) => void;
}) {
  return (
    <div className="wf-imp-agent">
      <div className="wf-imp-agent-h">
        <span className="wf-imp-alias">{r.alias}</span>
        <span className="wf-imp-base">
          {r.base}
          {r.embedded.model ? ` · ${r.embedded.model}` : ""}
        </span>
      </div>
      <div className="wf-imp-choices">
        <label
          className={`wf-imp-choice${choice === "map" ? " sel" : ""}${r.local_match ? "" : " off"}`}
        >
          <input
            type="radio"
            name={`imp-${r.alias}`}
            checked={choice === "map"}
            disabled={!r.local_match}
            onChange={() => onChoice("map")}
          />
          <span>
            Map to my agent
            {r.local_match ? <b> {r.local_match.name}</b> : <em> (no local match)</em>}
          </span>
        </label>
        <label className={`wf-imp-choice${choice === "embed" ? " sel" : ""}`}>
          <input
            type="radio"
            name={`imp-${r.alias}`}
            checked={choice === "embed"}
            onChange={() => onChoice("embed")}
          />
          <span>Use embedded spec</span>
        </label>
      </div>
    </div>
  );
}

export function ImportDialog({
  report,
  saving,
  error,
  onCancel,
  onImport,
}: {
  report: ImportReport;
  saving: boolean;
  error: string | null;
  onCancel: () => void;
  onImport: (spec: Spec) => void;
}) {
  const [choices, setChoices] = useState<Record<string, AgentChoice>>(() => initialChoices(report));

  const setChoice = (alias: string, c: AgentChoice) =>
    setChoices((prev) => ({ ...prev, [alias]: c }));

  return (
    <>
      <Scrim onClose={onCancel} zIndex={300} />
      <div className="wf-imp-modal" role="dialog" aria-modal="true">
        <div className="wf-imp-h">
          <Icon name="upload" size={15} />
          <span>Import “{report.spec.name}”</span>
          <button className="wf-imp-close flex-center" aria-label="Close" onClick={onCancel}>
            <Icon name="close" size={14} />
          </button>
        </div>

        <div className="wf-imp-body">
          <p className="wf-imp-lead">
            Resolve each agent this workflow uses. Mapping to one of your agents reuses its local
            configuration; otherwise the specification embedded in the file is used as-is for this
            workflow.
          </p>

          <div className="wf-imp-agents">
            {report.agents.map((r) => (
              <AgentRow
                key={r.alias}
                r={r}
                choice={choices[r.alias] ?? "embed"}
                onChoice={(c) => setChoice(r.alias, c)}
              />
            ))}
          </div>

          {report.warnings.length > 0 && (
            <div className="wf-imp-warns">
              <div className="wf-imp-warns-h">
                <Icon name="hand" size={13} /> {report.warnings.length} warning
                {report.warnings.length === 1 ? "" : "s"}
              </div>
              <ul>
                {report.warnings.map((w, i) => (
                  <li key={i}>{w}</li>
                ))}
              </ul>
            </div>
          )}

          {error && <div className="wf-imp-err">{error}</div>}
        </div>

        <div className="wf-imp-foot">
          <button className="btn-t" onClick={onCancel} disabled={saving}>
            Cancel
          </button>
          <button
            className="btn-t primary"
            onClick={() => onImport(applyResolutions(report, choices))}
            disabled={saving}
          >
            {saving ? "Importing…" : "Import workflow"}
          </button>
        </div>
      </div>
    </>
  );
}
