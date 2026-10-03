import { type ReactNode, useRef, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { DropdownMenu } from "./Dropdown";
import { IconButton } from "./IconButton";
import { usePlacement } from "./usePlacement";

/** An icon button that opens a menu anchored to it: below by default, above
 *  when it sits too close to the bottom of its scroller for the menu to fit
 *  (see `usePlacement`). Any click outside dismisses it. `children` renders
 *  the menu's rows, and gets a `close` for the row that acts. */
export function MenuButton({
  icon,
  tip,
  compact = false,
  disabled,
  className,
  children,
}: {
  icon: IconName;
  /** Tooltip, and the button's accessible name. */
  tip: string;
  /** The `xs` button, for a dense row (a turn footer, a message's actions). */
  compact?: boolean;
  disabled?: boolean;
  /** Layout class for the trigger. */
  className?: string;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const placement = usePlacement(open, wrapRef, menuRef);
  const close = () => setOpen(false);

  return (
    <div className="dd-anchor" ref={wrapRef}>
      <IconButton
        size={compact ? "xs" : undefined}
        tip={tip}
        className={className}
        aria-label={tip}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name={icon} size={compact ? 12 : undefined} />
      </IconButton>
      {open && (
        <>
          {/* Full-viewport scrim: any outside click dismisses the menu. */}
          <div className="dd-anchor-scrim" onClick={close} />
          <DropdownMenu ref={menuRef} role="menu" className={placement}>
            {children(close)}
          </DropdownMenu>
        </>
      )}
    </div>
  );
}
