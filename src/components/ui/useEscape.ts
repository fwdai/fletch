import { useEffect, useRef } from "react";

/** Every mounted dismissible surface, in mount order. Escape is one key and the
 *  surfaces stack — a folder picker opened from inside the New Project modal —
 *  so only the topmost may answer it, or one keypress would dismiss the whole
 *  pile at once. */
const stack: { fire: () => void }[] = [];

function onKey(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  stack.at(-1)?.fire();
}

/** Call `onClose` when Escape is pressed, for as long as the caller is mounted
 *  and is the innermost surface that wants it. Internal to this folder: `Scrim`
 *  (popovers + centered modals) and `ModalSheet` share it so every dismissible
 *  surface closes on Escape the same way. Not on the barrel — components get
 *  Escape by using those, not this.
 *
 *  The callback need not be stable: it is read through a ref, so the entry's
 *  place in the stack is where the surface was mounted and nothing else. A
 *  re-registering parent must not be able to take Escape from the child modal
 *  it is showing. */
export function useEscape(onClose: () => void) {
  const latest = useRef(onClose);
  useEffect(() => {
    latest.current = onClose;
  }, [onClose]);

  useEffect(() => {
    const entry = { fire: () => latest.current() };
    stack.push(entry);
    if (stack.length === 1) window.addEventListener("keydown", onKey);
    return () => {
      const at = stack.indexOf(entry);
      if (at >= 0) stack.splice(at, 1);
      if (stack.length === 0) window.removeEventListener("keydown", onKey);
    };
  }, []);
}
