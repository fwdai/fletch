import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { forwardRef } from "react";

/** The menu shell — a positioned, styled `.dd` container. Purely
 *  presentational: callers own open/close, positioning (via `style`), and
 *  dismissal. Extra classes (e.g. `ac-menu`, `fp-ctx`, `gsa-menu`) go through
 *  `className`. Forwards a ref so callers can measure/clamp it. */
export const DropdownMenu = forwardRef<HTMLDivElement, ComponentPropsWithoutRef<"div">>(
  function DropdownMenu({ className, children, ...rest }, ref) {
    return (
      <div ref={ref} className={["dd", className].filter(Boolean).join(" ")} {...rest}>
        {children}
      </div>
    );
  },
);

interface ItemState {
  /** Selected/current row. */
  active?: boolean;
  /** Dimmed, non-actionable (loading/unavailable). For `as="button"` it also
   *  sets the native `disabled` attribute. */
  disabled?: boolean;
  /** Destructive action tint. */
  danger?: boolean;
}

/** `div` (click-only rows) by default; `button` for menus/options. The rest of
 *  the props (onClick, style, title, role, …) pass straight through to the
 *  element, typed accordingly. */
type DropdownItemProps =
  | (ItemState & { as?: "div" } & Omit<ComponentPropsWithoutRef<"div">, keyof ItemState>)
  | (ItemState & { as: "button" } & Omit<ComponentPropsWithoutRef<"button">, keyof ItemState>);

/** A menu row. Owns the `.dd-item` structure + state classes; children carry
 *  the row content (`.di-i` icon / `.di-l` label / `.di-m` meta, etc.). */
export function DropdownItem({
  active,
  disabled,
  danger,
  className,
  children,
  as = "div",
  ...rest
}: DropdownItemProps) {
  const cls = [
    "dd-item",
    "flex-center",
    active ? "active" : "",
    disabled ? "is-disabled" : "",
    danger ? "danger" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  if (as === "button") {
    return (
      <button
        type="button"
        className={cls}
        disabled={disabled}
        {...(rest as ComponentPropsWithoutRef<"button">)}
      >
        {children}
      </button>
    );
  }
  return (
    <div className={cls} {...(rest as ComponentPropsWithoutRef<"div">)}>
      {children}
    </div>
  );
}

/** Uppercase section heading inside a menu. */
export function DropdownSection({ children }: { children: ReactNode }) {
  return <div className="dd-sect">{children}</div>;
}

/** Thin divider between groups of items. */
export function DropdownSeparator() {
  return <div className="dd-sep" />;
}
