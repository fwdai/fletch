import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { Icon } from "@/components/Icon";

interface Props extends Omit<ComponentPropsWithoutRef<"button">, "name" | "disabled"> {
  /** Leading avatar or glyph. */
  lead?: ReactNode;
  name: ReactNode;
  /** Muted second line under the name. */
  desc?: ReactNode;
  /** Whatever sits at the row's far end, before the check. */
  trail?: ReactNode;
  /** The row is the current pick: tinted and checked. */
  active?: boolean;
  /** The row's side panel is the one showing. */
  hot?: boolean;
  /** Dimmed and inert, via `aria-disabled` so hover (a tooltip, a title)
   *  still reaches it; the caller's handlers guard on it themselves. */
  disabled?: boolean;
  /** Heavier name, for the row that stands apart from the rest of its list. */
  strong?: boolean;
}

/** A pickable row of a `FlyoutMenu`: one choice in a side panel's list, or a
 *  compact row of the main card. */
export function FlyoutOption({
  lead,
  name,
  desc,
  trail,
  active,
  hot,
  disabled,
  strong,
  className,
  ...rest
}: Props) {
  const cls = [
    "fly-option",
    "flex-center",
    active ? "active" : "",
    hot ? "hot" : "",
    disabled ? "is-disabled" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button type="button" aria-disabled={disabled} className={cls} {...rest}>
      {lead}
      <span className="fly-option-main">
        <span className={`fly-option-name truncate text-base ${strong ? "strong" : ""}`}>
          {name}
        </span>
        {desc && <span className="fly-option-desc truncate text-xs">{desc}</span>}
      </span>
      {trail}
      {active && <Icon name="check" size={13} />}
    </button>
  );
}
