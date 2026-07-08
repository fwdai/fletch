import { useEffect, useRef, useState } from "react";
import { Icon } from "@/components/Icon";
import type { SetupRow } from "./types";

interface ConfigRowProps {
  row: SetupRow;
  override: string | undefined;
  onChange: (v: string) => void;
  onRevert: () => void;
}

/** One editable run-config row: shows the detected value as a placeholder,
 *  click-to-edit, revert-to-detected, and an override indicator. Reused by
 *  the Run panel sheet and the Project Settings modal. */
export function ConfigRow({ row, override, onChange, onRevert }: ConfigRowProps) {
  const [editing, setEditing] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing) inputRef.current?.focus();
  }, [editing]);

  const hasOverride = override != null;
  const display = hasOverride ? override : row.value;

  return (
    <div className={`rs-row flex-center${hasOverride ? " overridden" : ""}`}>
      <div className="rs-row-l">
        <div className="rs-label text-base">{row.key}</div>
        <div className="rs-source iflex-center text-xs">
          {hasOverride ? (
            <>
              <span className="dot" />
              Manual override
            </>
          ) : (
            <>
              Detected · <code>{row.source}</code>
            </>
          )}
        </div>
      </div>
      <div className="rs-row-r iflex-center">
        {editing ? (
          <input
            ref={inputRef}
            className="rs-input text-sm"
            type="text"
            defaultValue={display}
            placeholder={row.value}
            onBlur={(e) => {
              const v = e.target.value.trim();
              if (v === "" || v === row.value) onRevert();
              else onChange(v);
              setEditing(false);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") {
                e.currentTarget.value = display;
                e.currentTarget.blur();
              }
            }}
          />
        ) : (
          <button className="rs-value iflex-center text-sm" onClick={() => setEditing(true)}>
            <span className="rsv-text">{display}</span>
            <Icon name="edit" size={10} />
          </button>
        )}
        {hasOverride && !editing && (
          <button
            className="rs-revert iflex-center tip"
            data-tip="Revert to detected"
            onClick={onRevert}
          >
            <span className="rs-revert-ic iflex-center">
              <Icon name="refresh" size={11} />
            </span>
          </button>
        )}
      </div>
    </div>
  );
}
