import { useEffect, useRef, useState } from "react";
import type { Horizon, RoadmapItem } from "@/api";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { HORIZONS } from "../types";
import type { RoadmapState } from "../useRoadmap";
import { EmptyBoard } from "./EmptyBoard";
import { HorizonGroup } from "./HorizonGroup";
import { ItemCard } from "./ItemCard";
import { ItemDialog } from "./ItemDialog";
import { ProductMap } from "./ProductMap";

/** What the form is open on: an existing row, or a new one destined for
 *  `horizon` — the group whose "+" was pressed. */
type Editing = { item: RoadmapItem | null; horizon: Horizon };

/** The board the PM agent maintains: horizon groups of expandable items, with
 *  a proposal's additions rendered inline as ghost rows. Sibling tab shows the
 *  product map the agent reasons against. */
export function Board({ roadmap, repoPath }: { roadmap: RoadmapState; repoPath: string }) {
  const {
    items,
    ghosts,
    moves,
    map,
    shipped,
    tab,
    setTab,
    openCodes,
    toggleItem,
    focusCode,
    landed,
    loading,
    readOnly,
    error,
    clearError,
    addItem,
    editItem,
    removeItem,
  } = roadmap;

  const [editing, setEditing] = useState<Editing | null>(null);
  /** The row the form is editing, if it isn't creating one. */
  const editRow = editing?.item ?? null;
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
  /** Nothing on the board at all — not even a pending proposal. */
  const blank = !loading && items.length === 0 && ghosts.length === 0;
  const openNew = (horizon: Horizon) => setEditing({ item: null, horizon });

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
          <>
            {/* Zero shipped is the honest state of a new project, and saying so
                adds nothing — the stat appears once there's something to count. */}
            {shipped > 0 && (
              <span className="rm-shipped iflex-center mono text-xs">
                <Icon name="merge" size={11} />
                {shipped} shipped
              </span>
            )}
            {!readOnly && (
              <IconButton
                aria-label="Add roadmap item"
                tip="Add an item"
                onClick={() => openNew("next")}
              >
                <Icon name="plus" />
              </IconButton>
            )}
          </>
        )}
      </div>

      {error && (
        <div className="rm-board-err flex-center text-xs">
          <span className="rm-board-err-t">{error}</span>
          <button type="button" className="rm-board-err-x" onClick={clearError}>
            Dismiss
          </button>
        </div>
      )}

      <div className="rm-board-scroll" ref={scroll}>
        {tab === "map" ? (
          <ProductMap map={map} />
        ) : blank ? (
          <EmptyBoard readOnly={readOnly} onAdd={() => openNew("next")} />
        ) : (
          HORIZONS.map((h) => {
            const real = items.filter((i) => i.horizon === h.id);
            // Proposed additions sit in their target group, so the user sees
            // where a change would land before committing to it — but they
            // don't move the count until they're accepted.
            const rowsFor = [...real, ...ghosts.filter((i) => i.horizon === h.id)];
            return (
              <HorizonGroup
                key={h.id}
                label={h.label}
                note={h.note}
                count={real.length}
                empty={rowsFor.length === 0}
                onAdd={readOnly ? undefined : () => openNew(h.id)}
              >
                {rowsFor.map((it) => {
                  const row = it.item;
                  return (
                    <ItemCard
                      key={it.code}
                      item={it}
                      repoPath={repoPath}
                      // A row the PM has only proposed isn't real yet, whether
                      // it's a pending ghost in this session or a `proposed`
                      // row someone else's session wrote.
                      ghost={ghostCodes.has(it.code) || it.status === "proposed"}
                      open={openCodes.has(it.code)}
                      landed={landed.has(it.code)}
                      focused={focusCode === it.code || movedCodes.has(it.code)}
                      onToggle={() => toggleItem(it.code)}
                      onEdit={
                        row && !readOnly
                          ? () => setEditing({ item: row, horizon: row.horizon })
                          : undefined
                      }
                      cardRef={(el) => {
                        rows.current[it.code] = el;
                      }}
                    />
                  );
                })}
              </HorizonGroup>
            );
          })
        )}
      </div>

      {editing && (
        <ItemDialog
          item={editing.item}
          horizon={editing.horizon}
          onClose={() => setEditing(null)}
          onSave={(draft) => (editRow ? editItem(editRow.id, draft) : addItem(draft))}
          onDelete={editRow ? () => removeItem(editRow.id) : undefined}
        />
      )}
    </aside>
  );
}
