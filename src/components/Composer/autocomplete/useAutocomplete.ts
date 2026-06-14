import { useEffect, useState } from "react";
import type { AcSource } from "./types";
import { triggerTokenEnd } from "./triggers";

interface Args {
  /** Candidate sources. At most one is active at a time (their triggers are
   *  mutually exclusive at a given caret); the first with rows wins. */
  sources: AcSource[];
  text: string;
  caret: number;
  setText: (s: string) => void;
  setCaret: (n: number) => void;
  /** Move focus + the real DOM caret to `pos` after a pick. */
  focusAt: (pos: number) => void;
}

interface MenuProps {
  heading: string;
  rows: AcSource["rows"];
  highlight: number;
  onPick: (i: number) => void;
  onHighlight: (i: number) => void;
}

/** Owns the mechanics every autocompletion shares: choosing the active
 *  source, highlight index, Escape-to-dismiss, keyboard navigation, and
 *  applying a pick by splicing the trigger token out of the text. Sources
 *  supply only their rows and what a pick does. */
export function useAutocomplete({
  sources,
  text,
  caret,
  setText,
  setCaret,
  focusAt,
}: Args): { menu: MenuProps | null; onKeyDown: (e: React.KeyboardEvent) => boolean } {
  // Dismissed key is "trigger:query"; it clears itself once the query changes
  // (so typing more re-opens the menu) without a separate reset effect.
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [index, setIndex] = useState(0);

  const active = sources.find((s) => s.query !== null && s.rows.length > 0) ?? null;
  const key = active ? `${active.trigger}:${active.query}` : null;
  const open = active !== null && dismissed !== key;

  useEffect(() => {
    setIndex(0);
  }, [key]);

  function pick(i: number) {
    if (!active || active.query === null) return;
    const start = caret - active.query.length - active.trigger.length;
    const end = triggerTokenEnd(text, caret);
    const res = active.pick(i, { start, end });
    if (!res) return;
    const next = text.slice(0, start) + res.replace + text.slice(end);
    const pos = start + (res.caretOffset ?? res.replace.length);
    setText(next);
    setCaret(pos);
    focusAt(pos);
  }

  function onKeyDown(e: React.KeyboardEvent): boolean {
    if (!open || !active) return false;
    const n = active.rows.length;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => (i + 1) % n);
      return true;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => (i - 1 + n) % n);
      return true;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      pick(index);
      return true;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      setDismissed(key);
      return true;
    }
    return false;
  }

  return {
    menu:
      active && open
        ? {
            heading: active.heading,
            rows: active.rows,
            highlight: index,
            onPick: pick,
            onHighlight: setIndex,
          }
        : null,
    onKeyDown,
  };
}
