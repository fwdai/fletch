import { useEffect } from "react";

/** Whether a key press is the user typing, not a command. */
function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return el?.tagName === "INPUT" || el?.tagName === "TEXTAREA" || !!el?.isContentEditable;
}

/** `j` / `k` step forward / back while `enabled`. Bare keys only — a chord
 *  with a modifier belongs to someone else. */
export function useStepKeys(enabled: boolean, next: () => void, prev: () => void): void {
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target)) return;
      if (e.key === "j") next();
      else if (e.key === "k") prev();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled, next, prev]);
}
