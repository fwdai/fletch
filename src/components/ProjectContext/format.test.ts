import { describe, expect, it } from "vitest";
import type { ContextProposal } from "@/api";
import { corroborationLabel, sortForReview } from "./format";

const stamp = {
  author: { kind: "extractor" },
  source: { kind: "agent_turn" },
  provenance: {},
};

function entity(id: string, created_at: number): ContextProposal {
  return {
    id,
    project_id: "p",
    payload: {
      type: "entity",
      input: { slug: id, kind: "feature", name: id, summary: "" },
      stamp,
    },
    evidence: [],
    status: "pending",
    created_at,
  } as unknown as ContextProposal;
}

/** An assertion proposal with one quote per entry of `turns`; `null` for a
 *  quote with no turn. */
function assertion(id: string, turns: (string | null)[], created_at: number): ContextProposal {
  return {
    id,
    project_id: "p",
    payload: {
      type: "assertion",
      input: { statement: id, about: [] },
      stamp,
      relation: { kind: "new" },
      about_pending: [],
    },
    evidence: turns.map((turn_id, i) => ({ quote: `q${i}`, ...(turn_id ? { turn_id } : {}) })),
    status: "pending",
    created_at,
  } as unknown as ContextProposal;
}

describe("sortForReview", () => {
  it("puts entities first, then the most distinct turns, then the oldest", () => {
    const sorted = sortForReview([
      assertion("new-once", ["t1"], 30),
      assertion("old-once", ["t2"], 10),
      assertion("one-turn-thrice", ["t3", "t3", "t3"], 5),
      assertion("two-turns", ["t4", "t5"], 40),
      entity("entity", 50),
      assertion("none", [], 1),
    ]);
    expect(sorted.map((p) => p.id)).toEqual([
      "entity",
      "two-turns",
      "one-turn-thrice",
      "old-once",
      "new-once",
      "none",
    ]);
  });

  it("counts quotes when none carries a turn", () => {
    const sorted = sortForReview([
      assertion("one", [null], 1),
      assertion("three", [null, null, null], 2),
    ]);
    expect(sorted.map((p) => p.id)).toEqual(["three", "one"]);
  });

  it("leaves its input alone", () => {
    const input = [assertion("a", ["t1"], 2), entity("b", 1)];
    sortForReview(input);
    expect(input.map((p) => p.id)).toEqual(["a", "b"]);
  });
});

describe("corroborationLabel", () => {
  it("names turns when there are several, else quotes", () => {
    expect(corroborationLabel(assertion("a", ["t1", "t2", "t2"], 0))).toBe("2 turns");
    expect(corroborationLabel(assertion("a", ["t1", "t1"], 0))).toBe("2 quotes");
    expect(corroborationLabel(assertion("a", ["t1"], 0))).toBeNull();
  });
});
