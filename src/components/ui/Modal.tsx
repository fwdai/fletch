import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { forwardRef } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { IconButton } from "./IconButton";
import { Scrim } from "./Scrim";
import { useEscape } from "./useEscape";

/** Card width — `sm` 380px · `md` 460px · `lg` 520px. */
export type ModalSize = "sm" | "md" | "lg";

/** Stacking layer, mirroring the `--z-popover` / `--z-overlay` tokens.
 *  `popover` is the default; `overlay` is for a modal that must sit above one
 *  (the connect flows, opened from inside another surface). */
export type ModalLayer = "popover" | "overlay";

const LAYER_Z: Record<ModalLayer, number> = { popover: 300, overlay: 400 };

interface ModalProps {
  /** Header icon, left of the title. Rendered in the accent color. */
  icon: IconName;
  /** Header text; doubles as the dialog's accessible name. */
  title: string;
  onClose: () => void;
  size?: ModalSize;
  layer?: ModalLayer;
  className?: string;
  /** Body + footer — compose from `ModalBody` / `ModalFooter`. */
  children: ReactNode;
}

/** Centered modal dialog: dimmed scrim + card + the standard icon/title/close
 *  header. Closes on scrim click and on Escape (both via `Scrim`). The caller
 *  owns whether it's mounted at all — this renders unconditionally.
 *
 *  The backdrop always dims: a modal blocks the app behind it, and saying so is
 *  the point. Something that shouldn't dim isn't a modal — that's a popover, so
 *  reach for a bare `Scrim` (invisible by default) plus your own container. */
export function Modal({
  icon,
  title,
  onClose,
  size = "md",
  layer = "popover",
  className,
  children,
}: ModalProps) {
  const z = LAYER_Z[layer];
  return (
    <>
      <Scrim onClose={onClose} zIndex={z} blur />
      <div
        className={["modal", size === "md" ? "" : size, className].filter(Boolean).join(" ")}
        // The card sits one step above its own scrim.
        style={{ zIndex: z + 1 }}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="modal-h text-base">
          <Icon name={icon} size={15} />
          <span>{title}</span>
          <IconButton aria-label="Close" onClick={onClose}>
            <Icon name="close" />
          </IconButton>
        </div>
        {children}
      </div>
    </>
  );
}

interface ModalBodyProps extends ComponentPropsWithoutRef<"div"> {
  /** Center short, prose-y content instead of stacking it left-aligned. */
  center?: boolean;
}

/** The modal's scrolling content column. */
export function ModalBody({ center, className, children, ...rest }: ModalBodyProps) {
  return (
    <div
      className={["modal-body", center ? "center" : "", className].filter(Boolean).join(" ")}
      {...rest}
    >
      {children}
    </div>
  );
}

/** Action row pinned below the body, separated by a rule. Modals whose actions
 *  scroll with the content put them in the body instead. */
export function ModalFooter({ className, children, ...rest }: ComponentPropsWithoutRef<"div">) {
  return (
    <div className={["modal-foot", className].filter(Boolean).join(" ")} {...rest}>
      {children}
    </div>
  );
}

interface ModalSheetProps {
  onClose: () => void;
  /** The dialog's accessible name. */
  label: string;
  /** Pin the sheet to full viewport height instead of sizing it to content. */
  fill?: boolean;
  className?: string;
  /** The whole sheet — header included; these are wide, bespoke surfaces. */
  children: ReactNode;
}

/** The wide overlay sheet (History, Project Settings): a dimmed backdrop that
 *  closes on click or Escape, wrapping a large card. Unlike `Modal` it brings no
 *  header — these surfaces each have their own. Forwards a ref to the sheet so
 *  callers can hit-test against it. */
export const ModalSheet = forwardRef<HTMLDivElement, ModalSheetProps>(function ModalSheet(
  { onClose, label, fill, className, children },
  ref,
) {
  useEscape(onClose);
  return (
    <div className="modal-overlay ui-scrim" onClick={onClose}>
      <div
        ref={ref}
        className={["modal-sheet", fill ? "fill" : "", className].filter(Boolean).join(" ")}
        role="dialog"
        aria-modal="true"
        aria-label={label}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
});
