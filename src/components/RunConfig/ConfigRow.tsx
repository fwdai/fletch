import { useState } from "react";
import { Icon } from "@/components/Icon";
import type { SetupRow } from "./types";
import { ValueChip } from "./ValueChip";

interface ConfigRowProps {
  row: SetupRow;
  override: string | undefined;
  /** Which surface the row is rendered on. Project Settings edits ARE the
   *  project setting — an edited row carries no special label or styling.
   *  The Run panel layers per-agent values on top of the project's, so an
   *  edited row there is marked as overriding the project setting. */
  scope: "project" | "agent";
  onChange: (v: string) => void;
  onRevert: () => void;
}

/** One editable run-config row. The value is a shared [`ValueChip`] — a chip
 *  whose in-field edit icon and width animate on the view↔edit transition.
 *  Overridden rows on the agent surface get a revert button; project edits
 *  are final (clearing the field falls back to detected). Reused by the Run
 *  panel sheet and the Project Settings modal. */
export function ConfigRow({ row, override, scope, onChange, onRevert }: ConfigRowProps) {
  const [editing, setEditing] = useState(false);
  const hasOverride = override != null;
  const display = hasOverride ? override : row.value;
  // The detected default seeds the placeholder; a fallback row has none, so
  // it carries its own hint instead.
  const placeholder = row.value || row.placeholder || "";

  // A committed value that clears the field or matches the detected default
  // reverts; anything else is an override. (`ValueChip` already suppresses a
  // no-op commit that equals `display`.)
  const onCommit = (v: string) => {
    if (v === "" || v === row.value) onRevert();
    else onChange(v);
  };

  // Project Settings edits ARE the project setting — no override framing
  // there, in label or styling. Only the agent surface marks a row that
  // deviates from what the project says.
  const fromProject = row.origin === "project";
  const marksOverride = hasOverride && scope === "agent";
  const overrideLabel = fromProject ? "Overrides project setting" : "Overrides project default";
  const revertTip = fromProject ? "Revert to project setting" : "Revert to detected";

  const caption = marksOverride ? (
    <>
      <span className="dot" />
      {overrideLabel}
    </>
  ) : hasOverride ? null : fromProject ? (
    <>Project setting</>
  ) : row.source ? (
    <>
      Detected · <code>{row.source}</code>
    </>
  ) : (
    <>Not set</>
  );

  return (
    <div className={`rs-row flex-center${marksOverride ? " overridden" : ""}`}>
      <div className="rs-row-l">
        <div className="rs-label text-base">{row.key}</div>
        {caption && <div className="rs-source iflex-center text-xs">{caption}</div>}
      </div>
      <div className="rs-row-r iflex-center">
        <ValueChip
          value={display}
          placeholder={placeholder}
          ariaLabel={row.key}
          onCommit={onCommit}
          onEditingChange={setEditing}
        />
        {marksOverride && !editing && (
          <button className="rs-revert iflex-center tip" data-tip={revertTip} onClick={onRevert}>
            <span className="rs-revert-ic iflex-center">
              <Icon name="refresh" size={11} />
            </span>
          </button>
        )}
      </div>
    </div>
  );
}
