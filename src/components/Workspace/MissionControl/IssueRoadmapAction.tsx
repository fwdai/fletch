import { Icon } from "@/components/Icon";
import type { FunnelAction } from "./funnel";

/** The roadmap half of an inbox row's actions. Three states, decided in
 *  funnel.ts: offer the route, say it's already there, or say nothing at all —
 *  no project owns the repo, or its board hasn't answered yet, and an Add
 *  offered without knowing what's on the board is how duplicates get made. */
export function IssueRoadmapAction({ action, onAdd }: { action: FunnelAction; onAdd: () => void }) {
  if (action.kind === "none") return null;
  if (action.kind === "routed") {
    return (
      <button
        type="button"
        className="mc-btn mc-inbox-routed"
        disabled
        title="Already a roadmap item — rule on it there"
      >
        <Icon name="check" size={12} /> On roadmap
      </button>
    );
  }
  return (
    <button
      type="button"
      className="mc-btn"
      onClick={onAdd}
      title="Add as a proposed item you rule on"
    >
      <Icon name="map" size={12} /> Add to roadmap
    </button>
  );
}
