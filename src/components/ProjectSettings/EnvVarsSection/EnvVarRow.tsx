import { useState } from "react";
import { Icon } from "@/components/Icon";
import { ValueChip } from "@/components/RunConfig";
import { IconButton } from "@/components/ui/IconButton";
import type { EnvVarCfg } from "@/storage/runEnv";
import { SandboxToggle } from "./SandboxToggle";

interface Props {
  varKey: string;
  /** The value in the repo's `.env`; `undefined` if the var isn't in `.env`. */
  envValue: string | undefined;
  /** The override value (from the keychain) when `cfg.source === "override"`. */
  overrideValue: string | undefined;
  cfg: EnvVarCfg;
  onToggleShare: (shared: boolean) => void;
  /** Committed value from the chip: non-empty sets an override, empty reverts.
   *  Async — the row awaits it so it can hold the revert/remove controls back
   *  until the keychain + document writes settle. */
  onCommit: (value: string) => void | Promise<void>;
  /** Explicit revert-to-`.env` (the revert control by the caption). */
  onRevert: () => void;
  /** Delete a variable that isn't in `.env` (a user-added or now-stale one). */
  onRemove: () => void;
}

/** One environment-variable row. Same value-editing UX as the run-config rows
 *  (shared [`ValueChip`]). The revert control lives with the "Overridden"
 *  caption on the left — grouped with the state it undoes — so the right-hand
 *  value + share cluster stays aligned across every row. */
export function EnvVarRow({
  varKey,
  envValue,
  overrideValue,
  cfg,
  onToggleShare,
  onCommit,
  onRevert,
  onRemove,
}: Props) {
  // Hold the revert/remove controls back while the value chip is being edited
  // *and* while its blur-commit is still writing. A control clicked in that
  // window would fire its mutation concurrently with the commit and the two
  // could race on the keychain + document (e.g. a commit re-adding a variable
  // that remove just deleted). The chip drops `editing` on blur *before* it
  // fires the async `onCommit`, so `editing` alone reopens the controls mid-
  // write; `committing` keeps them hidden until that write settles. Both flip
  // in the same blur event, so React batches them — the controls never flash
  // back in between. (The run-config row only needs the `editing` guard.)
  const [editing, setEditing] = useState(false);
  const [committing, setCommitting] = useState(false);
  const busy = editing || committing;

  const handleCommit = (value: string) => {
    setCommitting(true);
    Promise.resolve(onCommit(value)).finally(() => setCommitting(false));
  };

  const isOverride = cfg.source === "override";
  const inEnv = envValue !== undefined;
  // The effective value: the override when set, otherwise the mirrored `.env`
  // value. Editing to a value that differs from `.env` creates an override.
  const display = isOverride ? (overrideValue ?? "") : (envValue ?? "");

  const caption = isOverride ? (
    inEnv ? (
      <>
        <span className="dot" /> Overridden · differs from <code>.env</code>
      </>
    ) : (
      <>
        <span className="dot" /> Added · not in <code>.env</code>
      </>
    )
  ) : inEnv ? (
    <>
      from <code>.env</code>
    </>
  ) : (
    <>
      not in <code>.env</code>
    </>
  );

  return (
    <div className={`ev-row flex-center${cfg.shared ? "" : " ev-off"}`}>
      <div className="ev-l">
        <div className="ev-key-row iflex-center">
          <span className="ev-key mono text-base truncate">{varKey}</span>
          {isOverride && inEnv && !busy && (
            <IconButton
              size="sm"
              tip="Revert to .env"
              aria-label="Revert to .env"
              onClick={onRevert}
            >
              <Icon name="refresh" size={12} />
            </IconButton>
          )}
          {!inEnv && !busy && (
            <IconButton
              size="sm"
              tip="Remove variable"
              aria-label="Remove variable"
              onClick={onRemove}
            >
              <Icon name="trash" size={12} />
            </IconButton>
          )}
        </div>
        <div className="ev-cap iflex-center text-xs">{caption}</div>
      </div>
      <div className="ev-r iflex-center">
        <ValueChip
          value={display}
          placeholder={envValue ?? "not set"}
          ariaLabel={varKey}
          onCommit={handleCommit}
          onEditingChange={setEditing}
        />
        <SandboxToggle value={cfg.shared} onChange={onToggleShare} />
      </div>
    </div>
  );
}
