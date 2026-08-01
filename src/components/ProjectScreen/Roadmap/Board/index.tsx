import { useEffect, useRef } from "react";
import { Icon } from "@/components/Icon";
import { HORIZONS } from "../types";
import type { RoadmapState } from "../useRoadmap";
import { HorizonGroup } from "./HorizonGroup";
import { ItemCard } from "./ItemCard";
import { ProductMap } from "./ProductMap";

/** The board the PM agent maintains: horizon groups of expandable items, with
 *  a proposal's additions rendered inline as ghost rows. Sibling tab shows the
 *  product map the agent reasons against. */
export function Board({ roadmap }: { roadmap: RoadmapState }) {
  const { items, ghosts, moves, map, shipped, tab, setTab, openCodes, toggleItem, focusCode } =
    roadmap;

  const scroll = useRef<HTMLDivElement>(null);
  const rows = useRef<Record<string, HTMLDivElement | null>>({});

  // Bring a jumped-to row into the upper third of the viewport.
  useEffect(() => {
    if (!focusCode) return;
    const el = rows.current[focusCode];
    const box = scroll.current;
    if (!el || !box) return;
    const top = el.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop;
    box.scrollTop = Math.max(0, top - box.clientHeight / 3);
  }, [focusCode]);

  const ghostCodes = new Set(ghosts.map((g) => g.code));
  const movedCodes = new Set(moves.map((m) => m.code));

  return (
    <aside className="rm-board">
      <div className="rm-board-h flex-center">
        <div className="rm-tabs">
          <button
            type="button"
            className={`rm-tab iflex-center text-sm ${tab === "roadmap" ? "active" : ""}`}
            onClick={() => setTab("roadmap")}
          >
            <Icon name="map" size={12} /> Roadmap
          </button>
          <button
            type="button"
            className={`rm-tab iflex-center text-sm ${tab === "map" ? "active" : ""}`}
            onClick={() => setTab("map")}
          >
            <Icon name="cube" size={12} /> Product map
          </button>
        </div>
        <span className="grow" />
        {tab === "roadmap" && (
          <span className="rm-shipped iflex-center mono text-xs">
            <Icon name="merge" size={11} />
            {shipped} shipped
          </span>
        )}
      </div>

      <div className="rm-board-scroll" ref={scroll}>
        {tab === "map" ? (
          <ProductMap map={map} />
        ) : (
          HORIZONS.map((h) => {
            // Proposed additions sit in their target group, so the user sees
            // where a change would land before committing to it.
            const rowsFor = [
              ...items.filter((i) => i.horizon === h.id),
              ...ghosts.filter((i) => i.horizon === h.id),
            ];
            return (
              <HorizonGroup key={h.id} label={h.label} note={h.note} count={rowsFor.length}>
                {rowsFor.map((it) => (
                  <ItemCard
                    key={it.code}
                    item={it}
                    ghost={ghostCodes.has(it.code)}
                    open={openCodes.has(it.code)}
                    focused={focusCode === it.code || movedCodes.has(it.code)}
                    onToggle={() => toggleItem(it.code)}
                    cardRef={(el) => {
                      rows.current[it.code] = el;
                    }}
                  />
                ))}
              </HorizonGroup>
            );
          })
        )}
      </div>
    </aside>
  );
}
