/** Pill switch used by feature/provider rows. CSS-driven — the
 *  `data-on` attribute selects the active state. `disabled` is for a switch
 *  whose backing value is unknown or read-only right now: it still shows a
 *  position, but a click must not invent a value. */
export function Toggle({
  value,
  onChange,
  disabled = false,
  title,
}: {
  value: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <button
      type="button"
      className="sp-toggle"
      data-on={value ? "1" : "0"}
      onClick={() => onChange(!value)}
      aria-checked={value}
      role="switch"
      disabled={disabled}
      title={title}
    >
      <i />
    </button>
  );
}
