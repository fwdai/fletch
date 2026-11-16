import { type RefObject, useEffect, useRef } from "react";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** What Tab can reach inside `scope`, in order. */
function tabStops(scope: HTMLElement): HTMLElement[] {
  return Array.from(scope.querySelectorAll<HTMLElement>(FOCUSABLE));
}

/** Keep the keyboard inside a dialog for as long as it is mounted: focus moves
 *  into `ref`'s element as it opens, Tab cycles within it, and focus goes back
 *  to where it was when it closes. Without it a dialog opened by a chord
 *  leaves the caret in the field behind its scrim.
 *
 *  Focus lands on the dialog itself, not its first control: a focused Close
 *  button would turn the next Enter into a dismissal. Content that focuses a
 *  field of its own (`autoFocus`, an effect) keeps it. On close, focus is
 *  returned only when nothing else has taken it since — an action that moved
 *  it on purpose (a new draft's composer) wins. */
export function useFocusScope(ref: RefObject<HTMLElement | null>) {
  // Read while rendering: by the time an effect runs, `autoFocus` inside the
  // dialog has already moved it.
  const returnTo = useRef<Element | null>(document.activeElement);

  useEffect(() => {
    const scope = ref.current;
    if (!scope) return;
    scope.dataset.focusScope = "";
    if (!scope.hasAttribute("tabindex")) scope.tabIndex = -1;
    if (!scope.contains(document.activeElement)) scope.focus();

    const onKey = (e: KeyboardEvent) => {
      // A control that uses Tab itself (a terminal) or a dialog nested in
      // this one has already answered.
      if (e.key !== "Tab" || e.defaultPrevented) return;
      const stops = tabStops(scope);
      const first = stops[0];
      const last = stops.at(-1);
      const at = document.activeElement;
      if (!first || !last) {
        e.preventDefault();
      } else if (e.shiftKey && (at === first || at === scope)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && at === last) {
        e.preventDefault();
        first.focus();
      }
    };
    scope.addEventListener("keydown", onKey);

    return () => {
      scope.removeEventListener("keydown", onKey);
      const at = document.activeElement;
      const untouched = !at || at === document.body || scope.contains(at);
      const el = returnTo.current;
      if (untouched && el instanceof HTMLElement && el.isConnected) el.focus();
    };
  }, [ref]);
}
