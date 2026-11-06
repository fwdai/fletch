// The project screen's Roadmap segment: one project's board as the host holds
// it, read whole when the segment opens and kept live by the events the desktop
// board follows. What the segment lists is derived from it in lib/roadmap.ts,
// with the desktop's own selectors.
//
// Beside the PM proposals (proposals.ts) rather than folded into them: those
// are the ghosts a planning chat asks about, this is the board itself.

import type { RoadmapItem, RoadmapItemEvent } from "@desktop/api/types/roadmap";
import type { ProjectSettingsChangedEvent } from "@desktop/api/types/settings";
import { upsertLatest } from "@desktop/components/RoadmapScreen/Roadmap/NeedsYou/select";
import { applyRowEvent } from "@desktop/rowSync";
import type { Api } from "../api";
import type { MobileState } from "./index";

type Set = (partial: Partial<MobileState> | ((s: MobileState) => Partial<MobileState>)) => void;
type Get = () => MobileState;

export interface RoadmapBoard {
  /** Every row, in the host's order, rows that arrive live appended. */
  items: RoadmapItem[];
  /** The newest history event per row — `roadmap_latest_events`, then every
   *  `roadmap:item-event`. */
  latest: RoadmapItemEvent[];
  /** The project's settings — `get_project_settings`, then every
   *  `project_settings:changed`. The Inbox reads `roadmap.triage` off it. */
  settings: Record<string, string>;
}

export interface RoadmapSlice {
  /** Per project id; absent until the segment has read that board. */
  boards: Record<string, RoadmapBoard>;
  loadBoard(projectId: string): Promise<void>;
  /** Drop a note in the project's inbox. Rejects with the host's reason, so the
   *  field can keep the text. */
  captureRoadmapItem(projectId: string, text: string): Promise<void>;
  /** The live folds. Each is a no-op for a board nothing has read, so the event
   *  handlers can call them unconditionally. */
  applyBoardRow(item: RoadmapItem): void;
  removeBoardRow(id: string): void;
  applyBoardEvent(event: RoadmapItemEvent): void;
  applyProjectSetting(change: ProjectSettingsChangedEvent): void;
}

export function createRoadmapSlice(set: Set, get: Get, api: Api): RoadmapSlice {
  /** Change one board that has been read, leaving every other at its reference. */
  const patch = (projectId: string, change: (board: RoadmapBoard) => Partial<RoadmapBoard>) =>
    set((s) => {
      const board = s.boards[projectId];
      return board ? { boards: { ...s.boards, [projectId]: { ...board, ...change(board) } } } : {};
    });

  const applyBoardRow = (item: RoadmapItem) =>
    patch(item.project_id, (b) => ({
      items: applyRowEvent(b.items, { kind: "upsert", row: item }),
    }));

  return {
    boards: {},

    async loadBoard(projectId) {
      try {
        const [items, latest, settings] = await Promise.all([
          api.roadmapListItems(projectId),
          // Both only shade the Inbox's wording; neither is worth the lists.
          api.roadmapLatestEvents(projectId).catch(() => []),
          api.getProjectSettings(projectId).catch(() => ({})),
        ]);
        set((s) => ({ boards: { ...s.boards, [projectId]: { items, latest, settings } } }));
      } catch {
        // Advisory, like the other list reads: the segment keeps what it had
        // and the next open or reconnect tries again.
      }
    },

    async captureRoadmapItem(projectId, text) {
      // Folded in on the answer rather than on its `roadmap:item`, so the row
      // is in the Inbox by the time the field clears.
      applyBoardRow(await api.roadmapCapture(projectId, text));
    },

    applyBoardRow,

    removeBoardRow(id) {
      // The event carries the bare id, so the board is found by its row.
      const { boards } = get();
      const projectId = Object.keys(boards).find((pid) =>
        boards[pid].items.some((i) => i.id === id),
      );
      if (projectId) patch(projectId, (b) => ({ items: b.items.filter((i) => i.id !== id) }));
    },

    applyBoardEvent(event) {
      patch(event.project_id, (b) => ({ latest: upsertLatest(b.latest, event) }));
    },

    applyProjectSetting({ project_id, key, value }) {
      patch(project_id, (b) => {
        const settings = { ...b.settings };
        // `null` is a deleted row: the key is back to its default.
        if (value === null) delete settings[key];
        else settings[key] = value;
        return { settings };
      });
    },
  };
}
