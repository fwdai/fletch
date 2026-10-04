import { useRef, useState } from "react";
import { api } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";

/** The "Binary" detail row in a provider card. Read-only by default — showing
 *  the effective path the way it always has — but with an inline pencil that
 *  turns it into a validated text editor. Saving runs `validate_agent_bin` so a
 *  broken path is rejected with an inline error before it's ever persisted; an
 *  empty value clears the override and reverts to auto-detection.
 *
 *  The override itself lives in the store (and the settings DB); this component
 *  only renders state and reports edits up through `onSave`. */
export function BinaryPathRow({
  providerLabel,
  effectivePath,
  override,
  resolved,
  onHost,
  gate,
  onSave,
}: {
  providerLabel: string;
  /** What to display when not editing: override ?? live probe ?? default. */
  effectivePath: string;
  /** The raw custom path, if one is set. Drives the "Custom" tag. */
  override?: string;
  /** Whether the probe resolved the binary to an executable file (the probe's
   *  `path`, not its parsed version — a working CLI may report no version). An
   *  override that fails this is flagged as not-runnable. */
  resolved: boolean;
  /** The remote host the override belongs to, when one is on screen: the path
   *  is on ITS disk, so this Mac can't validate it (the host's next spawn is
   *  the check). Undefined for the local environment. */
  onHost?: string;
  /** Why the override can't be edited here, or null/undefined when it can. */
  gate?: string | null;
  /** Persist (path) or clear (null) the override. Resolves once the store and
   *  backend have updated; rejects to keep the editor open on failure. */
  onSave: (path: string | null) => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const broken = !!override && !resolved;

  const beginEdit = () => {
    setDraft(override ?? "");
    setError(null);
    setEditing(true);
    // Focus after the input mounts.
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  const cancel = () => {
    setEditing(false);
    setError(null);
  };

  const commit = async () => {
    if (busy) return;
    const value = draft.trim();
    setBusy(true);
    setError(null);
    try {
      // Empty clears the override — no validation needed to go back to auto.
      // A host's path is checked by the host, not against this Mac's disk.
      if (value && !onHost) {
        const result = await api.validateAgentBin(value);
        if (!result.executable) {
          setError("No executable file at this path.");
          return;
        }
      }
      await onSave(value || null);
      setEditing(false);
    } catch {
      setError("Couldn't save this path. Try again.");
    } finally {
      setBusy(false);
    }
  };

  const reset = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await onSave(null);
      setEditing(false);
    } catch {
      // Mirror commit()'s failure feedback — reset runs from the display view,
      // so the error renders below the row there (see the !editing branch).
      setError("Couldn't reset this path. Try again.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="set-prov-drow">
      <span className="set-prov-dk">Binary</span>
      {editing ? (
        <div className="set-prov-bin-edit">
          <div className="set-prov-bin-input-row flex-center">
            <input
              ref={inputRef}
              className="set-prov-bin-input mono text-sm"
              value={draft}
              placeholder={effectivePath}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              disabled={busy}
              onChange={(e) => {
                setDraft(e.target.value);
                if (error) setError(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") void commit();
                else if (e.key === "Escape") cancel();
              }}
            />
            <IconButton
              className="sm-i"
              tip="Save"
              aria-label="Save path"
              disabled={busy}
              onClick={() => void commit()}
            >
              <Icon name="check" size={14} />
            </IconButton>
            <IconButton
              className="sm-i"
              tip="Cancel"
              aria-label="Cancel"
              disabled={busy}
              onClick={cancel}
            >
              <Icon name="close" size={14} />
            </IconButton>
          </div>
          {error ? (
            <div className="set-prov-bin-err text-xs">{error}</div>
          ) : (
            <div className="set-prov-bin-hint text-xs">
              Absolute path to the {providerLabel} binary{onHost ? ` on ${onHost}` : ""}. Leave
              blank to auto-detect.
            </div>
          )}
        </div>
      ) : (
        <div className="set-prov-bin-edit">
          <div className="set-prov-bin-view flex-center">
            <span className={`set-prov-dv mono ${broken ? "broken" : ""}`}>{effectivePath}</span>
            {override && <span className="set-badge custom">Custom</span>}
            {broken && <span className="set-prov-bin-warn text-xs">not found</span>}
            <span className="grow" />
            {override && (
              <Button
                variant="ghost"
                size="sm"
                disabled={busy || !!gate}
                onClick={() => void reset()}
              >
                Reset to auto
              </Button>
            )}
            <IconButton
              className="sm-i"
              tip={gate ?? "Edit path"}
              aria-label="Edit binary path"
              disabled={!!gate}
              onClick={beginEdit}
            >
              <Icon name="edit" size={13} />
            </IconButton>
          </div>
          {error && <div className="set-prov-bin-err text-xs">{error}</div>}
        </div>
      )}
    </div>
  );
}
