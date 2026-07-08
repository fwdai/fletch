import { useState } from "react";
import {
  ECOSYSTEM_LABEL,
  persistRunOverrides,
  RunConfigEditor,
  reconcileOverrides,
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
export function RunEnvSection({ projectId, rows, ecosystem, initialOverrides }: Props) {
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

  const overrideCount = Object.keys(overrides).length;

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Run &amp; environment</h2>
        <p className="ps-section-lead text-sm">
          Defaults every agent in this project inherits. Detected from the repo; edit a value to
          override it. Individual runs can still override these from the Run panel.
        </p>
      </header>

      {rows.length === 0 ? (
        <div className="ps-state text-sm">
          No run configuration detected for this project — nothing to configure yet.
        </div>
      ) : (
        <>
          <div className="ps-eco text-xs">
            {ecosystem ? (
              <>
                Detected · <code>{ECOSYSTEM_LABEL[ecosystem] ?? ecosystem}</code>
              </>
            ) : (
              <>No ecosystem detected — edit values below</>
            )}
            {overrideCount > 0 && (
              <span className="ps-eco-count">
                {" · "}
                <b>{overrideCount}</b> override{overrideCount === 1 ? "" : "s"}
              </span>
            )}
          </div>
          <RunConfigEditor rows={rows} draft={overrides} onChange={onChange} onRevert={onRevert} />
        </>
      )}
    </section>
  );
}
