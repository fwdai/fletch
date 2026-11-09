import type { DiffBaseMode } from "@/api";
import { Button } from "@/components/ui/Button";

/** An empty list says which scope is empty. Under Uncommitted the agent's work
 *  may all be in commits, so it offers the way to it. */
export function EmptyChanges({ base, onShowAll }: { base: DiffBaseMode; onShowAll: () => void }) {
  if (base === "head") {
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
