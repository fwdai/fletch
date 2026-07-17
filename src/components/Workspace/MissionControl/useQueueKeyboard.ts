import { useEffect, useState } from "react";
import type { ReviewItem } from "./queue";

interface Args {
  items: ReviewItem[];
  /** False while a review modal is open — triage keys pause so they don't fire
   *  behind the surface. */
  active: boolean;
  onEnter: (item: ReviewItem) => void;
  onApprove: (item: ReviewItem) => void;
  onRequestChanges: (item: ReviewItem) => void;
}

/** Keyboard triage for the queue: j/k (and ↓/↑) move the focused card, ↵ opens
 *  its review, `a` approves, `r` requests changes. Mirrors the app's global-
 *  shortcut guard (ignore while typing in an input/textarea) and additionally
 *  bows out for contenteditable, any modifier chord, a focused control (button,
 *  link, card), and while a modal is open — so it never steals or doubles a
 *  keystroke meant for the focused element. */
export function useQueueKeyboard({ items, active, onEnter, onApprove, onRequestChanges }: Args) {
  const [index, setIndex] = useState(0);

  // Keep the focused index in range as the queue shrinks/grows.
  useEffect(() => {
    setIndex((i) => Math.min(Math.max(0, i), Math.max(0, items.length - 1)));
  }, [items.length]);

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const el = document.activeElement as HTMLElement | null;
      const tag = el?.tagName ?? "";
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el?.isContentEditable)
        return;
      // A focused control (a card's Approve button, the card itself) owns its
      // keys — Enter must not both click it and fire the triage action.
      if (tag === "BUTTON" || tag === "A" || el?.getAttribute("role") === "button") return;
      if (items.length === 0) return;
      const cur = items[Math.min(index, items.length - 1)];
      switch (e.key) {
        case "j":
        case "ArrowDown":
          e.preventDefault();
          setIndex((i) => Math.min(items.length - 1, i + 1));
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          setIndex((i) => Math.max(0, i - 1));
          break;
        case "Enter":
          e.preventDefault();
          onEnter(cur);
          break;
        case "a":
          e.preventDefault();
          onApprove(cur);
          break;
        case "r":
          e.preventDefault();
          onRequestChanges(cur);
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active, items, index, onEnter, onApprove, onRequestChanges]);

  return { index, setIndex };
}
