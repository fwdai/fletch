import type { DirEntry, DirListing } from "@desktop/api/types/checkout";
import { Icon } from "@desktop/components/Icon";
import { type ReactNode, useEffect, useState } from "react";
import { Sheet } from "../../components/ui";
import { childPath, parentPath } from "../../lib/paths";
import { useStore } from "../../store";

const isHidden = (entry: DirEntry) => entry.name.startsWith(".");

/** Browse the Mac over `list_dir`. Directories only — everything this sheet
 *  can do takes a folder — and the walk is anchored on the `base` the host
 *  reports, never on the string it was asked for: `~` and symlinks resolve
 *  there, not here. */
export function FolderPicker({
  start = "~",
  actionLabel,
  hint,
  onUse,
  busy,
  error,
}: {
  start?: string;
  actionLabel: string;
  hint?: ReactNode;
  onUse: (path: string) => void;
  busy?: boolean;
  /** An error from whatever the caller did with the picked folder; shown in
   *  the same place as a failed listing. */
  error?: string | null;
}) {
  const listDir = useStore((s) => s.listDir);
  const [path, setPath] = useState(start);
  const [listing, setListing] = useState<DirListing | null>(null);
  const [loading, setLoading] = useState(true);
  const [showHidden, setShowHidden] = useState(false);
  const [listError, setListError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setLoading(true);
    setListError(null);
    listDir(path).then(
      (next) => {
        if (!live) return;
        setListing(next);
        setLoading(false);
      },
      (e: unknown) => {
        if (!live) return;
        // Drop the old listing with it, so the crumb and the action point at
        // the folder that failed rather than at the last one that worked.
        setListing(null);
        setListError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      },
    );
    return () => {
      live = false;
    };
  }, [path, listDir]);

  const base = listing?.base ?? path;
  const dirs = (listing?.entries ?? []).filter((e) => e.is_dir);
  const hiddenCount = dirs.filter(isHidden).length;
  const shown = showHidden ? dirs : dirs.filter((e) => !isHidden(e));
  const up = parentPath(base);
  const shownError = error ?? listError;

  return (
    <>
      <div className="ap-path">
        {up && (
          <button
            type="button"
            className="ibtn sm"
            onClick={() => setPath(up)}
            aria-label="Up one folder"
          >
            <Icon name="chevL" size={18} strokeWidth={1.8} />
          </button>
        )}
        <span className="p mono">{base}</span>
      </div>
      <div className="card">
        {shown.map((entry) => (
          <button
            type="button"
            key={entry.name}
            className="row"
            onClick={() => setPath(childPath(base, entry.name))}
          >
            <Icon name="folder" size={16} />
            <div className="main">
              <div className="lbl">{entry.name}</div>
            </div>
            <Icon name="chevR" size={16} className="chev" />
          </button>
        ))}
        {shown.length === 0 && (
          <div className="empty">
            {loading ? "Reading…" : listError ? "Couldn't read this folder" : "No folders here"}
          </div>
        )}
      </div>
      {hiddenCount > 0 && (
        <div className="ap-hid">
          <button type="button" className="chip" onClick={() => setShowHidden(!showHidden)}>
            <Icon name={showHidden ? "minus" : "plus"} size={11} strokeWidth={2.2} />
            {showHidden ? "Hide" : "Show"} {hiddenCount} hidden
          </button>
        </div>
      )}
      {shownError && <div className="err ap-err">{shownError}</div>}
      <button
        type="button"
        className="btn primary block ap-use"
        disabled={busy || !listing}
        onClick={() => onUse(base)}
      >
        {busy ? "Working…" : actionLabel}
      </button>
      {hint && <div className="ap-hint">{hint}</div>}
    </>
  );
}

/** The same picker one sheet up, for choosing a folder without losing the form
 *  underneath. It unmounts a beat after closing, so the next open starts from
 *  `start` again. */
export function FolderPickerSheet({
  open,
  onClose,
  start,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  start?: string;
  onPick: (path: string) => void;
}) {
  return (
    <Sheet
      open={open}
      onClose={onClose}
      stacked
      title="Destination folder"
      right={
        <button type="button" className="tbtn" onClick={onClose}>
          Cancel
        </button>
      }
    >
      <FolderPicker
        start={start}
        actionLabel="Use this folder"
        onUse={(picked) => {
          onPick(picked);
          onClose();
        }}
      />
    </Sheet>
  );
}
