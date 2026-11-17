import { Icon } from "@desktop/components/Icon";
import { useConnectedHosts } from "../store";

/** A row of the paired hosts that are connected, by name, with `value` picked:
 *  which host something is to happen on. What picking one means is the
 *  caller's, and so is which hosts can't be picked (`reasonOf`): those are
 *  greyed, each with its reason on a line under the row. */
export function HostPicker({
  label,
  value,
  onChange,
  reasonOf,
}: {
  label: string;
  value: string;
  onChange: (hostKey: string) => void;
  /** Why a host can't be picked, or null while it can. */
  reasonOf?: (hostKey: string) => string | null;
}) {
  const hosts = useConnectedHosts();
  const reasons = hosts.map((h) => reasonOf?.(h.hostKey) ?? null);
  return (
    <>
      <div className="host-pick" role="radiogroup" aria-label={label}>
        <span className="lbl">{label}</span>
        <div className="chips">
          {hosts.map((h, i) => (
            <button
              key={h.hostKey}
              type="button"
              role="radio"
              aria-checked={h.hostKey === value}
              className={`chip${h.hostKey === value ? " on" : ""}`}
              disabled={reasons[i] !== null}
              onClick={() => onChange(h.hostKey)}
            >
              <Icon name="laptop" size={12} />
              {h.name}
            </button>
          ))}
        </div>
      </div>
      {hosts.map(
        (h, i) =>
          reasons[i] && (
            <div key={h.hostKey} className="chips-note host-note">
              {h.name} · {reasons[i]}
            </div>
          ),
      )}
    </>
  );
}
