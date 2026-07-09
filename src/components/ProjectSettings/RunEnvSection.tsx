import { useState } from "react";
import {
  EcosystemBadge,
  persistRunOverrides,
  RunConfigEditor,
  reconcileOverrides,
  rowsOrFallback,
  type SetupRow,
} from "@/components/RunConfig";

interface Props {
  projectId: string;
  rows: SetupRow[];
  ecosystem: string | null;
  initialOverrides: Record<string, string>;
}

/** The run configuration every agent in the project inherits. Unlike the Run
 *  panel sheet — which stages a draft and applies on restart — this edits the
 *  project defaults directly, so changes autosave per row. */
export function RunEnvSection({ projectId, rows: detected, ecosystem, initialOverrides }: Props) {
  // Nothing detected still gets the empty fallback fields — a project with an
  // unrecognized stack should be configurable, not hidden.
  const rows = rowsOrFallback(detected);
  const [overrides, setOverrides] = useState<Record<string, string>>(initialOverrides);

  // Reconcile a single edit against the detected rows and persist it. Same
  // logic the Run panel runs on Apply, just committed immediately.
  const commit = (next: Record<string, string>) => {
    const { cleaned, toSet, toDelete } = reconcileOverrides(rows, overrides, next);
    persistRunOverrides(projectId, toSet, toDelete);
    setOverrides(cleaned);
  };

  const onChange = (id: string, value: string) => commit({ ...overrides, [id]: value });
  const onRevert = (id: string) => {
    const next = { ...overrides };
    delete next[id];
    commit(next);
  };

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Run &amp; environment</h2>
        <p className="ps-section-lead text-sm">
          The run configuration every agent in this project inherits. Values are detected from the
          repo as defaults; edit one to make it the project setting. Individual agents can still
          override these from the Run panel.
        </p>
      </header>

      <div className="ps-eco text-xs">
        <EcosystemBadge ecosystem={ecosystem} />
      </div>
      <RunConfigEditor
        rows={rows}
        draft={overrides}
        scope="project"
        onChange={onChange}
        onRevert={onRevert}
      />
    </section>
  );
}
