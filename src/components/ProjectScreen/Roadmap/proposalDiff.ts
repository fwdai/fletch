// What a pending update proposal would change, as the card's diff renders it —
// pure over the item and the patch, kept out of the JSX so the pairing rules
// (which fields show, what counts as a change, how lists merge) are testable
// without React.
//
// The patch's keys are exactly the fields the PM touched (the backend stores it
// that way), but a touched field isn't necessarily a *changed* one — a patch
// restating the current title would render as a diff of nothing. Every entry
// here is a real difference; a proposal whose patch is all no-ops diffs empty,
// and the card shows only the bar.

import type { RoadmapItem, RoadmapProposalPatch } from "@/api";

/** A scalar field's before/after. `from`/`to` are display strings; null means
 *  "nothing" (an empty `why`, a cleared `area`) — the renderer decides how to
 *  draw absence, this module only decides that it differs. */
export interface TextDiff {
  field: "title" | "why" | "horizon" | "area";
  label: string;
  from: string | null;
  to: string | null;
}

/** One line of a list diff: kept entries anchor the reading, added ones are
 *  highlighted, removed ones struck. Order is the proposed list's (the state
 *  being asked for), with the removals appended at the end. */
export interface ListEntry {
  text: string;
  change: "kept" | "added" | "removed";
}

export interface ListDiff {
  field: "accept" | "deps";
  label: string;
  entries: ListEntry[];
}

export interface ProposalDiff {
  texts: TextDiff[];
  lists: ListDiff[];
}

/** Collapse "no value" spellings so `""` vs absent vs untouched all compare as
 *  the same nothing. */
const norm = (v: string | null | undefined): string | null => {
  const s = (v ?? "").trim();
  return s === "" ? null : s;
};

/** Pair each patched field with the item's current value, keeping only real
 *  differences. Field order is the card's own reading order. */
export function buildProposalDiff(item: RoadmapItem, patch: RoadmapProposalPatch): ProposalDiff {
  const texts: TextDiff[] = [];
  const text = (field: TextDiff["field"], label: string, from: string | null) => {
    // `in` rather than truthiness: `area: null` is a present key that clears.
    if (!(field in patch)) return;
    const to = norm(patch[field]);
    if (to !== from) texts.push({ field, label, from, to });
  };
  text("title", "Title", norm(item.title));
  text("why", "Why", norm(item.why));
  text("horizon", "Horizon", item.horizon);
  text("area", "Area", norm(item.area));

  const lists: ListDiff[] = [];
  const list = (field: ListDiff["field"], label: string, from: string[]) => {
    const to = patch[field];
    if (!to) return;
    const current = new Set(from);
    const proposed = new Set(to);
    if (from.length === proposed.size && from.every((e) => proposed.has(e))) return;
    lists.push({
      field,
      label,
      entries: [
        ...to.map<ListEntry>((text) => ({
          text,
          change: current.has(text) ? "kept" : "added",
        })),
        ...from
          .filter((e) => !proposed.has(e))
          .map<ListEntry>((text) => ({ text, change: "removed" })),
      ],
    });
  };
  list("accept", "Done when", item.accept);
  list("deps", "After", item.deps);

  return { texts, lists };
}

/** Nothing would actually change — the card shows the bar (and the note) but
 *  no diff block. */
export function isEmptyDiff(diff: ProposalDiff): boolean {
  return diff.texts.length === 0 && diff.lists.length === 0;
}
