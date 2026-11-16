import type { RoadmapItem } from "@/api";

/** A roadmap item as a starting prompt for an agent: what to build, why, and
 *  what "done" means. Seeds a composer (the board's "Send to an agent", the
 *  add menu's roadmap list) so the user reviews and launches rather than
 *  retyping the row. */
export function roadmapItemBrief(
  item: Pick<RoadmapItem, "code" | "title" | "why"> & { accept?: string[] },
): string {
  const lines = [`${item.code}: ${item.title}`, "", item.why];
  if (item.accept?.length) lines.push("", "Done when:", ...item.accept.map((a) => `- ${a}`));
  return lines.join("\n").trim();
}
