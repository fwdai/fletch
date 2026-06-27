import { useEffect, useState } from "react";
import { triggerTokenEnd } from "./triggers";
import type { AcSource } from "./types";

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
export function useAutocomplete({ sources, text, caret, setText, setCaret, focusAt }: Args): {
  menu: MenuProps | null;
  onKeyDown: (e: React.KeyboardEvent) => boolean;
} {
  // The Escape-dismissed menu, keyed by "trigger:query" so editing the query
  // re-opens it; cleared when the token disappears (see the effect below).
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [index, setIndex] = useState(0);

  const active = sources.find((s) => s.query !== null && s.rows.length > 0) ?? null;
  const key = active ? `${active.trigger}:${active.query}` : null;
  const open = active !== null && dismissed !== key;

  useEffect(() => {
    setIndex(0);
    // Forget a prior Escape once the trigger token goes away entirely (caret
    // moves off it, or the text is cleared on send) so the same query opens
    // again in a later message. While the token is still present, `dismissed`
    // stays set — re-typing the exact query keeps it closed, but any change
    // makes `key` differ and re-opens.
    if (key === null) setDismissed(null);
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
