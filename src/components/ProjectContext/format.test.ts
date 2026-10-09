import { describe, expect, it } from "vitest";
import type { ContextProposal } from "@/api";
import { sortForReview } from "./format";

function proposal(id: string, quotes: number, created_at: number): ContextProposal {
  return {
    id,
    project_id: "p",
    payload: {
      type: "entity",
      input: { slug: id, kind: "feature", name: id, summary: "" },
      stamp: {
        author: { kind: "extractor" },
        source: { kind: "agent_turn" },
        provenance: {},
      },
    },
    evidence: Array.from({ length: quotes }, (_, i) => ({ quote: `q${i}` })),
    status: "pending",
    created_at,
  } as ContextProposal;
}

describe("sortForReview", () => {
  it("puts the most-repeated first, then the oldest", () => {
    const sorted = sortForReview([
      proposal("new-once", 1, 30),
      proposal("old-once", 1, 10),
      proposal("thrice", 3, 40),
      proposal("none", 0, 5),
      proposal("twice", 2, 20),
    ]);
    expect(sorted.map((p) => p.id)).toEqual(["thrice", "twice", "old-once", "new-once", "none"]);
  });

  it("leaves its input alone", () => {
    const input = [proposal("a", 1, 2), proposal("b", 2, 1)];
    sortForReview(input);
    expect(input.map((p) => p.id)).toEqual(["a", "b"]);
  });
});
