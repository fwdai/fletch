import { useEffect, useRef, useState } from "react";
import { Icon } from "@/components/Icon";
import type { SetupRow } from "./types";

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

/** Widest the value can render in view mode before it truncates. */
const VIEW_MAX_WIDTH = 220;
/** Input width while editing. */
const EDIT_WIDTH = 200;

/** One editable run-config row. The value is a single persistent input styled
 *  as a chip in view mode, so the border, focus ring, and width all animate
 *  on the view↔edit transition instead of the two states swapping elements.
 *  Overridden rows on the agent surface get a revert button; project edits
 *  are final (clearing the field falls back to detected). Reused by the Run
 *  panel sheet and the Project Settings modal. */
export function ConfigRow({ row, override, scope, onChange, onRevert }: ConfigRowProps) {
  const [editing, setEditing] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  // Set when Escape cancels the edit, so the blur that follows discards the
  // draft instead of committing it.
  const cancelledRef = useRef(false);

  const hasOverride = override != null;
  const display = hasOverride ? override : row.value;

  const [text, setText] = useState(display);
  useEffect(() => {
    if (!editing) setText(display);
  }, [display, editing]);

  // Inputs don't size to their content, but the value is monospace, so its
  // rendered width is exactly length × 1ch — sized in ch units the width is
  // always right, with no DOM measurement to go stale when the font loads.
  // Keeping the width explicit in both states is what lets it transition.
  const viewWidth = `min(${Math.max(display.length, 1)}ch + 2px, ${VIEW_MAX_WIDTH}px)`;

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
  ) : (
    <>
      Detected · <code>{row.source}</code>
    </>
  );

  return (
    <div className={`rs-row flex-center${marksOverride ? " overridden" : ""}`}>
      <div className="rs-row-l">
        <div className="rs-label text-base">{row.key}</div>
        {caption && <div className="rs-source iflex-center text-xs">{caption}</div>}
      </div>
      <div className="rs-row-r iflex-center">
        <div
          className={`rs-value iflex-center text-sm${editing ? " editing" : ""}`}
          onClick={() => inputRef.current?.focus()}
        >
          <input
            ref={inputRef}
            aria-label={row.key}
            type="text"
            value={text}
            readOnly={!editing}
            placeholder={row.value}
            style={{ width: editing ? EDIT_WIDTH : viewWidth }}
            onFocus={() => setEditing(true)}
            onChange={(e) => setText(e.target.value)}
            onBlur={(e) => {
              setEditing(false);
              if (cancelledRef.current) {
                cancelledRef.current = false;
                setText(display);
                return;
              }
              const v = e.currentTarget.value.trim();
              // Unchanged from what's shown — don't fire onChange/onRevert.
              // Comparing against `display` (the effective value: override if
              // set, else detected) rather than `row.value` (detected only) is
              // what keeps a no-op focus→blur on an already-overridden field
              // from re-committing the identical override.
              if (v === display) return;
              if (v === "" || v === row.value) onRevert();
              else onChange(v);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") {
                // Keep Escape from bubbling to the container's document/window
                // keydown listener (Project Settings modal, Run panel sheet),
                // which would otherwise close the whole surface instead of just
                // cancelling this in-progress edit.
                e.stopPropagation();
                cancelledRef.current = true;
                e.currentTarget.blur();
              }
            }}
          />
          <span className="rs-edit-ic iflex-center">
            <Icon name="edit" size={10} />
          </span>
        </div>
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
