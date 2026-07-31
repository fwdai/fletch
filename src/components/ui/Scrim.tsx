import { useEffect } from "react";

/** Full-viewport scrim that closes a popover on click or Escape. Used for
 *  new-project, settings, model-picker, etc.
 *
 *  Invisible by default — popovers shouldn't dim the app behind them. Pass
 *  `blur` for the dimmed, blurred backdrop the centered modals use; that's the
 *  same recipe `.ps-overlay` (Project Settings) and `.history-overlay`
 *  hand-roll, and those predate this prop and can adopt it. */
export function Scrim({
  onClose,
  zIndex = 199,
  blur = false,
}: {
  onClose: () => void;
  zIndex?: number;
  blur?: boolean;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div
      className={blur ? "ui-scrim" : undefined}
      style={{ position: "fixed", inset: 0, zIndex }}
      onClick={onClose}
    />
  );
}
