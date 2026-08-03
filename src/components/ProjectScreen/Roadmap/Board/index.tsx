import { type RefObject, useEffect, useMemo, useRef, useState } from "react";
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
export function Board({
  roadmap,
  repoPath,
  width,
  asideRef,
}: {
  roadmap: RoadmapState;
  repoPath: string;
  /** Column width set by the splitter, or null for the default even split —
   *  which is a CSS percentage, so it stays even as the window resizes. */
  width?: number | null;
  /** Lets the splitter measure this column at drag start. */
  asideRef?: RefObject<HTMLElement>;
}) {
  const {
    items,
    ghosts,
    proposals,
    map,
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
    events,
    runsById,
    addItem,
    editItem,
    removeItems,
    acceptItems,
    acceptProposals,
    rejectProposals,
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

  /** The PM's asks against admitted rows only. An ask targeting a row that is
   *  itself still a ghost stays out of the batch bar — its card shows no bar
   *  either (see ItemCard), so the count and the buttons agree. */
  const asks = useMemo(() => {
    const ghostIds = new Set(ghosts.map((g) => g.item.id));
    return [...proposals.values()].filter((p) => !ghostIds.has(p.item_id));
  }, [ghosts, proposals]);
  const openNew = (horizon: Horizon) => setEditing({ item: null, horizon });

  // Only the width is set inline. The stylesheet's min/max stay in force, so a
  // width persisted on a wide window can't crush the chat on a narrow one — the
  // column is clamped back on render, not silently overflowed.
  return (
    <aside className="rm-board" ref={asideRef} style={width == null ? undefined : { width }}>
      <div className="rm-board-h flex-center">
        <div className="rm-tabs">
          <button
            type="button"
            className={`rm-tab iflex-center text-sm ${tab === "roadmap" ? "active" : ""}`}
            onClick={() => setTab("roadmap")}
          >
            <Icon name="map" size={13} /> Roadmap
          </button>
          <button
            type="button"
            className={`rm-tab iflex-center text-sm ${tab === "map" ? "active" : ""}`}
            onClick={() => setTab("map")}
          >
            <Icon name="cube" size={13} /> Product map
          </button>
        </div>
        <span className="grow" />
        {/* No shipped count here: the page header already carries it, and two
            copies of the same number a centimetre apart is clutter, not
            reinforcement. */}
        {tab === "roadmap" && !readOnly && (
          <IconButton
            aria-label="Add roadmap item"
            tip="Add an item"
            tipDown
            onClick={() => openNew("next")}
          >
            <Icon name="plus" />
          </IconButton>
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
          the scroller rather than inside it — the ghosts and pending changes it
          acts on can be in three different horizons. Accept-all rules both:
          ghost rows join the roadmap, pending changes are applied. An ask
          against a row that is itself still a ghost is neither counted nor
          bulk-ruled — its card shows no bar for it (rule on the ghost first),
          and bulk-applying a patch to a ticket the user hasn't admitted would
          rule two questions with one click. */}
      {tab === "roadmap" && ghosts.length + asks.length > 1 && (
        <div className="rm-props flex-center text-xs">
          <span className="rm-props-n iflex-center mono">
            <Icon name="sparkle" size={11} />
            {ghosts.length + asks.length} proposed
          </span>
          <span className="rm-props-hint truncate">
            Nothing is on the roadmap until you say so.
          </span>
          <span className="grow" />
          <button
            type="button"
            className="rm-props-x"
            onClick={() => {
              if (ghosts.length) void removeItems(ghosts.map((g) => g.item.id));
              if (asks.length) void rejectProposals(asks.map((p) => p.id));
            }}
          >
            Discard all
          </button>
          <button
            type="button"
            className="rm-props-ok iflex-center"
            onClick={() => {
              if (ghosts.length) void acceptItems(ghosts.map((g) => g.item.id));
              if (asks.length) void acceptProposals(asks.map((p) => p.id));
            }}
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
                  /** The PM's pending ask against this row, if any. */
                  const proposal = proposals.get(row.id);
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
                      events={events.get(row.id)}
                      run={row.run_id ? runsById.get(row.run_id) : undefined}
                      workflowName={
                        writable
                          ? (workflows.resolve(row.workflow_def_id)?.name ?? null)
                          : undefined
                      }
                      onToggle={() => toggleItem(it.code, row.id)}
                      onEdit={
                        ghost || readOnly
                          ? undefined
                          : () => setEditing({ item: row, horizon: row.horizon })
                      }
                      onAccept={ghost && !readOnly ? () => acceptItems([row.id]) : undefined}
                      onDiscard={ghost && !readOnly ? () => removeItems([row.id]) : undefined}
                      proposal={proposal}
                      onAcceptProposal={
                        proposal && !readOnly ? () => acceptProposals([proposal.id]) : undefined
                      }
                      onRejectProposal={
                        proposal && !readOnly ? () => rejectProposals([proposal.id]) : undefined
                      }
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
