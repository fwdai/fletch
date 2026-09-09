import { useEffect, useRef } from "react";

/** Is this ⌘⇧D (⌃⇧D on a PC keyboard)? Shared with the textarea's handler so
 *  the two can't drift. */
export function isDictationHotkey(e: {
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  key: string;
}): boolean {
  return (e.metaKey || e.ctrlKey) && e.shiftKey && (e.key === "d" || e.key === "D");
}

/** Every mounted composer's toggle, most recently mounted last — the one on
 *  top gets the key. Normally that is the only one; a view that stacks two
 *  composers hands the shortcut to the newer. */
const stack: { current: () => void }[] = [];

function onWindowKey(e: KeyboardEvent) {
  if (!isDictationHotkey(e)) return;
  // The composer's own textarea saw it first and acted (it prevents default).
  if (e.defaultPrevented) return;
  // Any other editable field keeps its keys: dictating into the chat while the
  // caret sits in a commit message would be a surprise.
  const target = e.target as HTMLElement | null;
  const tag = target?.tagName ?? "";
  if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) return;
  const top = stack[stack.length - 1];
  if (!top) return;
  e.preventDefault();
  top.current();
}

/** ⌘⇧D from anywhere in the window — the hands are on the keyboard but the
 *  focus is elsewhere in the chat. One window listener for however many
 *  composers are mounted; the textarea's own handling takes precedence. */
export function useDictationHotkey(toggle: () => void) {
  const ref = useRef(toggle);
  ref.current = toggle;
  useEffect(() => {
    if (stack.length === 0) window.addEventListener("keydown", onWindowKey);
    stack.push(ref);
    return () => {
      const i = stack.indexOf(ref);
      if (i >= 0) stack.splice(i, 1);
      if (stack.length === 0) window.removeEventListener("keydown", onWindowKey);
    };
  }, []);
}
