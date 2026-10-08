import { type ReactNode, useEffect, useRef, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { DropdownMenu } from "./Dropdown";
import { IconButton } from "./IconButton";
import { Scrim } from "./Scrim";
import { usePlacement } from "./usePlacement";

/** What a custom trigger spreads onto its button. */
export interface MenuTriggerProps {
  onClick: () => void;
  disabled?: boolean;
  "aria-haspopup": "menu";
  "aria-expanded": boolean;
}

type TriggerChoice =
  | {
      icon: IconName;
      /** Tooltip, and the button's accessible name. */
      tip: string;
      /** The `xs` button, for a dense row (a turn footer, a message's actions). */
      compact?: boolean;
      /** Layout class for the trigger. */
      className?: string;
      trigger?: never;
    }
  | {
      /** A labelled trigger in place of the icon button; spread the props onto
       *  a `Button`. */
      trigger: (props: MenuTriggerProps) => ReactNode;
      icon?: never;
      tip?: never;
      compact?: never;
      className?: never;
    };

/** A button that opens a menu anchored to it: below by default, above when it
 *  sits too close to the bottom of its scroller for the menu to fit (see
 *  `usePlacement`). An outside click or Escape dismisses it, and so does the
 *  trigger turning disabled. `children` renders the menu's rows, and gets a
 *  `close` for the row that acts. */
export function MenuButton(
  props: TriggerChoice & {
    disabled?: boolean;
    /** Runs as the menu opens, for a menu that re-reads what it lists. */
    onOpen?: () => void;
    /** Layout class for the anchor wrapping trigger and menu. */
    anchorClassName?: string;
    children: (close: () => void) => ReactNode;
  },
) {
  const { disabled, onOpen, anchorClassName, children } = props;
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const shown = open && !disabled;
  const placement = usePlacement(shown, wrapRef, menuRef);
  const close = () => setOpen(false);

  // Otherwise a menu dismissed by disabling would pop back when re-enabled.
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  const triggerProps: MenuTriggerProps = {
    onClick: () => {
      if (!open) onOpen?.();
      setOpen((v) => !v);
    },
    disabled,
    "aria-haspopup": "menu",
    "aria-expanded": shown,
  };

  return (
    <div className={["dd-anchor", anchorClassName].filter(Boolean).join(" ")} ref={wrapRef}>
      {props.trigger ? (
        props.trigger(triggerProps)
      ) : (
        <IconButton
          size={props.compact ? "xs" : undefined}
          tip={props.tip}
          className={props.className}
          aria-label={props.tip}
          {...triggerProps}
        >
          <Icon name={props.icon} size={props.compact ? 12 : undefined} />
        </IconButton>
      )}
      {shown && (
        <>
          <Scrim onClose={close} />
          <DropdownMenu ref={menuRef} role="menu" className={placement}>
            {children(close)}
          </DropdownMenu>
        </>
      )}
    </div>
  );
}
