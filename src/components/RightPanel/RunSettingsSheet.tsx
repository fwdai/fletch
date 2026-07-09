import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { EcosystemBadge, RunConfigEditor, type SetupRow } from "@/components/RunConfig";
import { Button } from "@/components/ui/Button";

interface Props {
  rows: SetupRow[];
  overrides: Record<string, string>;
  /** Detected ecosystem ("node", "rust", …), or null if none recognized. */
  ecosystem: string | null;
  onClose: () => void;
  onApply: (overrides: Record<string, string>) => void;
}

export function RunSettingsSheet({ rows, overrides, ecosystem, onClose, onApply }: Props) {
  // Draft = working copy while the sheet is open; uncommitted until Apply.
  const [draft, setDraft] = useState<Record<string, string>>(overrides);
  const setRow = (id: string, v: string | null) =>
    setDraft((d) => {
      const next = { ...d };
      if (v == null || v === "") delete next[id];
      else next[id] = v;
      return next;
    });
  const revert = (id: string) => setRow(id, null);
  const reset = () => setDraft({});
  // Rows whose draft differs from the detected value — the real overrides.
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
              <EcosystemBadge ecosystem={ecosystem} />
            </div>
          </div>
          <button className="run-sheet-x iflex-center" onClick={onClose} aria-label="Close">
            <Icon name="close" size={12} />
          </button>
        </div>

        {/* Body */}
        <div className="run-sheet-body">
          <RunConfigEditor
            rows={rows}
            draft={draft}
            scope="agent"
            onChange={setRow}
            onRevert={revert}
          />
        </div>

        {/* Footer */}
        <div className="run-sheet-f flex-center">
          <div className="rsf-status truncate text-sm">
            {changed.length === 0 ? (
              <span style={{ color: "var(--fg-3)" }}>No overrides — using project settings</span>
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
            <Button variant="ghost" onClick={reset} disabled={Object.keys(draft).length === 0}>
              Reset all
            </Button>

            <Button variant="primary" onClick={() => onApply(draft)} disabled={!dirty}>
              <Icon name="refresh" size={11} />
              Apply &amp; restart
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}
