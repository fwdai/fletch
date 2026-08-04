import { Icon } from "@/components/Icon";
import type { FunnelAction } from "./funnel";

/** The roadmap half of an inbox row's actions. Four states, decided in funnel.ts:
 *  offer the route, say it's already there, say it was turned down, or say nothing
 *  at all — no project owns the repo, or its board hasn't answered yet, and an Add
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
  // Routed once and discarded on the board. Said out loud rather than left as a
  // bare row, because the absence of an Add button is otherwise indistinguishable
  // from a board that failed to load — and re-offering Add is what made a
  // discarded issue come back on every read, forever.
  if (action.kind === "declined") {
    return (
      // Same quiet statement styling as `routed`: both are facts about the board
      // rather than things to click.
      <button
        type="button"
        className="mc-btn mc-inbox-routed"
        disabled
        title="You discarded this from the roadmap — it won't be offered again"
      >
        <Icon name="close" size={12} /> Not on roadmap
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
