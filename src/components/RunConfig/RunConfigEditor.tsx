import { ConfigRow } from "./ConfigRow";
import type { SetupRow } from "./types";

interface Props {
  rows: SetupRow[];
  /** Current override draft, keyed by row id. */
  draft: Record<string, string>;
  /** Which surface is editing — see ConfigRow. */
  scope: "project" | "agent";
  onChange: (id: string, value: string) => void;
  onRevert: (id: string) => void;
}

/** Renders detected run-config rows grouped by section, each editable. Pure
 *  presentation — draft state is owned by the caller. Reused by the Run
 *  panel sheet and Project Settings. */
export function RunConfigEditor({ rows, draft, scope, onChange, onRevert }: Props) {
  const groups = Array.from(new Set(rows.map((r) => r.group)));

  return (
    <>
      {groups.map((g) => (
        <div key={g} className="rs-group">
          <div className="rs-group-h text-xs">{g}</div>
          <div className="rs-group-rows">
            {rows
              .filter((r) => r.group === g)
              .map((r) => (
                <ConfigRow
                  key={r.id}
                  row={r}
                  override={draft[r.id]}
                  scope={scope}
                  onChange={(v) => onChange(r.id, v)}
                  onRevert={() => onRevert(r.id)}
                />
              ))}
          </div>
        </div>
      ))}
    </>
  );
}
