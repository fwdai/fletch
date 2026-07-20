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
  /** Committed value from the chip: non-empty sets an override, empty reverts. */
  onCommit: (value: string) => void;
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
          {isOverride && inEnv && (
            <IconButton
              size="sm"
              tip="Revert to .env"
              aria-label="Revert to .env"
              onClick={onRevert}
            >
              <Icon name="refresh" size={12} />
            </IconButton>
          )}
          {!inEnv && (
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
          onCommit={onCommit}
        />
        <SandboxToggle value={cfg.shared} onChange={onToggleShare} />
      </div>
    </div>
  );
}
