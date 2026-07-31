import { useEffect } from "react";

/** Call `onClose` when Escape is pressed, for as long as the caller is mounted.
 *  Internal to this folder: `Scrim` (popovers + centered modals) and
 *  `ModalSheet` share it so every dismissible surface closes on Escape the same
 *  way. Not on the barrel — components get Escape by using those, not this. Pass
 *  a stable callback (a store action or a `useCallback`) to avoid re-binding. */
export function useEscape(onClose: () => void) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
}
