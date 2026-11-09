import type { DiffBaseMode } from "@/api";
import { Button } from "@/components/ui/Button";

interface EmptyChangesProps {
  base: DiffBaseMode;
  /** The agent's work is in commits (any ahead of the base), so an empty
   *  Uncommitted list has somewhere to point. */
  committed: boolean;
  onShowAll: () => void;
}

/** An empty list says which scope is empty. Under Uncommitted with commits
 *  ahead the work is all committed, so it offers the way to it. */
export function EmptyChanges({ base, committed, onShowAll }: EmptyChangesProps) {
  if (base === "head" && committed) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">Everything is committed.</div>
        <Button variant="link" size="sm" onClick={onShowAll}>
          Show all changes
        </Button>
      </div>
    );
  }
  return (
    <div className="empty-msg" style={{ margin: "auto" }}>
      <div className="et">No changes yet</div>
      <div>Edits the agent makes to this checkout will show up here.</div>
    </div>
  );
}
