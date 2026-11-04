import { type KeyboardEvent, useEffect, useRef, useState } from "react";

export interface ScaleStop<T extends string = string> {
  value: T;
  label: string;
}

/** A value picked from a few ordered stops, as one segmented meter: lowest on
 *  the left, filled up to the stop in force and deepening left to right.
 *  Hovering or the arrows preview the fill; a click, Enter or Space picks.
 *
 *  Keys: ←/→ (and Home/End) move between stops, and only the focus. Nothing is
 *  committed by passing over a stop, because a pick can be expensive (changing
 *  effort restarts some agents). */
export function Scale<T extends string>({
  stops,
  value,
  ariaLabel,
  className,
  disabled = false,
  autoFocus = false,
  onPreview,
  onPick,
}: {
  stops: ScaleStop<T>[];
  value: T | undefined;
  ariaLabel: string;
  className?: string;
  disabled?: boolean;
  /** Focus the stop in force on mount, so the arrows start there. */
  autoFocus?: boolean;
  /** The stop being previewed, or null when the preview ends. */
  onPreview?: (stop: ScaleStop<T> | null) => void;
  /** Fires on every pick, the stop in force included. */
  onPick: (value: T) => void;
}) {
  // The stop the pointer or the keyboard is on, previewing the fill.
  const [hot, setHot] = useState<number | null>(null);
  const segs = useRef<(HTMLButtonElement | null)[]>([]);
  const currentIdx = stops.findIndex((s) => s.value === value);

  useEffect(() => {
    if (autoFocus) segs.current[Math.max(0, currentIdx)]?.focus();
  }, [autoFocus, currentIdx]);

  function preview(i: number | null) {
    setHot(i);
    onPreview?.(i === null ? null : stops[i]);
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const from = hot ?? Math.max(0, currentIdx);
    const to =
      e.key === "ArrowRight"
        ? Math.min(stops.length - 1, from + 1)
        : e.key === "ArrowLeft"
          ? Math.max(0, from - 1)
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? stops.length - 1
              : null;
    if (to === null) return;
    e.preventDefault();
    preview(to);
    segs.current[to]?.focus();
  }

  // What the meter shows filled: the preview while there is one, else the
  // stop in force.
  const shown = hot ?? currentIdx;

  return (
    <div
      className={["ui-scale", className].filter(Boolean).join(" ")}
      role="radiogroup"
      aria-label={ariaLabel}
      aria-disabled={disabled || undefined}
      onKeyDown={onKeyDown}
      onMouseLeave={() => preview(null)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget)) preview(null);
      }}
      style={{ gridTemplateColumns: `repeat(${stops.length}, minmax(44px, 1fr))` }}
    >
      {stops.map((s, i) => {
        const cls = [
          "ui-scale-seg",
          i <= shown ? "is-filled" : "",
          i === currentIdx ? "is-current" : "",
        ]
          .filter(Boolean)
          .join(" ");
        return (
          <button
            key={s.value}
            ref={(el) => {
              segs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={i === currentIdx}
            disabled={disabled}
            // Roving focus: one stop for the group, arrows within it.
            tabIndex={i === (hot ?? Math.max(0, currentIdx)) ? 0 : -1}
            className={cls}
            // The fill deepens left to right: each stop's share.
            style={{ ["--depth" as string]: (i + 1) / stops.length }}
            onMouseEnter={() => {
              if (!disabled) preview(i);
            }}
            onFocus={() => preview(i)}
            onClick={() => onPick(s.value)}
          >
            <span className="ui-scale-bar" />
            <span className="ui-scale-label text-xs">{s.label}</span>
          </button>
        );
      })}
    </div>
  );
}
