import { useState } from "react";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";
import type { ThinkingLevel } from "@/data/providerDetail";
import { RailGroup, RailPiece } from "./RunRail";

/** The rail's last piece: the thinking effort the agent runs at. Click opens
 *  the levels the current model offers, the one in force checked. On an
 *  existing session of a provider that bakes effort into the process the
 *  menu says so: a pick restarts the agent to re-apply the flag. */
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
  const current = levels.find((l) => l.value === value);
  const tip = restarts
    ? "Thinking effort — changing restarts the agent (rebuilds cache)"
    : "Thinking effort";

  return (
    <RailGroup>
      <RailPiece className="run-effort" tip={tip} open={open} onClick={() => setOpen((v) => !v)}>
        <Icon name="sparkle" size={12} />
        <span>{current?.label ?? ""}</span>
      </RailPiece>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <DropdownMenu className="run-menu" style={{ bottom: "calc(100% + 8px)", left: 0 }}>
            <DropdownSection>Thinking effort</DropdownSection>
            {levels.map((l) => {
              const active = l.value === value;
              return (
                <DropdownItem
                  key={l.value}
                  as="button"
                  role="menuitemradio"
                  aria-checked={active}
                  active={active}
                  onClick={() => {
                    setOpen(false);
                    if (!active) onPick(l.value);
                  }}
                >
                  <span className="di-l">{l.label}</span>
                  {active && <Icon name="check" size={13} />}
                </DropdownItem>
              );
            })}
            {restarts && (
              <div className="run-menu-note text-xs">
                Changing it restarts the agent, which rebuilds its cache.
              </div>
            )}
          </DropdownMenu>
        </>
      )}
    </RailGroup>
  );
}
