import { Icon } from "@desktop/components/Icon";
import { useConnectedHosts } from "../store";

/** A row of the paired hosts that are connected, by name, with `value` picked:
 *  which host something is to happen on. What picking one means is the
 *  caller's. */
export function HostPicker({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (hostKey: string) => void;
}) {
  const hosts = useConnectedHosts();
  return (
    <div className="host-pick" role="radiogroup" aria-label={label}>
      <span className="lbl">{label}</span>
      <div className="chips">
        {hosts.map((h) => (
          <button
            key={h.hostKey}
            type="button"
            role="radio"
            aria-checked={h.hostKey === value}
            className={`chip${h.hostKey === value ? " on" : ""}`}
            onClick={() => onChange(h.hostKey)}
          >
            <Icon name="laptop" size={12} />
            {h.name}
          </button>
        ))}
      </div>
    </div>
  );
}
