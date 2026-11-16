import { useEscape } from "./useEscape";

/** The scrim layer of a popover a chord opens from anywhere (quick settings,
 *  usage): one under the card's `--z-app-popover`, so the pair sits over
 *  an overlay that was already up. */
export const APP_POPOVER_SCRIM_Z = 419;

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
