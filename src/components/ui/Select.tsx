import { useState, type ReactNode } from "react";
import { Icon } from "../Icon";
import { Scrim } from "./Scrim";

export interface SelectOption<T extends string> {
  value: T;
  label: string;
  /** Optional secondary text shown muted to the right of the label. */
  hint?: string;
  /** Optional leading visual (e.g. a provider icon) shown before the label,
   *  in both the trigger and the option row. */
  icon?: ReactNode;
}

interface Props<T extends string> {
  value: T;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  placeholder?: string;
  ariaLabel?: string;
}

/** A styled single-select dropdown — a drop-in replacement for a native
 *  `<select>` that matches the app's `.dd` popover look (built on `Scrim`).
 *  Generic over the option value type so callers keep their string unions. */
export function Select<T extends string>({
  value,
  options,
  onChange,
  disabled = false,
  placeholder = "Select…",
  ariaLabel,
}: Props<T>) {
  const [open, setOpen] = useState(false);
  const selected = options.find((o) => o.value === value);

  return (
    <div className="ui-select">
      <button
        type="button"
        className={`ui-select-trigger ${open ? "open" : ""}`}
        disabled={disabled}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => !disabled && setOpen((v) => !v)}
      >
        {selected?.icon && <span className="ui-select-icon">{selected.icon}</span>}
        <span className={`ui-select-value ${selected ? "" : "is-placeholder"}`}>
          {selected?.label ?? placeholder}
        </span>
        <Icon name="chevD" size={11} />
      </button>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="dd ui-select-dd" role="listbox">
            {options.map((o) => (
              <div
                key={o.value}
                role="option"
                aria-selected={o.value === value}
                className={`dd-item ${o.value === value ? "active" : ""}`}
                onClick={() => {
                  onChange(o.value);
                  setOpen(false);
                }}
              >
                {o.icon && <span className="ui-select-icon">{o.icon}</span>}
                <span className="di-l">{o.label}</span>
                {o.hint && <span className="di-m">{o.hint}</span>}
                {o.value === value && <Icon name="check" size={12} />}
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
