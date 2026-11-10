import { useEffect, useState } from "react";
import { SetRow } from "../../primitives";

/** Settings › Devices › Clients › the listen port. Committed on blur/Enter; the
 *  host validates (and, when listening, rebinds) before the value is stored, so
 *  the field follows the reported port rather than the keystrokes. */
export function PortRow({
  port,
  disabled,
  onSet,
}: {
  /** The port the host reports — bound when listening, configured otherwise. */
  port: number;
  disabled?: boolean;
  onSet: (port: number) => void;
}) {
  const [draft, setDraft] = useState(String(port));

  useEffect(() => {
    setDraft(String(port));
  }, [port]);

  const commit = () => {
    const next = Number.parseInt(draft.trim(), 10);
    if (!Number.isInteger(next) || next < 1 || next > 65535) {
      setDraft(String(port));
      return;
    }
    if (next !== port) onSet(next);
  };

  return (
    <SetRow
      title="Port"
      sub="Local port other devices dial on your network. After a change, devices already paired reach this Mac through the relay."
    >
      <input
        className="set-text-field set-port mono text-sm"
        inputMode="numeric"
        value={draft}
        spellCheck={false}
        disabled={disabled}
        aria-label="Remote access port"
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          else if (e.key === "Escape") setDraft(String(port));
        }}
      />
    </SetRow>
  );
}
