import { describe, expect, it } from "vitest";
import { draftNames } from "@/helpers";
import type { DraftAgent } from "@/store";

function draft(name: string): DraftAgent {
  return { id: `draft-${name}`, name } as DraftAgent;
}

describe("draftNames", () => {
  it("reserves every open draft's name so two drafts never collide", () => {
    expect(draftNames([draft("gobi"), draft("kyoto")])).toEqual(["gobi", "kyoto"]);
  });

  it("is empty with no drafts", () => {
    expect(draftNames([])).toEqual([]);
  });

  // Regression guard for the `-2` suffix bug: this helper must NOT reach into
  // `workspace.agents`. That list carries archived agents (History reads the
  // same one), so folding it in burned a pool slot per archive until the
  // allocator fell back to numbered suffixes. Live agents are the DB's job —
  // see `allocate_draft_name` / `live_agent_ids`.
  it("takes only drafts — it has no access to the agent list", () => {
    expect(draftNames.length).toBe(1);
  });
});
