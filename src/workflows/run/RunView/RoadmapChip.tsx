// The run monitor's link back up a level: the roadmap item this run was
// dispatched for (`wf_run.roadmap_item_id`, migration 0028).
//
// A run is the answer to a question the board asked, and until now the monitor
// never said which question. The chip names the item and jumps to it — the
// board's own row, expanded and ringed (see `ui.focusRoadmapItem`).
//
// Followed by id rather than passed in: nothing above this pane holds the board,
// and a run outlives the screen the item was queued from. *Followed*, not fetched
// once — every word on the chip can change while it is on screen, and it used to
// show whatever was true at mount: an accepted PM retitle left the old title, and
// an item the merge sweep shipped kept a live-looking button whose click the board
// then silently swallowed (a `done` row leaves the board, so there is nothing to
// scroll to). One subscription (src/roadmapRows.ts, the same `roadmap:item`
// stream the board rides) makes the title and the clickability derived rather
// than remembered. An item that has since been deleted resolves to nothing and
// the chip renders nothing — a dead link is worse than no link.

import { Icon } from "../../../components/Icon";
import { useRoadmapRow } from "../../../roadmapRows";
import { useAppStore } from "../../../store";

export function RoadmapChip({ itemId, projectId }: { itemId: string; projectId: string }) {
  const item = useRoadmapRow(itemId);
  // The project screen is keyed by a repo path, so the jump needs the project's
  // primary repo — the first pinned repo that maps to it, which is exactly what
  // the sidebar's project group is keyed on.
  const repoPath = useAppStore(
    (s) => s.workspace?.projects.find((p) => p.project_id === projectId)?.path ?? null,
  );
  const focusRoadmapItem = useAppStore((s) => s.focusRoadmapItem);

  if (!item) return null;
  const body = (
    <>
      <Icon name="map" size={11} />
      <span className="mono">{item.code}</span>
      <span className="t-ellipsis">{item.title}</span>
    </>
  );
  // No pinned repo for the project (it was unpinned, or this run outlived it),
  // or the item shipped: a `done` row leaves the board entirely, so the jump
  // would open the roadmap and land on nothing — the common case for a
  // finished run. The item is still worth naming; there is just nowhere to go.
  // Derived from the live row, so a sweep that ships the item mid-view turns the
  // button into that statement rather than leaving a click that does nothing.
  if (!repoPath || item.status === "done") return <span className="wf-run-rm">{body}</span>;
  return (
    <button
      type="button"
      className="wf-run-rm"
      title={`Show ${item.code} on the roadmap`}
      onClick={() => focusRoadmapItem(repoPath, item.code)}
    >
      {body}
    </button>
  );
}
