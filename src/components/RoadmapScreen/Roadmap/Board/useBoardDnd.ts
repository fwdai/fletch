// Drag-to-reorder for the board: the thin React layer over rank.ts.
//
// HTML5 drag and drop, no library. The board is three short lists of rows that
// already have stable ids and a numeric order — everything a drag library would
// bring (sensors, virtualization, keyboard nesting) is weight this surface has no
// use for, and the whole gesture is four handlers plus one piece of state.
//
// This hook decides *where* a drop lands; rank.ts decides *what to write*, and
// the roadmap hook performs it. What is kept here is only the transient state a
// pointer gesture needs: which row is being dragged, and which edge of which row
// the indicator line is currently on.
//
// Only `proposed | open | queued` rows are draggable. Anything from `active` on
// has been dispatched — reordering it would change nothing, since the queue only
// picks among queued items — and a `done` row is off the board entirely.

import { useCallback, useState } from "react";
import type { Horizon, RoadmapItem } from "@/api";
import { dropIndex, planDrop } from "../rank";

/** Which side of a row the drop indicator sits on. */
export type Edge = "before" | "after";

/** What the card needs to take part in a drag. Spread-friendly on purpose: the
 *  card only draws, it decides nothing. */
export interface CardDnd {
  draggable: boolean;
  /** This row is the one being dragged. */
  dragging: boolean;
  /** Draw the drop indicator on this edge of the row, or nowhere. */
  edge: Edge | null;
  onDragStart: (e: React.DragEvent) => void;
  onDragEnd: () => void;
  onDragOver: (e: React.DragEvent) => void;
  onDragLeave: (e: React.DragEvent) => void;
  onDrop: (e: React.DragEvent) => void;
}

/** What a horizon group needs: the drop target for "past the last row", which is
 *  also how an empty group is filled. */
export interface GroupDnd {
  /** A drag is hovering the group's empty space — the append position. */
  over: boolean;
  onDragOver: (e: React.DragEvent) => void;
  onDragLeave: (e: React.DragEvent) => void;
  onDrop: (e: React.DragEvent) => void;
}

/** Can this row be dragged? */
export const isDraggable = (item: RoadmapItem) =>
  item.status === "proposed" || item.status === "open" || item.status === "queued";

/** The dragged row's id, carried on the drag itself so a drop knows what it is
 *  even though the state below already holds it (a drag that started in another
 *  window, or lost state on re-render, still resolves). */
const MIME = "application/x-fletch-roadmap-item";

export function useBoardDnd({
  rows,
  moveItem,
  setRanks,
}: {
  /** Every row the board draws, in board order — the same list the groups are
   *  filtered from, so the insertion arithmetic sees exactly what the user sees. */
  rows: RoadmapItem[];
  /** Cross-group drop: one patch carrying the new horizon and the new rank. */
  moveItem: (id: string, to: Horizon, rank?: number) => void | Promise<void>;
  /** Same-group drop: rank writes only, no history. */
  setRanks: (writes: { id: string; rank: number }[]) => void | Promise<void>;
}) {
  const [dragId, setDragId] = useState<string | null>(null);
  /** Where the indicator is: a row and an edge, or a group's tail. */
  const [target, setTarget] = useState<{ id: string; edge: Edge } | { horizon: Horizon } | null>(
    null,
  );

  const clear = useCallback(() => {
    setDragId(null);
    setTarget(null);
  }, []);

  /** Apply a drop: resolve the destination group's rows (without the dragged
   *  one), turn the hovered edge into an index, and let rank.ts decide the
   *  writes. */
  const drop = useCallback(
    (id: string, horizon: Horizon, at: { id: string; edge: Edge } | null) => {
      const moved = rows.find((r) => r.id === id);
      clear();
      if (!moved || !isDraggable(moved)) return;
      const group = rows.filter((r) => r.horizon === horizon && r.id !== id);
      const index = at ? dropIndex(group, at.id, at.edge) : group.length;
      const plan = planDrop(group, index, id);
      const crossing = moved.horizon !== horizon;

      if (plan.kind === "set") {
        // A no-op drop (dropped back where it already was) writes nothing.
        if (!crossing && moved.rank === plan.rank) return;
        if (crossing) void moveItem(id, horizon, plan.rank);
        else void setRanks([{ id, rank: plan.rank }]);
        return;
      }
      // The fallback: the group's ranks left no gap, so it is renumbered. The
      // dragged row's own write carries the horizon when it is also moving.
      const own = plan.writes.find((w) => w.id === id);
      void setRanks(plan.writes.filter((w) => w.id !== id));
      if (crossing) void moveItem(id, horizon, own?.rank);
      else if (own) void setRanks([own]);
    },
    [rows, clear, moveItem, setRanks],
  );

  /** The props for one row. `horizon` is the group it is drawn in. */
  const cardDnd = useCallback(
    (item: RoadmapItem): CardDnd | undefined => {
      const draggable = isDraggable(item);
      const hovering = target && "id" in target && target.id === item.id ? target.edge : null;
      return {
        draggable,
        dragging: dragId === item.id,
        // Never point at the row being dragged: "insert before yourself" is not
        // a move, and the line would follow the cursor around its own card.
        edge: dragId && dragId !== item.id ? hovering : null,
        onDragStart: (e) => {
          if (!draggable) return;
          e.dataTransfer.setData(MIME, item.id);
          e.dataTransfer.effectAllowed = "move";
          setDragId(item.id);
        },
        onDragEnd: clear,
        onDragOver: (e) => {
          if (!dragId || dragId === item.id) return;
          // Claim the drop before the group's handler sees it: the row is the
          // more specific target.
          e.preventDefault();
          e.stopPropagation();
          e.dataTransfer.dropEffect = "move";
          const box = e.currentTarget.getBoundingClientRect();
          const edge: Edge = e.clientY < box.top + box.height / 2 ? "before" : "after";
          setTarget((prev) =>
            prev && "id" in prev && prev.id === item.id && prev.edge === edge
              ? prev
              : { id: item.id, edge },
          );
        },
        onDragLeave: (e) => {
          // Only when the pointer actually left this row — a move onto a child
          // element fires `dragleave` on the parent too.
          if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
          setTarget((prev) => (prev && "id" in prev && prev.id === item.id ? null : prev));
        },
        onDrop: (e) => {
          const id = e.dataTransfer.getData(MIME) || dragId;
          if (!id) return;
          e.preventDefault();
          e.stopPropagation();
          const box = e.currentTarget.getBoundingClientRect();
          const edge: Edge = e.clientY < box.top + box.height / 2 ? "before" : "after";
          drop(id, item.horizon, { id: item.id, edge });
        },
      };
    },
    [clear, dragId, drop, target],
  );

  /** The props for one horizon group: dropping anywhere the rows don't cover
   *  appends to the end of that group — and is the only way into an empty one. */
  const groupDnd = useCallback(
    (horizon: Horizon): GroupDnd => ({
      over: dragId != null && target != null && "horizon" in target && target.horizon === horizon,
      onDragOver: (e) => {
        if (!dragId) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        setTarget((prev) =>
          prev && "horizon" in prev && prev.horizon === horizon ? prev : { horizon },
        );
      },
      onDragLeave: (e) => {
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setTarget((prev) => (prev && "horizon" in prev && prev.horizon === horizon ? null : prev));
      },
      onDrop: (e) => {
        const id = e.dataTransfer.getData(MIME) || dragId;
        if (!id) return;
        e.preventDefault();
        drop(id, horizon, null);
      },
    }),
    [dragId, drop, target],
  );

  return { cardDnd, groupDnd };
}
