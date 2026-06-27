import { type RefObject, useCallback, useEffect, useRef, useState } from "react";

/** One navigable turn: `id` matches the `data-chat-turn` attribute on the
 *  rendered user bubble; `text` is the prompt preview shown in the outline. */
export interface ChatTurn {
  id: number;
  text: string;
}

/** Sleek turn navigator docked to the top-right of the transcript.
 *
 *  - `▲ / ▼` (and Alt+↑ / Alt+↓) step one user turn at a time for sequential
 *    reading.
 *  - Clicking the `n / N` counter opens an outline of every user prompt, so any
 *    message is reachable in a single click — no scrolling the whole log.
 *
 *  Self-contained: it reads turn positions straight from the scroll
 *  container's DOM (`[data-chat-turn]`), so it needs no per-message wiring
 *  beyond that attribute. Hidden when there's nothing to navigate (< 2 turns). */
export function ChatNav({
  scrollRef,
  turns,
}: {
  scrollRef: RefObject<HTMLDivElement | null>;
  turns: ChatTurn[];
}) {
  const [activeId, setActiveId] = useState(0);
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  // The active turn is the last one whose top has scrolled to/above the
  // reading line (a little below the top fold) — standard scroll-spy.
  const recompute = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const top = el.getBoundingClientRect().top;
    let current = 0;
    el.querySelectorAll<HTMLElement>("[data-chat-turn]").forEach((node) => {
      if (node.getBoundingClientRect().top - top <= 96) {
        const id = Number(node.dataset.chatTurn);
        if (!Number.isNaN(id)) current = id;
      }
    });
    setActiveId(current);
  }, [scrollRef]);

  // Keep the active turn in sync with what's on screen. Re-run on scroll, on
  // turn changes, and — via ResizeObserver — when content above the fold grows
  // or collapses (streaming replies, expanding tool rows) without a scroll.
  // All paths share one rAF so bursts coalesce into a single recompute.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    let raf: number | null = null;
    const schedule = () => {
      if (raf != null) return;
      raf = requestAnimationFrame(() => {
        raf = null;
        recompute();
      });
    };
    el.addEventListener("scroll", schedule, { passive: true });
    const content = el.firstElementChild;
    const ro = content ? new ResizeObserver(schedule) : null;
    if (content && ro) ro.observe(content);
    schedule();
    return () => {
      el.removeEventListener("scroll", schedule);
      ro?.disconnect();
      if (raf != null) cancelAnimationFrame(raf);
    };
  }, [scrollRef, recompute, turns]);

  const jumpTo = useCallback(
    (id: number) => {
      const target = scrollRef.current?.querySelector<HTMLElement>(
        `[data-chat-turn="${id}"]`,
      );
      if (!target) return;
      target.scrollIntoView({ behavior: "smooth", block: "start" });
      setActiveId(id);
      setOpen(false);
    },
    [scrollRef],
  );

  const activeIndex = turns.findIndex((t) => t.id === activeId);
  const idx = activeIndex < 0 ? 0 : activeIndex;

  const goPrev = useCallback(() => {
    if (idx > 0) jumpTo(turns[idx - 1].id);
  }, [idx, turns, jumpTo]);
  const goNext = useCallback(() => {
    if (idx < turns.length - 1) jumpTo(turns[idx + 1].id);
  }, [idx, turns, jumpTo]);

  // Alt+↑ / Alt+↓ step between turns from anywhere except while editing text.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
      const ae = document.activeElement as HTMLElement | null;
      const tag = ae?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || ae?.isContentEditable) return;
      e.preventDefault();
      if (e.key === "ArrowUp") goPrev();
      else goNext();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [goPrev, goNext]);

  // Dismiss the outline on outside click or Escape.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onEsc);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onEsc);
    };
  }, [open]);

  if (turns.length < 2) return null;

  return (
    <div className="chat-nav" ref={rootRef}>
      <div className="chat-nav-bar">
        <button
          type="button"
          className="chat-nav-btn"
          aria-label="Previous message"
          title="Previous message (Alt+↑)"
          disabled={idx <= 0}
          onClick={goPrev}
        >
          <Chevron up />
        </button>
        <button
          type="button"
          className="chat-nav-count"
          aria-label="Show all messages"
          aria-haspopup="listbox"
          aria-expanded={open}
          title="Show all messages"
          onClick={() => setOpen((o) => !o)}
        >
          <span className="chat-nav-count-n">
            {idx + 1}
            <span className="chat-nav-sep">/</span>
            {turns.length}
          </span>
          <MenuIcon />
        </button>
        <button
          type="button"
          className="chat-nav-btn"
          aria-label="Next message"
          title="Next message (Alt+↓)"
          disabled={idx >= turns.length - 1}
          onClick={goNext}
        >
          <Chevron />
        </button>
      </div>
      {open && (
        <div className="chat-nav-list" role="listbox" aria-label="Conversation turns">
          {turns.map((t, i) => (
            <button
              key={t.id}
              type="button"
              role="option"
              aria-selected={t.id === activeId}
              className={`chat-nav-row${t.id === activeId ? " is-active" : ""}`}
              onClick={() => jumpTo(t.id)}
            >
              <span className="chat-nav-row-n">{i + 1}</span>
              <span className="chat-nav-row-t">{preview(t.text)}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function preview(text: string): string {
  const oneLine = text.replace(/\s+/g, " ").trim();
  return oneLine.length > 64 ? `${oneLine.slice(0, 64)}…` : oneLine;
}

function Chevron({ up = false }: { up?: boolean }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path
        d={up ? "M3.5 10L8 5.5L12.5 10" : "M3.5 6L8 10.5L12.5 6"}
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** Table-of-contents glyph (three lines) signalling the counter opens a list. */
function MenuIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path
        d="M3 5h10M3 8h10M3 11h7"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
    </svg>
  );
}
