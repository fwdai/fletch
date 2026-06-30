import { useEffect, useRef, useState } from "react";
import { Icon } from "../Icon";

export interface SetupRow {
  id: string;
  group: string;
  key: string;
  value: string; // inferred / default
  source: string; // e.g. "scripts.dev", "vite.config.ts"
}

interface Props {
  rows: SetupRow[];
  overrides: Record<string, string>;
  /** Detected ecosystem ("node", "rust", …), or null if none recognized. */
  ecosystem: string | null;
  onClose: () => void;
  onApply: (overrides: Record<string, string>) => void;
}

const ECOSYSTEM_LABEL: Record<string, string> = {
  node: "Node",
  python: "Python",
  ruby: "Ruby",
  rust: "Rust",
  go: "Go",
};

export function RunSettingsSheet({ rows, overrides, ecosystem, onClose, onApply }: Props) {
  // Draft = working copy while the sheet is open; uncommitted until Apply.
  const [draft, setDraft] = useState<Record<string, string>>(overrides);

  const groups = Array.from(new Set(rows.map((r) => r.group)));

  const setRow = (id: string, v: string | null) =>
    setDraft((d) => {
      const next = { ...d };
      if (v == null || v === "") delete next[id];
      else next[id] = v;
      return next;
    });

  const revert = (id: string) => setRow(id, null);

  // Rows whose draft differs from the inferred value
  const changed = rows.filter((r) => draft[r.id] != null && draft[r.id] !== r.value);
  const dirty =
    changed.length > 0 ||
    Object.keys(draft).length !== Object.keys(overrides).length ||
    Object.keys(draft).some((k) => draft[k] !== overrides[k]);

  // Close on Escape
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);

  return (
    <>
      {/* Fixed transparent layer — captures clicks outside the run panel */}
      <div style={{ position: "fixed", inset: 0, zIndex: 4 }} onClick={onClose} />
      {/* Visual scrim — blurs the logs area behind the sheet */}
      <div className="run-sheet-scrim" onClick={onClose} />
      <div className="run-sheet" role="dialog" aria-label="Run configuration">
        {/* Header */}
        <div className="run-sheet-h">
          <div className="rsh-left">
            <div className="rsh-title text-base">Run configuration</div>
            <div className="rsh-sub text-sm">
              {ecosystem ? (
                <>
                  Detected · <code>{ECOSYSTEM_LABEL[ecosystem] ?? ecosystem}</code>
                </>
              ) : (
                <>No ecosystem detected — edit values below</>
              )}
            </div>
          </div>
          <button className="run-sheet-x iflex-center" onClick={onClose} aria-label="Close">
            <Icon name="close" size={12} />
          </button>
        </div>

        {/* Body */}
        <div className="run-sheet-body">
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
                      onChange={(v) => setRow(r.id, v)}
                      onRevert={() => revert(r.id)}
                    />
                  ))}
              </div>
            </div>
          ))}
        </div>

        {/* Footer */}
        <div className="run-sheet-f flex-center">
          <div className="rsf-status truncate text-sm">
            {changed.length === 0 ? (
              <span style={{ color: "var(--fg-3)" }}>No overrides — using detected values</span>
            ) : (
              <span>
                <b style={{ color: "var(--fg-0)" }}>{changed.length}</b>
                {" override"}
                {changed.length === 1 ? "" : "s"}
                {" pending"}
              </span>
            )}
          </div>
          <div className="rsf-actions">
            <button
              className="btn-t iflex-center ghost"
              onClick={() => setDraft({})}
              disabled={Object.keys(draft).length === 0}
            >
              Reset all
            </button>

            <button
              className="btn-t iflex-center primary"
              onClick={() => onApply(draft)}
              disabled={!dirty}
            >
              <Icon name="refresh" size={11} />
              Apply &amp; restart
            </button>
          </div>
        </div>
      </div>
    </>
  );
}

// ── Config row ───────────────────────────────────────────────────────────────

interface ConfigRowProps {
  row: SetupRow;
  override: string | undefined;
  onChange: (v: string) => void;
  onRevert: () => void;
}

function ConfigRow({ row, override, onChange, onRevert }: ConfigRowProps) {
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
