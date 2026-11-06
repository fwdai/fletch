// The two lists the project screen's Roadmap segment draws, from the board the
// store holds — through the desktop board's own selectors, so a capture's chip
// says here exactly what the Mac's Inbox lane says.

import type { RoadmapItem } from "@desktop/api/types/roadmap";
import { flagOn, TRIAGE_KEY } from "@desktop/components/ProjectScreen/Roadmap/autonomy";
import { buildInbox, inboxChip } from "@desktop/components/ProjectScreen/Roadmap/inbox";
import type { RoadmapBoard } from "../store/roadmap";

export interface RoadmapLists {
  /** The Inbox lane's cards — untriaged captures and ideas — each with where
   *  triage stands. Mined brainstorms stay folded away, as on the Mac. */
  inbox: { item: RoadmapItem; chip: string }[];
  /** What is lined up to be built: the Next horizon's open and queued rows, in
   *  board (rank) order. */
  next: RoadmapItem[];
}

export function roadmapLists({ items, latest, settings }: RoadmapBoard): RoadmapLists {
  const triageOn = flagOn(settings[TRIAGE_KEY], true);
  return {
    inbox: buildInbox({ rows: items, latestEvents: latest }).open.map((entry) => ({
      item: entry.item,
      chip: inboxChip(entry, triageOn),
    })),
    next: items
      .filter((i) => i.horizon === "next" && (i.status === "open" || i.status === "queued"))
      .sort((a, b) => a.rank - b.rank),
  };
}
