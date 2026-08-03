import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";
import type { GroupDnd } from "./useBoardDnd";

/** One horizon section of the board — a labelled header with a right-aligned
 *  count over a rule-anchored stack of rows. */
export function HorizonGroup({
  label,
  note,
  count,
  empty,
  onAdd,
  dnd,
  children,
}: {
  label: string;
  note: string;
  /** Committed rows only — proposed ones aren't real yet, so they don't count
   *  here either (this is the same number the page header shows). */
  count: number;
  /** Nothing to render at all, committed or proposed. */
  empty: boolean;
  /** Add an item straight into this horizon — the group is the placement, so
   *  the form opens already knowing it. Omitted on a read-only board. */
  onAdd?: () => void;
  /** The group as a drop target: dropping anywhere the rows don't cover appends
   *  to the end of this horizon, which is also the only way into an empty one.
   *  Absent on a read-only board. */
  dnd?: GroupDnd;
  children: ReactNode;
}) {
  return (
    <section className="rm-group">
      <div className="rm-group-h">
        <span className="rm-group-l mono text-xs">{label}</span>
        <span className="rm-group-n text-xs">{note}</span>
        <span className="rm-group-c mono text-xs">{count}</span>
        {onAdd && (
          <button
            type="button"
            className="rm-group-add iflex-center"
            onClick={onAdd}
            aria-label={`Add an item to ${label}`}
          >
            <Icon name="plus" size={11} />
          </button>
        )}
      </div>
      {/* The rows' own stack is the drop zone for "past the last one". Not a
          control: the placement it performs is also reachable from the item
          form's horizon field, so no keyboard path is lost. */}
      <div
        className={`rm-group-body ${dnd?.over ? "dropping" : ""}`}
        onDragOver={dnd?.onDragOver}
        onDragLeave={dnd?.onDragLeave}
        onDrop={dnd?.onDrop}
      >
        {empty ? <div className="rm-empty text-xs">Nothing here yet.</div> : children}
      </div>
    </section>
  );
}
