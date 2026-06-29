import { type ReactNode, useEffect, useRef, useState } from "react";
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
 *  Generic over the option value type so callers keep their string unions.
 *
 *  Keyboard: options are real `<button>`s (focusable, Enter/Space select);
 *  ArrowUp/Down move between them, Home/End jump to the ends, and Escape
 *  closes (handled by `Scrim`). The selected option is focused on open. */
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
  const listRef = useRef<HTMLDivElement>(null);

  // On open, move focus to the selected option (or the first) so arrow keys
  // and Enter work immediately without a Tab.
  useEffect(() => {
    if (!open) return;
    const list = listRef.current;
    if (!list) return;
    const items = list.querySelectorAll<HTMLButtonElement>("[role='option']");
    const activeIdx = Math.max(
      0,
      options.findIndex((o) => o.value === value),
    );
    items[activeIdx]?.focus();
  }, [open, options, value]);

  function onListKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    const items = Array.from(
      listRef.current?.querySelectorAll<HTMLButtonElement>("[role='option']") ?? [],
    );
    if (items.length === 0) return;
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown") {
      e.preventDefault();
      items[(current + 1) % items.length]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      items[(current - 1 + items.length) % items.length]?.focus();
    } else if (e.key === "Home") {
      e.preventDefault();
      items[0]?.focus();
    } else if (e.key === "End") {
      e.preventDefault();
      items[items.length - 1]?.focus();
    }
  }

  function pick(v: T) {
    onChange(v);
    setOpen(false);
  }

  return (
    <div className="ui-select">
      <button
        type="button"
        className={`ui-select-trigger flex-center text-base ${open ? "open" : ""}`}
        disabled={disabled}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => !disabled && setOpen((v) => !v)}
      >
        {selected?.icon && <span className="ui-select-icon iflex-center">{selected.icon}</span>}
        <span className={`ui-select-value truncate ${selected ? "" : "is-placeholder"}`}>
          {selected?.label ?? placeholder}
        </span>
        <Icon name="chevD" size={11} />
      </button>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div ref={listRef} className="dd ui-select-dd" role="listbox" onKeyDown={onListKeyDown}>
            {options.map((o) => (
              <button
                key={o.value}
                type="button"
                role="option"
                aria-selected={o.value === value}
                className={`dd-item flex-center ${o.value === value ? "active" : ""}`}
                onClick={() => pick(o.value)}
              >
                {o.icon && <span className="ui-select-icon iflex-center">{o.icon}</span>}
                <span className="di-l">{o.label}</span>
                {o.hint && <span className="di-m">{o.hint}</span>}
                {o.value === value && <Icon name="check" size={12} />}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
