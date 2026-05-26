/** Pill switch used by feature/provider rows. CSS-driven — the
 *  `data-on` attribute selects the active state. */
export function Toggle({
  value,
  onChange,
}: {
  value: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      type="button"
      className="sp-toggle"
      data-on={value ? "1" : "0"}
      onClick={() => onChange(!value)}
      aria-checked={value}
      role="switch"
    >
      <i />
    </button>
  );
}
