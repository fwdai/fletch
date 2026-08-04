import { describe, expect, it } from "vitest";
import type { RoadmapProposal } from "@/api";
import { cardRuling, pendingDeltas, pendingElsewhere } from "./ruling";

function ask(over: Partial<RoadmapProposal> = {}): RoadmapProposal {
  return {
    id: "pr-1",
    item_id: "i-1",
    project_id: "p1",
    kind: "update",
    patch: { title: "A better title" },
    note: "clearer",
    created_at: 0,
    ...over,
  };
}

describe("cardRuling", () => {
  it("draws nothing on a plain row", () => {
    const r = cardRuling(false, null);
    expect(r.kind).toBe("none");
    expect(r.label).toBe("");
    expect(r.showsDiff).toBe(false);
  });

  it("asks for admission on a ghost", () => {
    const r = cardRuling(true, null);
    expect(r.kind).toBe("ghost");
    expect(r.admits).toBe(true);
    expect(r.appliesPatch).toBe(false);
    expect(r.declineRemovesRow).toBe(true);
    expect(r.variant).toBe("ghost");
  });

  it("asks for the patch on a row already on the roadmap", () => {
    const r = cardRuling(false, ask());
    expect(r.kind).toBe("ask");
    expect(r.admits).toBe(false);
    expect(r.appliesPatch).toBe(true);
    expect(r.declineRemovesRow).toBe(false);
    expect(r.declineLabel).toBe("Decline");
    expect(r.variant).toBe("prop");
  });

  // The bug: a ghost carrying an ask showed the ask's *diff* with no bar able to
  // rule it, and the batch bar dropped the ask from its count. One ruling now
  // covers both, and the diff is rendered exactly when it can be ruled.
  it("folds a ghost and an ask against it into one ruling", () => {
    const r = cardRuling(true, ask());
    expect(r.kind).toBe("revised");
    expect(r.admits).toBe(true);
    expect(r.appliesPatch).toBe(true);
    expect(r.showsDiff).toBe(true);
    expect(r.declineRemovesRow).toBe(true);
    expect(r.declineLabel).toBe("Discard");
    expect(r.variant).toBe("ghost");
    expect(r.proposal?.id).toBe("pr-1");
  });

  it("never shows a diff nobody can rule", () => {
    for (const r of [
      cardRuling(false, null),
      cardRuling(true, null),
      cardRuling(false, ask({ kind: "discard", patch: null })),
      cardRuling(true, ask({ kind: "discard", patch: null })),
    ]) {
      expect(r.showsDiff).toBe(false);
    }
    // …and shows it in exactly the two cases where a patch is on the table.
    expect(cardRuling(false, ask()).showsDiff).toBe(true);
    expect(cardRuling(true, ask()).showsDiff).toBe(true);
  });

  it("reads a withdrawal on a ghost as the PM taking its suggestion back", () => {
    const r = cardRuling(true, ask({ kind: "discard", patch: null }));
    expect(r.kind).toBe("revised");
    expect(r.appliesPatch, "there is no patch on a discard ask").toBe(false);
    expect(r.label).toContain("withdrawn");
  });
});

describe("pendingDeltas", () => {
  const order = { project_id: "p1", codes: ["FLT-100"], note: null, created_at: 0 };
  const brief = { project_id: "p1", content: "# vision", note: null, created_at: 0 };

  it("counts all four kinds, not two", () => {
    const d = pendingDeltas({
      ghostIds: ["g1", "g2"],
      asks: [ask({ id: "a1", item_id: "i-1" })],
      orderProposal: order,
      briefProposal: brief,
    });
    expect(d).toMatchObject({ ghosts: 2, asks: 1, order: 1, brief: 1, total: 5, batch: 3 });
  });

  it("is empty on a board with nothing pending", () => {
    const d = pendingDeltas({ ghostIds: [], asks: [], orderProposal: null, briefProposal: null });
    expect(d.total).toBe(0);
    expect(d.batch).toBe(0);
    expect(d.askIds).toEqual([]);
    expect(d.declinableAskIds).toEqual([]);
  });

  // The B4 half: an ask against a ghost is a real pending delta the user must
  // rule (the backend keeps quoting it), so it counts — where the batch bar used
  // to drop it entirely.
  it("counts an ask against a ghost, and accepts it after the admission", () => {
    const d = pendingDeltas({
      ghostIds: ["g1"],
      asks: [ask({ id: "a1", item_id: "g1" })],
      orderProposal: null,
      briefProposal: null,
    });
    expect(d.asks).toBe(1);
    expect(d.batch, "the ghost and its revision are both owed a ruling").toBe(2);
    expect(d.askIds).toEqual(["a1"]);
  });

  // …but it cannot be *declined* on its own: discarding the ghost deletes the row
  // and the backend cascades the ask away with it.
  it("keeps a ghost's ask out of the declinable set", () => {
    const d = pendingDeltas({
      ghostIds: ["g1"],
      asks: [ask({ id: "a1", item_id: "g1" }), ask({ id: "a2", item_id: "i-9" })],
      orderProposal: null,
      briefProposal: null,
    });
    expect(d.askIds).toEqual(["a1", "a2"]);
    expect(d.declinableAskIds).toEqual(["a2"]);
  });

  it("does not let the batch bar claim the board-scoped pair", () => {
    const d = pendingDeltas({
      ghostIds: [],
      asks: [],
      orderProposal: order,
      briefProposal: brief,
    });
    expect(d.total).toBe(2);
    expect(d.batch, "neither is ruled from the batch bar").toBe(0);
  });
});

describe("pendingElsewhere", () => {
  const base = { ghostIds: ["g1"], asks: [], orderProposal: null, briefProposal: null };
  const order = { project_id: "p1", codes: [], note: null, created_at: 0 };
  const brief = { project_id: "p1", content: "", note: null, created_at: 0 };

  it("says nothing when the batch bar covers everything", () => {
    expect(pendingElsewhere(pendingDeltas(base))).toBe("");
  });

  it("names the surfaces the batch buttons don't reach", () => {
    expect(pendingElsewhere(pendingDeltas({ ...base, orderProposal: order }))).toBe(
      "Also pending: a new order.",
    );
    expect(pendingElsewhere(pendingDeltas({ ...base, briefProposal: brief }))).toBe(
      "Also pending: a brief update.",
    );
    expect(
      pendingElsewhere(pendingDeltas({ ...base, orderProposal: order, briefProposal: brief })),
    ).toBe("Also pending: a new order and a brief update.");
  });
});
