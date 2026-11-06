// The Inbox lane's model: the captured rows, what triage made of each, and the
// words its card says. Pure, like partition.ts, so the chip and the mined fold
// are testable without a DOM.

import type { RoadmapItem, RoadmapItemEvent } from "@/api";
import { isCaptured } from "./partition";

export interface InboxEntry {
  item: RoadmapItem;
  /** Codes proposed from this row (`from_item`), in board order. */
  derived: string[];
  /** The host's word on why triage hasn't started — its deferral note, while
   *  that is the newest thing said about an untriaged row. */
  waiting: string | null;
}

export interface Inbox {
  /** Untriaged rows and ideas — the cards the lane shows. */
  open: InboxEntry[];
  /** Brainstorms whose tickets have been proposed, folded under "Mined". */
  mined: InboxEntry[];
}

export function buildInbox({
  rows,
  latestEvents,
}: {
  rows: readonly RoadmapItem[];
  latestEvents: readonly RoadmapItemEvent[];
}): Inbox {
  const inbox: Inbox = { open: [], mined: [] };
  for (const item of rows) {
    if (!isCaptured(item)) continue;
    const latest = latestEvents.find((e) => e.item_id === item.id);
    const entry: InboxEntry = {
      item,
      derived: rows.filter((r) => r.from_item === item.id).map((r) => r.code),
      waiting:
        item.kind == null && latest?.kind === "note" && latest.actor === "drainer"
          ? latest.detail
          : null,
    };
    (item.triage?.mined ? inbox.mined : inbox.open).push(entry);
  }
  return inbox;
}

/** The chip on a card's line: where triage stands. `triageOn` is the project's
 *  `roadmap.triage` dial — off, an untriaged row waits for the user, not the PM. */
export function inboxChip(entry: InboxEntry, triageOn: boolean): string {
  const { kind, triage } = entry.item;
  if (kind === "idea") {
    const n = triage?.questions?.length ?? 0;
    return `Idea · brief, ${n} question${n === 1 ? "" : "s"}`;
  }
  if (triage?.mined) {
    return entry.derived.length ? `Mined → ${entry.derived.join(", ")}` : "Mined";
  }
  if (!triageOn) return "Untriaged";
  return entry.waiting ? "Waiting for a PM chat" : "Triaging…";
}

/** The opening turn of "Open in PM chat": the code, so the PM reads the row
 *  itself, and the op that settles it. */
export function inboxChatPrompt(item: RoadmapItem): string {
  const verb = item.kind === "idea" ? "work through the idea" : "triage";
  return `Let's ${verb} ${item.code} from the roadmap inbox ("${item.title}"). Read it with roadmap_list, ask me what you need, and settle it with roadmap_triage once we agree.`;
}
