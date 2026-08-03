import { describe, expect, it } from "vitest";
import type { RoadmapItem } from "@/api";
import { buildProposalDiff, isEmptyDiff } from "./proposalDiff";

function item(over: Partial<RoadmapItem> = {}): RoadmapItem {
  return {
    id: "i1",
    project_id: "p1",
    code: "FLT-100",
    parent_id: null,
    title: "Old title",
    why: "old why",
    horizon: "later",
    status: "open",
    rank: 1,
    area: "workflow",
    source: "user",
    accept: ["a", "b"],
    deps: [],
    agent_id: null,
    workflow_def_id: null,
    run_id: null,
    pr_url: null,
    pr_number: null,
    created_at: 0,
    updated_at: 0,
    ...over,
  };
}

describe("buildProposalDiff", () => {
  it("pairs each patched text field with the current value, in reading order", () => {
    const diff = buildProposalDiff(item(), {
      title: "New title",
      horizon: "now",
    });
    expect(diff.texts).toEqual([
      { field: "title", label: "Title", from: "Old title", to: "New title" },
      { field: "horizon", label: "Horizon", from: "later", to: "now" },
    ]);
    expect(diff.lists).toEqual([]);
  });

  it("keeps only real differences — a patch restating the row diffs empty", () => {
    const diff = buildProposalDiff(item(), {
      title: "Old title",
      why: "old why",
      accept: ["a", "b"],
    });
    expect(isEmptyDiff(diff)).toBe(true);
  });

  it("treats an explicit null as clearing, and a first value as setting", () => {
    // `area: null` clears; a why where there was none sets from nothing.
    const diff = buildProposalDiff(item({ why: "" }), { area: null, why: "because" });
    expect(diff.texts).toEqual([
      { field: "why", label: "Why", from: null, to: "because" },
      { field: "area", label: "Area", from: "workflow", to: null },
    ]);
  });

  it("merges a list as the proposed order plus the removals, tagged", () => {
    const diff = buildProposalDiff(item({ accept: ["a", "b"], deps: ["FLT-90"] }), {
      accept: ["b", "c"],
      deps: [],
    });
    expect(diff.lists).toEqual([
      {
        field: "accept",
        label: "Done when",
        entries: [
          { text: "b", change: "kept" },
          { text: "c", change: "added" },
          { text: "a", change: "removed" },
        ],
      },
      {
        field: "deps",
        label: "After",
        entries: [{ text: "FLT-90", change: "removed" }],
      },
    ]);
  });

  it("ignores fields the patch never touched, even when they look clearable", () => {
    const diff = buildProposalDiff(item({ area: null }), { title: "New" });
    expect(diff.texts).toHaveLength(1);
    expect(diff.texts[0].field).toBe("title");
  });
});
