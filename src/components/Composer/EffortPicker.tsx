import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Scale } from "@/components/ui/Scale";
import { Scrim } from "@/components/ui/Scrim";
import type { ThinkingLevel } from "@/data/providerDetail";
import { RailGroup, RailPiece } from "./RunRail";

/** The rail's last piece: the thinking effort the agent runs at. A click
 *  opens a `Scale` of the levels the current model offers, with the level in
 *  force focused; a pick closes it, and so does Escape. On a provider that
 *  bakes effort into the process each pick restarts the agent, and the scale
 *  says so. */
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
  // The level being previewed, named in the head while there is one.
  const [previewed, setPreviewed] = useState<string | null>(null);
  const current = levels.find((l) => l.value === value);
  const tip = restarts
    ? "Thinking effort — changing restarts the agent (rebuilds cache)"
    : "Thinking effort";

  function close() {
    setOpen(false);
    setPreviewed(null);
  }

  function pick(next: string) {
    close();
    if (next !== value) onPick(next);
  }

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
              <span className="effort-now">{previewed ?? current?.label ?? levels[0]?.label}</span>
            </div>
            <Scale
              stops={levels}
              value={value}
              ariaLabel="Thinking effort"
              autoFocus
              onPreview={(stop) => setPreviewed(stop?.label ?? null)}
              onPick={pick}
            />
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
