import { useEffect } from "react";

/** Full-viewport invisible scrim that closes a popover on click or
 *  Escape. Used for new-project, settings, model-picker, etc. */
export function Scrim({ onClose, zIndex = 199 }: { onClose: () => void; zIndex?: number }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return <div style={{ position: "fixed", inset: 0, zIndex }} onClick={onClose} />;
}
