import { useEscape } from "./useEscape";

/** Full-viewport scrim that closes a popover on click or Escape. Used for
 *  new-project, settings, model-picker, etc. — and, through `Modal`, by every
 *  centered modal dialog.
 *
 *  Invisible by default — popovers shouldn't dim the app behind them. Pass
 *  `blur` for the dimmed, blurred backdrop the centered modals use; `.modal-sheet`'s
 *  overlay wears the same `.ui-scrim` class for that recipe. */
export function Scrim({
  onClose,
  zIndex = 199,
  blur = false,
}: {
  onClose: () => void;
  zIndex?: number;
  blur?: boolean;
}) {
  useEscape(onClose);
  return (
    <div
      className={blur ? "ui-scrim" : undefined}
      style={{ position: "fixed", inset: 0, zIndex }}
      onClick={onClose}
    />
  );
}
