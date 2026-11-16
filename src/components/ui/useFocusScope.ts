import { type RefObject, useEffect, useLayoutEffect, useRef } from "react";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** What Tab can reach inside `scope`, in order. */
function tabStops(scope: HTMLElement): HTMLElement[] {
  return Array.from(scope.querySelectorAll<HTMLElement>(FOCUSABLE));
}

interface Scope {
  el: HTMLElement;
  /** Where focus last was inside it, to go back to rather than to the top. */
  last: HTMLElement | null;
}

/** Every mounted dialog that holds the keyboard, in mount order. They stack —
 *  the palette over a modal, a folder picker over New Project — and only the
 *  one on top holds it (`owner`). */
const scopes: Scope[] = [];

/** Where a dialog sits in the stacking order: the z-index it, or the overlay
 *  it is drawn in, was given. Dialogs are all top-level layers, so theirs
 *  compare directly. */
function layerOf(el: HTMLElement): number {
  for (let node: HTMLElement | null = el; node; node = node.parentElement) {
    const z = Number.parseInt(getComputedStyle(node).zIndex, 10);
    if (!Number.isNaN(z)) return z;
  }
  return 0;
}

/** The dialog the user can see on top: the highest layer, and among equals
 *  the one opened last. Not simply the last one mounted — a popover opened by
 *  a chord under a taller overlay is covered by it, and a covered dialog must
 *  not take the keyboard from the one in front. */
function owner(): Scope | undefined {
  let top: Scope | undefined;
  let topLayer = Number.NEGATIVE_INFINITY;
  for (const scope of scopes) {
    const layer = layerOf(scope.el);
    if (layer >= topLayer) {
      top = scope;
      topLayer = layer;
    }
  }
  return top;
}

/** Focus belongs inside the dialog on top. Whatever moved it out — a global
 *  shortcut focusing the composer, a field behind the scrim focusing itself —
 *  it is handed back, so nothing behind a dialog can be typed into or tabbed
 *  through. Checked a microtask later: a dialog mounting in this same commit
 *  focuses its own field before it has joined the stack. */
function onFocusIn() {
  queueMicrotask(() => {
    const top = owner();
    const at = document.activeElement;
    if (!top || !(at instanceof HTMLElement) || at === document.body) return;
    if (top.el.contains(at)) top.last = at;
    else (top.last?.isConnected ? top.last : top.el).focus();
  });
}

/** Keep the keyboard inside a dialog for as long as it is mounted: focus moves
 *  into `ref`'s element as it opens, stays there (Tab cycles within it, and
 *  focus sent anywhere else comes back), and returns to where it was when the
 *  dialog closes. Without it a dialog opened by a chord leaves the caret in
 *  the field behind its scrim.
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

  // Joined in the layout phase, so the stack is right before the microtask
  // that checks where this commit left focus; left in it too, so the focus
  // handed back below is not taken for an escape.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const scope: Scope = { el, last: null };
    scopes.push(scope);
    if (scopes.length === 1) document.addEventListener("focusin", onFocusIn);

    // On the document, after the dialog's own handlers have had the key.
    const onKey = (e: KeyboardEvent) => {
      // Not this dialog's while another is on top of it; and a control that
      // uses Tab itself (a terminal, the palette's input) has already answered.
      if (e.key !== "Tab" || e.defaultPrevented || owner() !== scope) return;
      const stops = tabStops(el);
      const first = stops[0];
      const last = stops.at(-1);
      const at = document.activeElement;
      if (!first || !last) {
        e.preventDefault();
      } else if (e.shiftKey && (at === first || at === el)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && at === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey);

    return () => {
      document.removeEventListener("keydown", onKey);
      scopes.splice(scopes.indexOf(scope), 1);
      if (scopes.length === 0) document.removeEventListener("focusin", onFocusIn);
    };
  }, [ref]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.dataset.focusScope = "";
    if (!el.hasAttribute("tabindex")) el.tabIndex = -1;
    // Only as the dialog on top: one opened under another leaves focus where
    // the user can see it.
    if (owner()?.el === el && !el.contains(document.activeElement)) el.focus();
    return () => {
      const at = document.activeElement;
      const untouched = !at || at === document.body || el.contains(at);
      const back = returnTo.current;
      if (untouched && back instanceof HTMLElement && back.isConnected) back.focus();
    };
  }, [ref]);
}
