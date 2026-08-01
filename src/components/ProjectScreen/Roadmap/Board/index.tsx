import { useEffect, useRef, useState } from "react";
import type { Horizon, RoadmapItem } from "@/api";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
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
 *  the PM's outstanding proposals rendered inline as ghost rows in the horizon
 *  they'd land in. Sibling tab shows the product map the agent reasons against. */
export function Board({ roadmap, repoPath }: { roadmap: RoadmapState; repoPath: string }) {
  const {
    items,
    ghosts,
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
    notes,
    addItem,
    editItem,
    removeItems,
    acceptItems,
    queueItems,
    unqueueItems,
    markDone,
    workflows,
  } = roadmap;
  const selectRun = useAppStore((s) => s.selectRun);
  const closeProjectScreen = useAppStore((s) => s.closeProjectScreen);

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

      {/* One proposal is ruled on from its own card; a batch gets a single bar,
          so accepting six tickets isn't six trips down the board. It sits above
          the scroller rather than inside it — the ghosts it acts on can be in
          three different horizons. */}
      {tab === "roadmap" && ghosts.length > 1 && (
        <div className="rm-props flex-center text-xs">
          <span className="rm-props-n iflex-center mono">
            <Icon name="sparkle" size={11} />
            {ghosts.length} proposed
          </span>
          <span className="rm-props-hint truncate">
            Nothing is on the roadmap until you say so.
          </span>
          <span className="grow" />
          <button
            type="button"
            className="rm-props-x"
            onClick={() => removeItems(ghosts.map((g) => g.item.id))}
          >
            Discard all
          </button>
          <button
            type="button"
            className="rm-props-ok iflex-center"
            onClick={() => acceptItems(ghosts.map((g) => g.item.id))}
          >
            <Icon name="check" size={11} /> Accept all
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
                  // A proposed row is on the board but not *of* it yet: it is
                  // ruled on rather than edited or sent to an agent.
                  const ghost = it.status === "proposed";
                  // The queue owns everything from `queued` on: an `active`
                  // row is the drainer's, and the user's lever on it is the
                  // run, not the row. `in_review` keeps one manual lever —
                  // "Mark done" — for merges the sweep can't see.
                  const writable = !ghost && !readOnly;
                  return (
                    <ItemCard
                      key={it.code}
                      item={it}
                      repoPath={repoPath}
                      ghost={ghost}
                      open={openCodes.has(it.code)}
                      landed={landed.has(it.code)}
                      focused={focusCode === it.code}
                      note={notes.get(row.id)}
                      workflowName={
                        writable
                          ? (workflows.resolve(row.workflow_def_id)?.name ?? null)
                          : undefined
                      }
                      onToggle={() => toggleItem(it.code)}
                      onEdit={
                        ghost || readOnly
                          ? undefined
                          : () => setEditing({ item: row, horizon: row.horizon })
                      }
                      onAccept={ghost && !readOnly ? () => acceptItems([row.id]) : undefined}
                      onDiscard={ghost && !readOnly ? () => removeItems([row.id]) : undefined}
                      onQueue={
                        writable && it.status === "open" ? () => queueItems([row.id]) : undefined
                      }
                      onUnqueue={
                        writable && it.status === "queued"
                          ? () => unqueueItems([row.id])
                          : undefined
                      }
                      onMarkDone={
                        writable && it.status === "in_review" ? () => markDone(row.id) : undefined
                      }
                      onOpenRun={
                        row.run_id
                          ? () => {
                              // The run lives in the workspace, which this
                              // full-screen page covers — select it, then get
                              // out of the way.
                              selectRun(row.run_id as string);
                              closeProjectScreen();
                            }
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
          workflows={workflows}
          onClose={() => setEditing(null)}
          onSave={(draft) => (editRow ? editItem(editRow.id, draft) : addItem(draft))}
          onDelete={editRow ? () => removeItems([editRow.id]) : undefined}
        />
      )}
    </aside>
  );
}
