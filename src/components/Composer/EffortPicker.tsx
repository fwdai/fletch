import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { Icon } from "@/components/Icon";
import { Scrim } from "@/components/ui/Scrim";
import type { ThinkingLevel } from "@/data/providerDetail";
import { RailGroup, RailPiece } from "./RunRail";

/** The rail's last piece: the thinking effort the agent runs at. A click
 *  opens a horizontal scale of the levels the current model offers, low on
 *  the left and the most on the right, filled up to the level in force like
 *  a meter. Hovering previews the fill; a click picks and closes.
 *
 *  Keys: the scale opens with the current level focused, ←/→ (and
 *  Home/End) move between levels, Enter or Space picks, Escape closes. The
 *  arrows only move the focus — on a provider that bakes effort into the
 *  process each pick restarts the agent, so nothing is committed by passing
 *  over a level. On such a session the scale says so. */
export function EffortPicker({
  levels,
  value,
  restarts,
  onPick,
}: {
  levels: ThinkingLevel[];
  value: string | undefined;
  /** Picking a level restarts the running agent (rebuilds its cache). */
  restarts: boolean;
  onPick: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  // The level the pointer or the keyboard is on, previewing the fill.
  const [hot, setHot] = useState<number | null>(null);
  const segs = useRef<(HTMLButtonElement | null)[]>([]);
  const currentIdx = levels.findIndex((l) => l.value === value);
  const current = levels[currentIdx];
  const tip = restarts
    ? "Thinking effort — changing restarts the agent (rebuilds cache)"
    : "Thinking effort";

  // Focus the level in force when the scale opens, so the arrows start there.
  useEffect(() => {
    if (open) segs.current[Math.max(0, currentIdx)]?.focus();
  }, [open, currentIdx]);

  function close() {
    setOpen(false);
    setHot(null);
  }

  function pick(i: number) {
    close();
    if (i !== currentIdx) onPick(levels[i].value);
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const from = hot ?? Math.max(0, currentIdx);
    const to =
      e.key === "ArrowRight"
        ? Math.min(levels.length - 1, from + 1)
        : e.key === "ArrowLeft"
          ? Math.max(0, from - 1)
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? levels.length - 1
              : null;
    if (to === null) return;
    e.preventDefault();
    setHot(to);
    segs.current[to]?.focus();
  }

  // What the meter shows filled: the preview while there is one, else the
  // level in force.
  const shown = hot ?? currentIdx;

  return (
    <RailGroup>
      <RailPiece
        className="run-effort"
        tip={open ? undefined : tip}
        open={open}
        onClick={() => (open ? close() : setOpen(true))}
      >
        <Icon name="sparkle" size={12} />
        <span>{current?.label ?? ""}</span>
      </RailPiece>

      {open && (
        <>
          <Scrim onClose={close} />
          <div className="effort-pop" style={{ bottom: "calc(100% + 8px)", left: 0 }}>
            <div className="effort-head flex-center text-xs">
              <span>Thinking effort</span>
              <span className="effort-now">{levels[Math.max(0, shown)]?.label}</span>
            </div>
            <div
              className="effort-track"
              role="radiogroup"
              aria-label="Thinking effort"
              onKeyDown={onKeyDown}
              onMouseLeave={() => setHot(null)}
              style={{ gridTemplateColumns: `repeat(${levels.length}, minmax(44px, 1fr))` }}
            >
              {levels.map((l, i) => {
                const cls = [
                  "effort-seg",
                  i <= shown ? "is-filled" : "",
                  i === currentIdx ? "is-current" : "",
                  hot !== null && i === hot ? "is-hot" : "",
                ]
                  .filter(Boolean)
                  .join(" ");
                return (
                  <button
                    key={l.value}
                    ref={(el) => {
                      segs.current[i] = el;
                    }}
                    type="button"
                    role="radio"
                    aria-checked={i === currentIdx}
                    // Roving focus: one stop for the group, arrows within it.
                    tabIndex={i === (hot ?? Math.max(0, currentIdx)) ? 0 : -1}
                    className={cls}
                    // The fill deepens left to right: each level's share.
                    style={{ ["--depth" as string]: (i + 1) / levels.length }}
                    onMouseEnter={() => setHot(i)}
                    onFocus={() => setHot(i)}
                    onClick={() => pick(i)}
                  >
                    <span className="effort-bar" />
                    <span className="effort-label text-xs">{l.label}</span>
                  </button>
                );
              })}
            </div>
            <div className="effort-ends flex-center text-xs">
              <span>Faster</span>
              <span>Deeper</span>
            </div>
            {restarts && (
              <div className="effort-note text-xs">
                Changing it restarts the agent, which rebuilds its cache.
              </div>
            )}
          </div>
        </>
      )}
    </RailGroup>
  );
}
